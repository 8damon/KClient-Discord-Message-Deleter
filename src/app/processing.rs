use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::task::JoinSet;

use crate::{
    checkpoint, discord::DiscordClient, models::OwnedMessage, report::RunReport, state, terminal,
};

use super::progress::{
    configure_deletion_bar, create_spinner_bar, ProgressTracker, ScanEtaTracker,
};

pub(crate) struct ServerProcessContext {
    pub(crate) discord: Arc<DiscordClient>,
    pub(crate) guild_id: String,
    pub(crate) guild_name: String,
    pub(crate) my_id: String,
    pub(crate) channels: Vec<(String, String)>,
    pub(crate) cutoff: Option<DateTime<Utc>>,
    pub(crate) concurrency: usize,
    pub(crate) report: Arc<RunReport>,
}

pub(crate) async fn process_server_channels(context: ServerProcessContext) -> Result<()> {
    terminal::info(
        "kcordclient::app",
        format!("collecting authored messages across {}", context.guild_name),
    );

    let scan_progress_root = Arc::new(MultiProgress::new());
    terminal::set_progress_root(Some(scan_progress_root.clone()));
    let scan_progress = scan_progress_root.add(ProgressBar::new_spinner());
    let scan_style = ProgressStyle::with_template("  {spinner:.cyan} {msg}")
        .expect("valid scan progress template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
    scan_progress.set_style(scan_style);
    scan_progress.enable_steady_tick(std::time::Duration::from_millis(120));
    let mut eta_tracker = ScanEtaTracker::new();

    let grouped = context
        .discord
        .search_messages_in_guild(
            &context.guild_id,
            context.cutoff,
            &context.my_id,
            |progress| {
                let (display_total, remaining, eta) =
                    eta_tracker.estimate(progress.collected_results, progress.total_results);

                scan_progress.set_message(format!(
                    "Scanning {}  |  hits {} / {} total  |  left {}  |  ETA {}",
                    context.guild_name, progress.collected_results, display_total, remaining, eta
                ));
            },
        )
        .await
        .with_context(|| format!("failed searching messages across {}", context.guild_name))?;
    scan_progress.finish_and_clear();
    terminal::set_progress_root(None);

    let name_map = context
        .channels
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    let filtered_channels = grouped
        .into_iter()
        .map(|(channel_id, messages)| {
            let display_name = name_map
                .get(&channel_id)
                .cloned()
                .unwrap_or_else(|| format!("#{} in {}", channel_id, context.guild_name));
            (channel_id, display_name, messages)
        })
        .collect::<Vec<_>>();

    terminal::info(
        "kcordclient::app",
        format!(
            "guild search found messages in {} channels",
            filtered_channels.len()
        ),
    );

    process_prefetched_channels(
        context.discord,
        &context.my_id,
        filtered_channels,
        context.cutoff,
        context.concurrency,
        context.report,
    )
    .await
}

pub(crate) async fn process_channels(
    discord: Arc<DiscordClient>,
    my_id: &str,
    channels: Vec<(String, String)>,
    cutoff: Option<DateTime<Utc>>,
    concurrency: usize,
    aggressive_delete: bool,
    report: Arc<RunReport>,
) -> Result<()> {
    let progress_root = Arc::new(MultiProgress::new());
    terminal::set_progress_root(Some(progress_root.clone()));
    let tracker = ProgressTracker::new(&progress_root);
    let mut pending = channels.into_iter();
    let mut join_set = JoinSet::new();
    let worker_limit = concurrency.max(1);
    let mut failures = 0usize;
    let my_id = my_id.to_string();

    for _ in 0..worker_limit {
        if let Some((channel_id, display_name)) = pending.next() {
            spawn_channel_task(
                &mut join_set,
                ChannelTask {
                    discord: discord.clone(),
                    my_id: my_id.clone(),
                    channel_id,
                    display_name,
                    cutoff,
                    aggressive_delete,
                    prefer_history_scan: aggressive_delete,
                    progress_root: progress_root.clone(),
                    report: report.clone(),
                    tracker: tracker.clone(),
                },
            );
        }
    }

    while let Some(join_result) = join_set.join_next().await {
        match join_result.context("channel worker task failed")? {
            Ok(()) => {}
            Err(error) => {
                failures += 1;
                terminal::warn("kcordclient::app", format!("{:#}", error));
            }
        }

        if let Some((channel_id, display_name)) = pending.next() {
            spawn_channel_task(
                &mut join_set,
                ChannelTask {
                    discord: discord.clone(),
                    my_id: my_id.clone(),
                    channel_id,
                    display_name,
                    cutoff,
                    aggressive_delete,
                    prefer_history_scan: aggressive_delete,
                    progress_root: progress_root.clone(),
                    report: report.clone(),
                    tracker: tracker.clone(),
                },
            );
        }
    }

    if failures > 0 {
        terminal::warn(
            "kcordclient::app",
            format!("completed with {} channel failures", failures),
        );
    }

    tracker.finish();
    Ok(())
}

async fn process_prefetched_channels(
    discord: Arc<DiscordClient>,
    my_id: &str,
    channels: Vec<(String, String, Vec<OwnedMessage>)>,
    cutoff: Option<DateTime<Utc>>,
    concurrency: usize,
    report: Arc<RunReport>,
) -> Result<()> {
    let progress_root = Arc::new(MultiProgress::new());
    terminal::set_progress_root(Some(progress_root.clone()));
    let tracker = ProgressTracker::new(&progress_root);
    let mut pending = channels.into_iter();
    let mut join_set = JoinSet::new();
    let worker_limit = concurrency.max(1);
    let mut failures = 0usize;

    for _ in 0..worker_limit {
        if let Some((channel_id, display_name, messages)) = pending.next() {
            spawn_prefetched_channel_task(
                &mut join_set,
                PrefetchedChannelTask {
                    discord: discord.clone(),
                    my_id: my_id.to_string(),
                    channel_id,
                    display_name,
                    initial_messages: messages,
                    cutoff,
                    progress_root: progress_root.clone(),
                    report: report.clone(),
                    tracker: tracker.clone(),
                },
            );
        }
    }

    while let Some(join_result) = join_set.join_next().await {
        match join_result.context("channel worker task failed")? {
            Ok(()) => {}
            Err(error) => {
                failures += 1;
                terminal::warn("kcordclient::app", format!("{:#}", error));
            }
        }

        if let Some((channel_id, display_name, messages)) = pending.next() {
            spawn_prefetched_channel_task(
                &mut join_set,
                PrefetchedChannelTask {
                    discord: discord.clone(),
                    my_id: my_id.to_string(),
                    channel_id,
                    display_name,
                    initial_messages: messages,
                    cutoff,
                    progress_root: progress_root.clone(),
                    report: report.clone(),
                    tracker: tracker.clone(),
                },
            );
        }
    }

    if failures > 0 {
        terminal::warn(
            "kcordclient::app",
            format!("completed with {} channel failures", failures),
        );
    }

    tracker.finish();
    Ok(())
}

struct ChannelTask {
    discord: Arc<DiscordClient>,
    my_id: String,
    channel_id: String,
    display_name: String,
    cutoff: Option<DateTime<Utc>>,
    aggressive_delete: bool,
    prefer_history_scan: bool,
    progress_root: Arc<MultiProgress>,
    report: Arc<RunReport>,
    tracker: Arc<ProgressTracker>,
}

struct PrefetchedChannelTask {
    discord: Arc<DiscordClient>,
    my_id: String,
    channel_id: String,
    display_name: String,
    initial_messages: Vec<OwnedMessage>,
    cutoff: Option<DateTime<Utc>>,
    progress_root: Arc<MultiProgress>,
    report: Arc<RunReport>,
    tracker: Arc<ProgressTracker>,
}

fn spawn_channel_task(join_set: &mut JoinSet<Result<()>>, task: ChannelTask) {
    join_set.spawn(async move {
        let channel_id = task.channel_id.clone();
        let report = task.report.clone();
        let tracker = task.tracker.clone();
        let result = process_channel(task).await;

        if let Err(error) = &result {
            report.record_error(&channel_id, &format!("{:#}", error));
            state::set_error(&format!("{:#}", error));
            tracker.channel_finished();
        }

        result
    });
}

fn spawn_prefetched_channel_task(join_set: &mut JoinSet<Result<()>>, task: PrefetchedChannelTask) {
    join_set.spawn(async move {
        let channel_id = task.channel_id.clone();
        let report = task.report.clone();
        let tracker = task.tracker.clone();
        let result = process_prefetched_channel(task).await;

        if let Err(error) = &result {
            report.record_error(&channel_id, &format!("{:#}", error));
            state::set_error(&format!("{:#}", error));
            tracker.channel_finished();
        }

        result
    });
}

async fn process_channel(task: ChannelTask) -> Result<()> {
    let channel_id = task.channel_id.as_str();
    let display_name = task.display_name.as_str();
    let ck_key = checkpoint::cutoff_key(task.cutoff);
    let report = task.report.clone();
    let tracker = task.tracker.clone();
    report.start_channel(channel_id, display_name);
    tracker.channel_started();
    let progress = create_spinner_bar(&task.progress_root, display_name);

    let mut checkpoint = if let Some(existing) = checkpoint::load(channel_id, &ck_key) {
        terminal::info(
            "kcordclient::app",
            format!(
                "Resuming {} from checkpoint ({} found, {} already deleted)",
                display_name,
                existing.found.len(),
                existing.deleted_ids.len(),
            ),
        );
        existing
    } else {
        progress.set_message(format!("Scanning {}", display_name));
        let fetch = task
            .discord
            .fetch_messages_in_timeframe(
                channel_id,
                task.cutoff,
                &task.my_id,
                task.prefer_history_scan,
            )
            .await
            .with_context(|| format!("failed collecting messages for {}", display_name))?;
        report.record_scan(
            channel_id,
            "initial",
            fetch.method,
            task.cutoff,
            &fetch.messages,
        );
        terminal::info(
            "kcordclient::app",
            format!(
                "Collected {} own messages in {}",
                fetch.messages.len(),
                display_name
            ),
        );
        let checkpoint = checkpoint::ChannelCheckpoint::new(channel_id, &ck_key, fetch.messages);
        checkpoint
            .save()
            .with_context(|| format!("failed to save checkpoint for {}", display_name))?;
        checkpoint
    };

    let mut to_delete = checkpoint.pending();
    tracker.add_discovered(to_delete.len());
    tracker.fetch_finished();

    if to_delete.is_empty() {
        checkpoint.remove();
        report.finish_channel(channel_id, "no-messages");
        progress.finish_with_message(format!("No messages in {}", display_name));
        tracker.channel_finished();
        return Ok(());
    }

    to_delete.sort_by_key(|message| message.timestamp);
    configure_deletion_bar(&progress, to_delete.len() as u64, display_name, false);
    let mut deleted_first_pass = Vec::new();

    for chunk in to_delete.chunks(checkpoint::SAVE_INTERVAL) {
        let batch = task
            .discord
            .delete_messages(channel_id, chunk, Some(&progress), task.aggressive_delete)
            .await
            .with_context(|| format!("failed deleting messages in {}", display_name))?;

        for message in &batch.deleted {
            checkpoint.mark_deleted(&message.id);
        }
        for skipped in &batch.skipped {
            checkpoint.mark_deleted(&skipped.message.id);
            report.record_skip(
                channel_id,
                &skipped.message,
                skipped.status,
                &skipped.reason,
            );
        }

        checkpoint
            .save()
            .with_context(|| format!("failed to flush checkpoint for {}", display_name))?;
        tracker.add_deleted(batch.deleted.len() + batch.skipped.len());
        deleted_first_pass.extend(batch.deleted);
    }

    report.record_deletions(channel_id, "first-pass", &deleted_first_pass);
    checkpoint.remove();
    report.finish_channel(channel_id, "completed");
    progress.finish_with_message(format!("Completed {}", display_name));
    tracker.channel_finished();
    Ok(())
}

async fn process_prefetched_channel(task: PrefetchedChannelTask) -> Result<()> {
    let channel_id = task.channel_id.as_str();
    let display_name = task.display_name.as_str();
    let ck_key = checkpoint::cutoff_key(task.cutoff);
    let report = task.report.clone();
    report.start_channel(channel_id, display_name);
    task.tracker.channel_started();
    let progress = create_spinner_bar(&task.progress_root, display_name);

    let mut checkpoint = if let Some(existing) = checkpoint::load(channel_id, &ck_key) {
        terminal::info(
            "kcordclient::app",
            format!(
                "Resuming {} from checkpoint ({} found, {} already deleted)",
                display_name,
                existing.found.len(),
                existing.deleted_ids.len(),
            ),
        );
        existing
    } else {
        report.record_scan(
            channel_id,
            "initial-discovery",
            "guild-search",
            task.cutoff,
            &task.initial_messages,
        );
        let messages = if task.cutoff.is_some() {
            let history = task
                .discord
                .backfill_channel_history(channel_id, task.cutoff, &task.my_id)
                .await
                .with_context(|| format!("failed history backfill for {}", display_name))?;
            let merged = merge_owned_messages(task.initial_messages, history);
            report.record_scan(
                channel_id,
                "initial",
                "guild-search+history-scan",
                task.cutoff,
                &merged,
            );
            merged
        } else {
            report.record_scan(
                channel_id,
                "initial",
                "guild-search",
                task.cutoff,
                &task.initial_messages,
            );
            task.initial_messages
        };

        terminal::info(
            "kcordclient::app",
            format!(
                "Collected {} own messages in {}",
                messages.len(),
                display_name
            ),
        );

        let checkpoint = checkpoint::ChannelCheckpoint::new(channel_id, &ck_key, messages);
        checkpoint
            .save()
            .with_context(|| format!("failed to save checkpoint for {}", display_name))?;
        checkpoint
    };

    let mut to_delete = checkpoint.pending();
    task.tracker.add_discovered(to_delete.len());
    task.tracker.fetch_finished();

    if to_delete.is_empty() {
        checkpoint.remove();
        report.finish_channel(channel_id, "no-messages");
        progress.finish_with_message(format!("No messages in {}", display_name));
        task.tracker.channel_finished();
        return Ok(());
    }

    to_delete.sort_by_key(|message| message.timestamp);
    configure_deletion_bar(&progress, to_delete.len() as u64, display_name, false);
    let mut deleted_first_pass = Vec::new();

    for chunk in to_delete.chunks(checkpoint::SAVE_INTERVAL) {
        let batch = task
            .discord
            .delete_messages(channel_id, chunk, Some(&progress), false)
            .await
            .with_context(|| format!("failed deleting messages in {}", display_name))?;

        for message in &batch.deleted {
            checkpoint.mark_deleted(&message.id);
        }
        for skipped in &batch.skipped {
            checkpoint.mark_deleted(&skipped.message.id);
            report.record_skip(
                channel_id,
                &skipped.message,
                skipped.status,
                &skipped.reason,
            );
        }

        checkpoint
            .save()
            .with_context(|| format!("failed to flush checkpoint for {}", display_name))?;
        task.tracker
            .add_deleted(batch.deleted.len() + batch.skipped.len());
        deleted_first_pass.extend(batch.deleted);
    }

    report.record_deletions(channel_id, "first-pass", &deleted_first_pass);
    checkpoint.remove();
    report.finish_channel(channel_id, "completed");
    progress.finish_with_message(format!("Completed {}", display_name));
    task.tracker.channel_finished();
    Ok(())
}

fn merge_owned_messages(first: Vec<OwnedMessage>, second: Vec<OwnedMessage>) -> Vec<OwnedMessage> {
    let mut seen = HashSet::new();
    let mut merged = Vec::with_capacity(first.len() + second.len());

    for message in first.into_iter().chain(second) {
        if seen.insert(message.id.clone()) {
            merged.push(message);
        }
    }

    merged
}
