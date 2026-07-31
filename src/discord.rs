use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration as StdDuration,
};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use indicatif::ProgressBar;
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore};

use crate::{
    fetch::{FetchResult, SearchProgress},
    models::SearchResponse,
    models::{Channel, Guild, Me, Message, OwnedMessage},
    proxy, terminal,
};

const API_BASE: &str = "https://discord.com/api/v10";
const FETCH_PAGE_SIZE: usize = 100;
const MAX_DELETE_ATTEMPTS: usize = 10;
const MAX_RATE_LIMIT_RETRIES: usize = 20;
const HISTORY_SCAN_RETRIES: usize = 2;
const TRANSIENT_DELETE_BACKOFF_BASE_SECS: f64 = 0.75;
const TRANSIENT_DELETE_BACKOFF_MAX_SECS: f64 = 12.0;

const DELETE_PACE_START_MS: u64 = 15;
const DELETE_PACE_FAST_START_MS: u64 = 0;
const DELETE_PACE_MIN_MS: u64 = 0;
const DELETE_PACE_MAX_MS: u64 = 1_500;
const DELETE_PACE_STEP_DOWN_MS: u64 = 5;
const DELETE_PACE_STEP_UP_FACTOR: f64 = 1.8;

const SEARCH_PACE_MS: u64 = 40;
const GUILD_SEARCH_PREFETCH_LIMIT: usize = 75_000;

fn is_video_attachment(attachment: &crate::models::Attachment) -> bool {
    if let Some(content_type) = attachment.content_type.as_deref() {
        if content_type.to_ascii_lowercase().starts_with("video/") {
            return true;
        }
    }

    let Some(filename) = attachment.filename.as_deref() else {
        return false;
    };

    let extension = filename
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "mp4"
            | "mov"
            | "webm"
            | "mkv"
            | "avi"
            | "m4v"
            | "flv"
            | "gifv"
            | "m4p"
            | "mpeg"
            | "mpg"
            | "m2ts"
            | "wmv"
            | "ogv"
            | "3gp"
            | "ts",
    )
}

fn content_has_link(content: &str) -> bool {
    let lowered = content.to_ascii_lowercase();
    lowered.contains("http://")
        || lowered.contains("https://")
        || lowered.contains("www.")
        || lowered.contains("discordapp.com")
        || lowered.contains("discord.com")
}

fn owned_message(
    id: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    content: String,
    attachments: &[crate::models::Attachment],
) -> OwnedMessage {
    let has_video = attachments.iter().any(is_video_attachment);
    let has_media = !attachments.is_empty();
    OwnedMessage {
        id,
        timestamp,
        content: content.clone(),
        has_link: content_has_link(&content),
        has_media,
        has_file: has_media && !attachments.iter().all(is_video_attachment),
        has_video,
    }
}

pub struct SkippedDelete {
    pub message: OwnedMessage,
    pub status: u16,
    pub reason: String,
}

pub enum DeleteProgress {
    Deleted(OwnedMessage),
    Skipped(SkippedDelete),
}

struct DeleteRetryContext<'a> {
    channel_id: &'a str,
    display_name: &'a str,
    message_id: &'a str,
    attempt: usize,
    max_attempts: usize,
    wait_secs: f64,
    progress: Option<&'a ProgressBar>,
}

pub struct DiscordClient {
    pool: Vec<Client>,
    pool_next: AtomicUsize,
    request_limit: Arc<Semaphore>,
    delete_pacers: Arc<Mutex<HashMap<String, DeletePacer>>>,
}

struct DeletePacer {
    delay_ms: u64,
}

impl DeletePacer {
    fn new(initial_delay_ms: u64) -> Self {
        Self {
            delay_ms: initial_delay_ms,
        }
    }

    fn current_delay(&self) -> u64 {
        self.delay_ms
    }

    fn on_success(&mut self) -> u64 {
        self.delay_ms = self.delay_ms.saturating_sub(DELETE_PACE_STEP_DOWN_MS);
        self.delay_ms
    }

    fn on_rate_limit(&mut self, retry_after_secs: f64) -> u64 {
        let backed_off = ((self.delay_ms as f64) * DELETE_PACE_STEP_UP_FACTOR).round() as u64;
        let retry_floor = (retry_after_secs * 1_000.0).ceil() as u64;
        self.delay_ms = backed_off
            .max(retry_floor)
            .clamp(DELETE_PACE_MIN_MS, DELETE_PACE_MAX_MS);
        self.delay_ms
    }
}

fn initial_delete_delay_ms(aggressive: bool) -> u64 {
    if aggressive {
        DELETE_PACE_FAST_START_MS
    } else {
        DELETE_PACE_START_MS
    }
}

impl DiscordClient {
    pub fn new(token: &str, max_in_flight: usize) -> Result<Self> {
        let client = proxy::build_direct_client(token)?;
        Ok(Self::from_clients(vec![client], max_in_flight))
    }

    pub fn from_clients(clients: Vec<Client>, max_in_flight: usize) -> Self {
        Self {
            pool: clients,
            pool_next: AtomicUsize::new(0),
            request_limit: Arc::new(Semaphore::new(max_in_flight.max(1))),
            delete_pacers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn pick_client(&self) -> &Client {
        let n = self.pool_next.fetch_add(1, Ordering::Relaxed);
        &self.pool[n % self.pool.len()]
    }

    fn scaled_wait(&self, retry_after: f64) -> f64 {
        let n = self.pool.len();
        if n > 1 {
            (retry_after / n as f64).max(0.1)
        } else {
            retry_after + 0.5
        }
    }

    pub async fn me(&self) -> Result<Me> {
        self.get_json("/users/@me").await
    }

    pub async fn guild(&self, guild_id: &str) -> Result<Guild> {
        self.get_json(&format!("/guilds/{guild_id}")).await
    }

    pub async fn channel(&self, channel_id: &str) -> Result<Channel> {
        self.get_json(&format!("/channels/{channel_id}")).await
    }

    pub async fn message(&self, channel_id: &str, message_id: &str) -> Result<Message> {
        self.get_json(&format!("/channels/{channel_id}/messages/{message_id}"))
            .await
    }

    pub async fn guild_channels(&self, guild_id: &str) -> Result<Vec<Channel>> {
        self.get_json(&format!("/guilds/{guild_id}/channels")).await
    }

    pub async fn create_dm(&self, recipient_id: &str) -> Result<Channel> {
        self.post_json(
            "/users/@me/channels",
            &serde_json::json!({ "recipient_id": recipient_id }),
        )
        .await
    }

    pub async fn fetch_messages_in_timeframe(
        &self,
        channel_id: &str,
        cutoff: Option<DateTime<Utc>>,
        my_id: &str,
        prefer_history_scan: bool,
    ) -> Result<FetchResult> {
        match self
            .scan_messages_in_channel_with_retry(channel_id, cutoff, my_id, HISTORY_SCAN_RETRIES)
            .await
        {
            Ok(messages) => Ok(FetchResult {
                messages,
                method: "history-scan",
            }),
            Err(error) => {
                if prefer_history_scan {
                    return Err(error);
                }
                terminal::warn(
                    "kcordclient::discord",
                    format!(
                        "history scan failed for {}, falling back to search: {:#}",
                        channel_id, error
                    ),
                );
                Ok(FetchResult {
                    messages: self
                        .search_messages_in_channel(channel_id, cutoff, my_id)
                        .await?,
                    method: "search",
                })
            }
        }
    }

    pub async fn backfill_channel_history(
        &self,
        channel_id: &str,
        cutoff: Option<DateTime<Utc>>,
        my_id: &str,
    ) -> Result<Vec<OwnedMessage>> {
        self.scan_messages_in_channel_with_retry(channel_id, cutoff, my_id, HISTORY_SCAN_RETRIES)
            .await
    }

    pub async fn search_messages_in_guild<F>(
        &self,
        guild_id: &str,
        cutoff: Option<DateTime<Utc>>,
        my_id: &str,
        mut on_progress: F,
    ) -> Result<HashMap<String, Vec<OwnedMessage>>>
    where
        F: FnMut(SearchProgress),
    {
        let cutoff_time = cutoff.unwrap_or_else(|| Utc::now() - Duration::days(365 * 100));
        let mut grouped = HashMap::<String, Vec<OwnedMessage>>::new();
        let mut seen_ids = HashSet::new();
        let mut seen_count = 0usize;
        let mut cursor: Option<String> = None;

        loop {
            let mut path = format!(
                "/guilds/{guild_id}/messages/search?author_id={my_id}&include_nsfw=true&limit=25&sort_by=timestamp&sort_order=desc"
            );
            if let Some(max_id) = &cursor {
                path.push_str("&max_id=");
                path.push_str(max_id);
            }
            let response: SearchResponse = self.get_json(&path).await?;

            let total = response.total_results;
            let mut next_cursor: Option<String> = None;
            let mut reached_cutoff = false;

            for message in response.messages.into_iter().flatten() {
                update_oldest_cursor(&mut next_cursor, &message.id);
                if message.author.id != my_id {
                    continue;
                }
                if message.timestamp < cutoff_time {
                    reached_cutoff = true;
                    continue;
                }
                let Some(channel_id) = message.channel_id.clone() else {
                    continue;
                };
                if !seen_ids.insert(message.id.clone()) {
                    continue;
                }
                seen_count += 1;
                if seen_count > GUILD_SEARCH_PREFETCH_LIMIT {
                    return Err(anyhow!(
                        "guild search prefetch cap exceeded (>{GUILD_SEARCH_PREFETCH_LIMIT})"
                    ));
                }
                let attachments = &message.attachments;
                grouped.entry(channel_id).or_default().push(owned_message(
                    message.id,
                    message.timestamp,
                    message.content,
                    attachments,
                ));
            }

            on_progress(SearchProgress {
                total_results: total,
                collected_results: seen_ids.len() as u64,
            });

            if reached_cutoff {
                break;
            }

            let Some(next_cursor) = next_cursor else {
                break;
            };
            if cursor.as_ref() == Some(&next_cursor) {
                break;
            }
            cursor = Some(next_cursor);

            tokio::time::sleep(StdDuration::from_millis(SEARCH_PACE_MS)).await;
        }

        Ok(grouped)
    }

    async fn search_messages_in_channel(
        &self,
        channel_id: &str,
        cutoff: Option<DateTime<Utc>>,
        my_id: &str,
    ) -> Result<Vec<OwnedMessage>> {
        let cutoff_time = cutoff.unwrap_or_else(|| Utc::now() - Duration::days(365 * 100));
        let mut results = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut path = format!(
                "/channels/{channel_id}/messages/search?author_id={my_id}&include_nsfw=true&limit=25&sort_by=timestamp&sort_order=desc"
            );
            if let Some(max_id) = &cursor {
                path.push_str("&max_id=");
                path.push_str(max_id);
            }
            let response: SearchResponse = self.get_json(&path).await?;
            let mut next_cursor: Option<String> = None;
            let mut reached_cutoff = false;

            let batch = response
                .messages
                .into_iter()
                .flatten()
                .inspect(|m| update_oldest_cursor(&mut next_cursor, &m.id))
                .filter(|m| m.author.id == my_id)
                .filter_map(|m| {
                    if m.timestamp < cutoff_time {
                        reached_cutoff = true;
                        return None;
                    }
                    Some(m)
                })
                .filter(|m| seen_ids.insert(m.id.clone()))
                .map(|m| owned_message(m.id, m.timestamp, m.content, &m.attachments))
                .collect::<Vec<_>>();

            results.extend(batch);

            if reached_cutoff {
                break;
            }

            let Some(next_cursor) = next_cursor else {
                break;
            };
            if cursor.as_ref() == Some(&next_cursor) {
                break;
            }
            cursor = Some(next_cursor);

            tokio::time::sleep(StdDuration::from_millis(SEARCH_PACE_MS)).await;
        }

        Ok(results)
    }

    async fn scan_messages_in_channel(
        &self,
        channel_id: &str,
        cutoff: Option<DateTime<Utc>>,
        my_id: &str,
    ) -> Result<Vec<OwnedMessage>> {
        let mut messages = Vec::new();
        let mut before: Option<String> = None;
        let cutoff_time = cutoff.unwrap_or_else(|| Utc::now() - Duration::days(365 * 100));

        loop {
            let mut path = format!("/channels/{channel_id}/messages?limit={FETCH_PAGE_SIZE}");
            if let Some(before_id) = &before {
                path.push_str("&before=");
                path.push_str(before_id);
            }

            let page: Vec<Message> = self.get_json(&path).await?;
            if page.is_empty() {
                break;
            }

            let mut hit_old = false;
            for message in &page {
                if message.timestamp < cutoff_time {
                    hit_old = true;
                    break;
                }

                if message.author.id == my_id {
                    messages.push(owned_message(
                        message.id.clone(),
                        message.timestamp,
                        message.content.clone(),
                        &message.attachments,
                    ));
                }
            }

            if hit_old || page.len() < FETCH_PAGE_SIZE {
                break;
            }

            before = page.last().map(|message| message.id.clone());
        }

        Ok(messages)
    }

    async fn scan_messages_in_channel_with_retry(
        &self,
        channel_id: &str,
        cutoff: Option<DateTime<Utc>>,
        my_id: &str,
        retries: usize,
    ) -> Result<Vec<OwnedMessage>> {
        let mut attempts = 0usize;

        loop {
            match self
                .scan_messages_in_channel(channel_id, cutoff, my_id)
                .await
            {
                Ok(messages) => return Ok(messages),
                Err(error) if attempts < retries => {
                    attempts += 1;
                    terminal::warn(
                        "kcordclient::discord",
                        format!(
                            "history scan retry {attempts}/{retries} for {} after error: {:#}",
                            channel_id, error
                        ),
                    );
                    tokio::time::sleep(StdDuration::from_millis(250)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub async fn delete_messages(
        &self,
        channel_id: &str,
        display_name: &str,
        to_delete: &[OwnedMessage],
        progress: Option<&ProgressBar>,
        aggressive: bool,
        mut on_progress: impl FnMut(DeleteProgress) -> Result<()>,
    ) -> Result<()> {
        terminal::info(
            "kcordclient::discord",
            format!("Deleting batch of {} messages", to_delete.len()),
        );

        for message in to_delete {
            let message_id = &message.id;
            let path = format!("/channels/{channel_id}/messages/{message_id}");
            let mut attempts = 0;
            let mut rate_limit_retries = 0;

            while attempts < MAX_DELETE_ATTEMPTS {
                attempts += 1;
                let pace_before_request =
                    self.delete_delay_for_channel(channel_id, aggressive).await;
                if pace_before_request > 0 {
                    tokio::time::sleep(StdDuration::from_millis(pace_before_request)).await;
                }

                let response = match self.delete(&path).await {
                    Ok(response) => response,
                    Err(error) if attempts < MAX_DELETE_ATTEMPTS => {
                        let wait = transient_delete_backoff_secs(attempts);
                        self.record_delete_rate_limit(channel_id, aggressive, wait)
                            .await;
                        log_delete_retry(
                            DeleteRetryContext {
                                channel_id,
                                display_name,
                                message_id,
                                attempt: attempts,
                                max_attempts: MAX_DELETE_ATTEMPTS,
                                wait_secs: wait,
                                progress,
                            },
                            &format!("{:#}", error),
                        );
                        tokio::time::sleep(StdDuration::from_secs_f64(wait)).await;
                        continue;
                    }
                    Err(error) => {
                        return Err(anyhow!("failed to delete {message_id}: {:#}", error));
                    }
                };

                match response.status() {
                    status if status.is_success() || status == StatusCode::NOT_FOUND => {
                        let deleted_message = message.clone();
                        on_progress(DeleteProgress::Deleted(deleted_message))?;
                        if let Some(progress) = progress {
                            progress.inc(1);
                            set_delete_progress_message(progress, display_name, "Deleting");
                        }
                        self.record_delete_success(channel_id, aggressive).await;
                        break;
                    }
                    StatusCode::TOO_MANY_REQUESTS => {
                        rate_limit_retries += 1;
                        let retry_after = parse_retry_after(response, rate_limit_retries).await;
                        let wait = self.scaled_wait(retry_after);
                        self.record_delete_rate_limit(channel_id, aggressive, wait)
                            .await;
                        terminal::record_rate_limit(wait);
                        if rate_limit_retries >= MAX_RATE_LIMIT_RETRIES {
                            return Err(anyhow!(
                                "rate limit retries exceeded while deleting {message_id}"
                            ));
                        }
                        log_delete_retry(
                            DeleteRetryContext {
                                channel_id,
                                display_name,
                                message_id,
                                attempt: attempts,
                                max_attempts: MAX_DELETE_ATTEMPTS,
                                wait_secs: wait,
                                progress,
                            },
                            "rate limited",
                        );
                        tokio::time::sleep(StdDuration::from_secs_f64(wait)).await;
                    }
                    status
                        if is_retryable_delete_status(status) && attempts < MAX_DELETE_ATTEMPTS =>
                    {
                        let body = response.text().await.unwrap_or_default();
                        let reason = format_http_error(status, &body);
                        let wait = transient_delete_backoff_secs(attempts);
                        self.record_delete_rate_limit(channel_id, aggressive, wait)
                            .await;
                        log_delete_retry(
                            DeleteRetryContext {
                                channel_id,
                                display_name,
                                message_id,
                                attempt: attempts,
                                max_attempts: MAX_DELETE_ATTEMPTS,
                                wait_secs: wait,
                                progress,
                            },
                            &reason,
                        );
                        tokio::time::sleep(StdDuration::from_secs_f64(wait)).await;
                    }
                    status => {
                        let body = response.text().await.unwrap_or_default();
                        let reason = format_http_error(status, &body);
                        if status.is_client_error() {
                            terminal::warn(
                                "kcordclient::discord",
                                format!(
                                    "skipping undeletable message {} in {}: {}",
                                    message_id, channel_id, reason
                                ),
                            );
                            let skipped_delete = SkippedDelete {
                                message: message.clone(),
                                status: status.as_u16(),
                                reason,
                            };
                            on_progress(DeleteProgress::Skipped(SkippedDelete {
                                message: skipped_delete.message.clone(),
                                status: skipped_delete.status,
                                reason: skipped_delete.reason.clone(),
                            }))?;
                            if let Some(progress) = progress {
                                progress.inc(1);
                                set_delete_progress_message(progress, display_name, "Deleting");
                            }
                            break;
                        }
                        return Err(anyhow!("failed to delete {message_id}: {}", reason));
                    }
                }
            }

            if attempts == MAX_DELETE_ATTEMPTS {
                return Err(anyhow!("max retries reached while deleting {message_id}"));
            }
        }

        Ok(())
    }

    async fn delete_delay_for_channel(&self, channel_id: &str, aggressive: bool) -> u64 {
        let mut pacers = self.delete_pacers.lock().await;
        pacers
            .entry(channel_id.to_string())
            .or_insert_with(|| DeletePacer::new(initial_delete_delay_ms(aggressive)))
            .current_delay()
    }

    async fn record_delete_success(&self, channel_id: &str, aggressive: bool) {
        let mut pacers = self.delete_pacers.lock().await;
        pacers
            .entry(channel_id.to_string())
            .or_insert_with(|| DeletePacer::new(initial_delete_delay_ms(aggressive)))
            .on_success();
    }

    async fn record_delete_rate_limit(
        &self,
        channel_id: &str,
        aggressive: bool,
        retry_after_secs: f64,
    ) {
        let mut pacers = self.delete_pacers.lock().await;
        pacers
            .entry(channel_id.to_string())
            .or_insert_with(|| DeletePacer::new(initial_delete_delay_ms(aggressive)))
            .on_rate_limit(retry_after_secs);
    }

    async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut rate_limit_retries = 0;
        loop {
            let response = self.get(path).await?;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                rate_limit_retries += 1;
                let retry_after = parse_retry_after(response, rate_limit_retries).await;
                let wait = self.scaled_wait(retry_after);
                terminal::record_rate_limit(wait);
                if rate_limit_retries >= MAX_RATE_LIMIT_RETRIES {
                    return Err(anyhow!("rate limit retries exceeded for {}", path));
                }
                tokio::time::sleep(StdDuration::from_secs_f64(wait)).await;
                continue;
            }

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!("request to {} failed: {} {}", path, status, body));
            }

            return response
                .json::<T>()
                .await
                .with_context(|| format!("failed to parse response from {path}"));
        }
    }

    async fn post_json<T>(&self, path: &str, body: &Value) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut rate_limit_retries = 0;
        loop {
            let response = self.post(path, body).await?;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                rate_limit_retries += 1;
                let retry_after = parse_retry_after(response, rate_limit_retries).await;
                let wait = self.scaled_wait(retry_after);
                terminal::record_rate_limit(wait);
                if rate_limit_retries >= MAX_RATE_LIMIT_RETRIES {
                    return Err(anyhow!("rate limit retries exceeded for {}", path));
                }
                tokio::time::sleep(StdDuration::from_secs_f64(wait)).await;
                continue;
            }

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!("request to {} failed: {} {}", path, status, body));
            }

            return response
                .json::<T>()
                .await
                .with_context(|| format!("failed to parse response from {path}"));
        }
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response> {
        let _permit = self
            .request_limit
            .acquire()
            .await
            .context("request semaphore closed")?;
        self.pick_client()
            .get(format!("{API_BASE}{path}"))
            .send()
            .await
            .with_context(|| format!("GET request failed for {path}"))
    }

    async fn post(&self, path: &str, body: &Value) -> Result<reqwest::Response> {
        let _permit = self
            .request_limit
            .acquire()
            .await
            .context("request semaphore closed")?;
        self.pick_client()
            .post(format!("{API_BASE}{path}"))
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST request failed for {path}"))
    }

    async fn delete(&self, path: &str) -> Result<reqwest::Response> {
        let _permit = self
            .request_limit
            .acquire()
            .await
            .context("request semaphore closed")?;
        self.pick_client()
            .delete(format!("{API_BASE}{path}"))
            .send()
            .await
            .with_context(|| format!("DELETE request failed for {path}"))
    }
}

async fn parse_retry_after(response: reqwest::Response, retry_count: usize) -> f64 {
    let header_retry = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<f64>().ok());
    let reset_after = response
        .headers()
        .get("x-ratelimit-reset-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<f64>().ok());

    let body_retry = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|value| value.get("retry_after").and_then(|value| value.as_f64()));

    let fallback = (retry_count as f64).min(5.0) * 0.5;
    body_retry
        .or(header_retry)
        .or(reset_after)
        .filter(|value| *value > 0.0)
        .unwrap_or(fallback.max(0.5))
}

fn update_oldest_cursor(cursor: &mut Option<String>, message_id: &str) {
    match cursor {
        Some(current) if current.as_str() <= message_id => {}
        _ => *cursor = Some(message_id.to_string()),
    }
}

fn format_http_error(status: StatusCode, body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        status.to_string()
    } else {
        format!("{status} {trimmed}")
    }
}

fn transient_delete_backoff_secs(attempt: usize) -> f64 {
    let factor = 2_f64.powi((attempt.saturating_sub(1)).min(4) as i32);
    (TRANSIENT_DELETE_BACKOFF_BASE_SECS * factor).min(TRANSIENT_DELETE_BACKOFF_MAX_SECS)
}

fn is_retryable_delete_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_EARLY
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    ) || status.is_server_error()
}

fn log_delete_retry(context: DeleteRetryContext<'_>, reason: &str) {
    terminal::warn(
        "kcordclient::discord",
        format!(
            "delete retry {}/{} for {} ({}) on {} after {}; waiting {:.1}s",
            context.attempt,
            context.max_attempts,
            context.channel_id,
            context.display_name,
            context.message_id,
            reason,
            context.wait_secs
        ),
    );

    if let Some(progress) = context.progress {
        set_delete_progress_message(
            progress,
            context.display_name,
            &format!(
                "Retry {}/{} in {:.1}s",
                context.attempt, context.max_attempts, context.wait_secs
            ),
        );
    }
}

fn set_delete_progress_message(progress: &ProgressBar, display_name: &str, phase: &str) {
    let completed = progress.position();
    let total = progress.length().unwrap_or(0);
    let remaining = total.saturating_sub(completed);
    progress.set_message(format!(
        "{phase} {display_name} | {completed}/{total} done | {remaining} left"
    ));
}
