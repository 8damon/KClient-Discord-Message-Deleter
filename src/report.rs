use std::{collections::BTreeMap, sync::Mutex};

use chrono::{DateTime, Local, Utc};

use crate::models::OwnedMessage;

pub struct RunReport {
    started_at: DateTime<Local>,
    target_name: String,
    timeframe_desc: String,
    channels: Mutex<BTreeMap<String, ChannelReport>>,
}

struct ChannelReport {
    channel_id: String,
    display_name: String,
    started_at: DateTime<Local>,
    finished_at: Option<DateTime<Local>>,
    status: String,
    scans: Vec<ScanRecord>,
    deletions: Vec<DeletionRecord>,
    skips: Vec<SkippedDeletionRecord>,
    errors: Vec<String>,
}

struct ScanRecord {
    phase: String,
    method: String,
    scanned_at: DateTime<Local>,
    message_count: usize,
    oldest_found: Option<DateTime<Utc>>,
    newest_found: Option<DateTime<Utc>>,
    cutoff: Option<DateTime<Utc>>,
}

struct DeletionRecord {
    pass: String,
    deleted_at: DateTime<Local>,
    message_id: String,
    message_timestamp: DateTime<Utc>,
    content: String,
}

struct SkippedDeletionRecord {
    skipped_at: DateTime<Local>,
    message_id: String,
    message_timestamp: DateTime<Utc>,
    status: u16,
    reason: String,
    content: String,
}

impl RunReport {
    pub fn new(target_name: String, timeframe_desc: String) -> Self {
        Self {
            started_at: Local::now(),
            target_name,
            timeframe_desc,
            channels: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn start_channel(&self, channel_id: &str, display_name: &str) {
        if let Ok(mut channels) = self.channels.lock() {
            channels
                .entry(channel_id.to_string())
                .or_insert(ChannelReport {
                    channel_id: channel_id.to_string(),
                    display_name: display_name.to_string(),
                    started_at: Local::now(),
                    finished_at: None,
                    status: "started".to_string(),
                    scans: Vec::new(),
                    deletions: Vec::new(),
                    skips: Vec::new(),
                    errors: Vec::new(),
                });
        }
    }

    pub fn record_scan(
        &self,
        channel_id: &str,
        phase: &str,
        method: &str,
        cutoff: Option<DateTime<Utc>>,
        messages: &[OwnedMessage],
    ) {
        if let Ok(mut channels) = self.channels.lock() {
            if let Some(channel) = channels.get_mut(channel_id) {
                channel.scans.push(ScanRecord {
                    phase: phase.to_string(),
                    method: method.to_string(),
                    scanned_at: Local::now(),
                    message_count: messages.len(),
                    oldest_found: messages.iter().map(|m| m.timestamp).min(),
                    newest_found: messages.iter().map(|m| m.timestamp).max(),
                    cutoff,
                });
            }
        }
    }

    pub fn record_deletions(&self, channel_id: &str, pass: &str, messages: &[OwnedMessage]) {
        if let Ok(mut channels) = self.channels.lock() {
            if let Some(channel) = channels.get_mut(channel_id) {
                let deleted_at = Local::now();
                channel
                    .deletions
                    .extend(messages.iter().map(|message| DeletionRecord {
                        pass: pass.to_string(),
                        deleted_at,
                        message_id: message.id.clone(),
                        message_timestamp: message.timestamp,
                        content: sanitize_log_content(&message.content),
                    }));
            }
        }
    }

    pub fn record_error(&self, channel_id: &str, error: &str) {
        if let Ok(mut channels) = self.channels.lock() {
            if let Some(channel) = channels.get_mut(channel_id) {
                channel.errors.push(error.to_string());
                channel.status = "failed".to_string();
                channel.finished_at = Some(Local::now());
            }
        }
    }

    pub fn record_skip(&self, channel_id: &str, message: &OwnedMessage, status: u16, reason: &str) {
        if let Ok(mut channels) = self.channels.lock() {
            if let Some(channel) = channels.get_mut(channel_id) {
                channel.skips.push(SkippedDeletionRecord {
                    skipped_at: Local::now(),
                    message_id: message.id.clone(),
                    message_timestamp: message.timestamp,
                    status,
                    reason: sanitize_log_content(reason),
                    content: sanitize_log_content(&message.content),
                });
            }
        }
    }

    pub fn finish_channel(&self, channel_id: &str, status: &str) {
        if let Ok(mut channels) = self.channels.lock() {
            if let Some(channel) = channels.get_mut(channel_id) {
                channel.status = status.to_string();
                channel.finished_at = Some(Local::now());
            }
        }
    }

    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str("=== kcordclient delete log ===\n");
        output.push_str(&format!("Run started: {}\n", self.started_at));
        output.push_str(&format!("Target: {}\n", self.target_name));
        output.push_str(&format!("Mode: {}\n\n", self.timeframe_desc));

        if let Ok(channels) = self.channels.lock() {
            output.push_str(&format!("Channels scanned: {}\n\n", channels.len()));

            for channel in channels.values() {
                output.push_str(&format!(
                    "--- {} ({}) ---\n",
                    channel.display_name, channel.channel_id
                ));
                output.push_str(&format!("Started: {}\n", channel.started_at));
                output.push_str(&format!(
                    "Finished: {}\n",
                    channel
                        .finished_at
                        .map(|time| time.to_string())
                        .unwrap_or_else(|| "n/a".to_string())
                ));
                output.push_str(&format!("Status: {}\n", channel.status));
                output.push_str(&format!("Scans performed: {}\n", channel.scans.len()));
                output.push_str(&format!("Messages deleted: {}\n", channel.deletions.len()));
                output.push_str(&format!("Messages skipped: {}\n", channel.skips.len()));

                if !channel.errors.is_empty() {
                    output.push_str("Errors:\n");
                    for error in &channel.errors {
                        output.push_str(&format!("  - {}\n", error));
                    }
                }

                if !channel.scans.is_empty() {
                    output.push_str("Scan details:\n");
                    for scan in &channel.scans {
                        output.push_str(&format!(
                            "  - phase={} method={} scanned_at={} found={} cutoff={} oldest_found={} newest_found={}\n",
                            scan.phase,
                            scan.method,
                            scan.scanned_at,
                            scan.message_count,
                            format_optional_utc(scan.cutoff),
                            format_optional_utc(scan.oldest_found),
                            format_optional_utc(scan.newest_found),
                        ));
                    }
                }

                if !channel.deletions.is_empty() {
                    output.push_str("Deleted messages:\n");
                    for deletion in &channel.deletions {
                        output.push_str(&format!(
                            "  - pass={} deleted_at={} message_id={} message_timestamp={} content={}\n",
                            deletion.pass,
                            deletion.deleted_at,
                            deletion.message_id,
                            deletion.message_timestamp,
                            deletion.content
                        ));
                    }
                }

                if !channel.skips.is_empty() {
                    output.push_str("Skipped messages:\n");
                    for skip in &channel.skips {
                        output.push_str(&format!(
                            "  - skipped_at={} message_id={} message_timestamp={} status={} reason={} content={}\n",
                            skip.skipped_at,
                            skip.message_id,
                            skip.message_timestamp,
                            skip.status,
                            skip.reason,
                            skip.content
                        ));
                    }
                }

                output.push('\n');
            }
        }

        output
    }
}

fn format_optional_utc(value: Option<DateTime<Utc>>) -> String {
    value
        .map(|timestamp| timestamp.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn sanitize_log_content(content: &str) -> String {
    let sanitized = content.replace(['\r', '\n'], " ");
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "<no text content>".to_string()
    } else {
        trimmed.chars().take(240).collect()
    }
}
