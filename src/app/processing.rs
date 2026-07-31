use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::task::JoinSet;

use crate::{
    args::WatchdogType,
    checkpoint,
    discord::{DeleteProgress, DiscordClient},
    models::OwnedMessage,
    report::RunReport,
    state, terminal,
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
    pub(crate) delete_type: Option<WatchdogType>,
    pub(crate) report: Arc<RunReport>,
}

pub(crate) struct ChannelProcessContext {
    pub(crate) discord: Arc<DiscordClient>,
    pub(crate) my_id: String,
    pub(crate) channels: Vec<(String, String)>,
    pub(crate) cutoff: Option<DateTime<Utc>>,
    pub(crate) concurrency: usize,
    pub(crate) aggressive_delete: bool,
    pub(crate) delete_type: Option<WatchdogType>,
    pub(crate) report: Arc<RunReport>,
}

const LIVE_CHECKPOINT_FLUSH_INTERVAL: usize = 25;

pub(crate) async fn process_server_channels(context: ServerProcessContext) -> Result<()> {
    terminal::info(
        "kcordclient::app",
        format!("collecting authored messages across {}", context.guild_name),
    );
    terminal::debug(
        "kcordclient::app",
        format!(
            "server delete config: concurrency={}, type={:?}",
            context.concurrency, context.delete_type
        ),
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

    let grouped = match context
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
        .with_context(|| format!("failed searching messages across {}", context.guild_name))
    {
        Ok(grouped) => grouped,
        Err(error) => {
            if error
                .to_string()
                .contains("guild search prefetch cap exceeded")
            {
                scan_progress.finish_and_clear();
                terminal::set_progress_root(None);
                terminal::warn(
                    "kcordclient::app",
                    format!(
                        "guild prefetch cap reached for {}; switching to channel-by-channel scan",
                        context.guild_name
                    ),
                );
                return process_channels(ChannelProcessContext {
                    discord: context.discord,
                    my_id: context.my_id,
                    channels: context.channels,
                    cutoff: context.cutoff,
                    concurrency: context.concurrency,
                    aggressive_delete: false,
                    delete_type: context.delete_type,
                    report: context.report,
                })
                .await;
            }
            return Err(error);
        }
    };
    terminal::verbose(
        1,
        "kcordclient::app",
        format!(
            "server {} search returned {} raw channel buckets",
            context.guild_name,
            grouped.len()
        ),
    );
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
            let filtered_messages = filter_messages_by_type(messages, context.delete_type);
            (channel_id, display_name, filtered_messages)
        })
        .filter(|(_, _, messages)| !messages.is_empty())
        .collect::<Vec<_>>();
    terminal::verbose(
        1,
        "kcordclient::app",
        format!(
            "server {} has {} channels eligible after filtering",
            context.guild_name,
            filtered_channels.len()
        ),
    );

    terminal::info(
        "kcordclient::app",
        format!(
            "guild search found messages in {} channels",
            filtered_channels.len()
        ),
    );
    terminal::verbose(
        2,
        "kcordclient::app",
        format!(
            "server {} has {} channels eligible after delete-type filtering",
            context.guild_name,
            filtered_channels.len()
        ),
    );

    process_prefetched_channels(
        context.discord,
        &context.my_id,
        filtered_channels,
        context.cutoff,
        context.concurrency,
        context.delete_type,
        context.report,
    )
    .await
}

pub(crate) async fn process_channels(context: ChannelProcessContext) -> Result<()> {
    terminal::verbose(
        1,
        "kcordclient::app",
        format!(
            "processing {} channels with concurrency {}",
            context.channels.len(),
            context.concurrency
        ),
    );
    let worker_limit = context.concurrency.max(1);
    terminal::verbose(
        1,
        "kcordclient::app",
        format!(
            "creating up to {} worker slots for {} channels",
            worker_limit,
            context.channels.len()
        ),
    );
    let progress_root = Arc::new(MultiProgress::new());
    terminal::set_progress_root(Some(progress_root.clone()));
    let tracker = ProgressTracker::new(&progress_root);
    let mut pending = context.channels.into_iter();
    let mut join_set = JoinSet::new();
    let mut failures = 0usize;
    let my_id = context.my_id;

    for _ in 0..worker_limit {
        if let Some((channel_id, display_name)) = pending.next() {
            terminal::verbose(
                2,
                "kcordclient::app",
                format!("spawning initial worker for {}", display_name),
            );
            spawn_channel_task(
                &mut join_set,
                ChannelTask {
                    discord: context.discord.clone(),
                    my_id: my_id.clone(),
                    channel_id,
                    display_name,
                    cutoff: context.cutoff,
                    aggressive_delete: context.aggressive_delete,
                    prefer_history_scan: context.aggressive_delete,
                    progress_root: progress_root.clone(),
                    report: context.report.clone(),
                    tracker: tracker.clone(),
                    delete_type: context.delete_type,
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
            terminal::verbose(
                3,
                "kcordclient::app",
                format!("reusing worker slot for {}", display_name),
            );
            spawn_channel_task(
                &mut join_set,
                ChannelTask {
                    discord: context.discord.clone(),
                    my_id: my_id.clone(),
                    channel_id,
                    display_name,
                    cutoff: context.cutoff,
                    aggressive_delete: context.aggressive_delete,
                    prefer_history_scan: context.aggressive_delete,
                    progress_root: progress_root.clone(),
                    report: context.report.clone(),
                    tracker: tracker.clone(),
                    delete_type: context.delete_type,
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
    delete_type: Option<WatchdogType>,
    report: Arc<RunReport>,
) -> Result<()> {
    let progress_root = Arc::new(MultiProgress::new());
    terminal::set_progress_root(Some(progress_root.clone()));
    let tracker = ProgressTracker::new(&progress_root);
    let mut pending = channels.into_iter();
    let mut join_set = JoinSet::new();
    let worker_limit = concurrency.max(1);
    terminal::verbose(
        1,
        "kcordclient::app",
        format!(
            "creating up to {} worker slots for {} prefetched channels",
            worker_limit,
            pending.len()
        ),
    );
    let mut failures = 0usize;

    for _ in 0..worker_limit {
        if let Some((channel_id, display_name, messages)) = pending.next() {
            terminal::verbose(
                2,
                "kcordclient::app",
                format!("spawning prefetched worker for {}", display_name),
            );
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
                    delete_type,
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
            terminal::verbose(
                3,
                "kcordclient::app",
                format!(
                    "reusing worker slot for prefetched channel {}",
                    display_name
                ),
            );
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
                    delete_type,
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
    delete_type: Option<WatchdogType>,
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
    delete_type: Option<WatchdogType>,
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

    let mut checkpoint = if let Some(mut existing) = checkpoint::load(channel_id, &ck_key) {
        existing.found = filter_messages_by_type(existing.found, task.delete_type);
        terminal::verbose(
            1,
            "kcordclient::app",
            format!("using existing checkpoint for {}", display_name),
        );
        terminal::info(
            "kcordclient::app",
            format!(
                "Resuming {} from checkpoint ({} found, {} already deleted)",
                display_name,
                existing.found.len(),
                existing.deleted_ids.len(),
            ),
        );
        terminal::debug(
            "kcordclient::app",
            format!(
                "loaded checkpoint for {} with {} pending after filtering",
                display_name,
                existing.pending().len()
            ),
        );
        existing
    } else {
        progress.set_message(format!("Scanning {}", display_name));
        terminal::verbose(
            1,
            "kcordclient::app",
            format!("collecting fresh messages for {}", display_name),
        );
        let mut fetch = task
            .discord
            .fetch_messages_in_timeframe(
                channel_id,
                task.cutoff,
                &task.my_id,
                task.prefer_history_scan,
            )
            .await
            .with_context(|| format!("failed collecting messages for {}", display_name))?;
        fetch.messages = filter_messages_by_type(fetch.messages, task.delete_type);
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
        terminal::debug(
            "kcordclient::app",
            format!(
                "initial fetch returned {} candidate messages in {}",
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
    terminal::verbose(
        2,
        "kcordclient::app",
        format!(
            "checkpoint {} has {} pending messages ready to delete",
            display_name,
            to_delete.len()
        ),
    );
    terminal::verbose(
        2,
        "kcordclient::app",
        format!(
            "deleting {} messages from {} in chunks of {}",
            to_delete.len(),
            display_name,
            checkpoint::SAVE_INTERVAL
        ),
    );
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
    let mut unsaved_progress = 0usize;

    let total_chunks = to_delete.len().div_ceil(checkpoint::SAVE_INTERVAL);
    for (chunk_index, chunk) in to_delete.chunks(checkpoint::SAVE_INTERVAL).enumerate() {
        terminal::verbose(
            3,
            "kcordclient::app",
            format!(
                "delete chunk {}/{} for {} with {} messages",
                chunk_index + 1,
                total_chunks.max(1),
                display_name,
                chunk.len()
            ),
        );
        let delete_result = task
            .discord
            .delete_messages(
                channel_id,
                display_name,
                chunk,
                Some(&progress),
                task.aggressive_delete,
                |event| {
                    match event {
                        DeleteProgress::Deleted(message) => {
                            checkpoint.mark_deleted(&message.id);
                            report.record_deletions(channel_id, "first-pass", &[message]);
                        }
                        DeleteProgress::Skipped(skipped) => {
                            checkpoint.mark_deleted(&skipped.message.id);
                            report.record_skip(
                                channel_id,
                                &skipped.message,
                                skipped.status,
                                &skipped.reason,
                            );
                        }
                    }

                    tracker.add_deleted(1);
                    unsaved_progress += 1;
                    if unsaved_progress >= LIVE_CHECKPOINT_FLUSH_INTERVAL {
                        checkpoint.save()?;
                        unsaved_progress = 0;
                    }
                    Ok(())
                },
            )
            .await;

        if let Err(error) = delete_result {
            checkpoint
                .save()
                .with_context(|| format!("failed to flush checkpoint for {}", display_name))?;
            return Err(error)
                .with_context(|| format!("failed deleting messages in {}", display_name));
        }

        checkpoint
            .save()
            .with_context(|| format!("failed to flush checkpoint for {}", display_name))?;
        unsaved_progress = 0;
    }

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

    let mut checkpoint = if let Some(mut existing) = checkpoint::load(channel_id, &ck_key) {
        existing.found = filter_messages_by_type(existing.found, task.delete_type);
        terminal::verbose(
            1,
            "kcordclient::app",
            format!("using existing checkpoint for prefetched {}", display_name),
        );
        terminal::info(
            "kcordclient::app",
            format!(
                "Resuming {} from checkpoint ({} found, {} already deleted)",
                display_name,
                existing.found.len(),
                existing.deleted_ids.len(),
            ),
        );
        terminal::debug(
            "kcordclient::app",
            format!(
                "loaded prefetch checkpoint for {} with {} pending after filtering",
                display_name,
                existing.pending().len()
            ),
        );
        existing
    } else {
        let initial_messages = filter_messages_by_type(task.initial_messages, task.delete_type);
        report.record_scan(
            channel_id,
            "initial-discovery",
            "guild-search",
            task.cutoff,
            &initial_messages,
        );
        let messages = if task.cutoff.is_some() {
            let history = task
                .discord
                .backfill_channel_history(channel_id, task.cutoff, &task.my_id)
                .await
                .with_context(|| format!("failed history backfill for {}", display_name))?;
            let merged = merge_owned_messages(initial_messages, history);
            let filtered = filter_messages_by_type(merged, task.delete_type);
            report.record_scan(
                channel_id,
                "initial",
                "guild-search+history-scan",
                task.cutoff,
                &filtered,
            );
            filtered
        } else {
            report.record_scan(
                channel_id,
                "initial",
                "guild-search",
                task.cutoff,
                &initial_messages,
            );
            initial_messages
        };

        terminal::info(
            "kcordclient::app",
            format!(
                "Collected {} own messages in {}",
                messages.len(),
                display_name
            ),
        );
        terminal::debug(
            "kcordclient::app",
            format!(
                "using {} messages for deletion workflow in {}",
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
    terminal::verbose(
        2,
        "kcordclient::app",
        format!(
            "prefetched checkpoint {} has {} pending messages ready to delete",
            display_name,
            to_delete.len()
        ),
    );
    terminal::verbose(
        2,
        "kcordclient::app",
        format!(
            "deleting {} prefetched messages from {} in chunks of {}",
            to_delete.len(),
            display_name,
            checkpoint::SAVE_INTERVAL
        ),
    );
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
    let mut unsaved_progress = 0usize;

    let total_chunks = to_delete.len().div_ceil(checkpoint::SAVE_INTERVAL);
    for (chunk_index, chunk) in to_delete.chunks(checkpoint::SAVE_INTERVAL).enumerate() {
        terminal::verbose(
            3,
            "kcordclient::app",
            format!(
                "delete prefetched chunk {}/{} for {} with {} messages",
                chunk_index + 1,
                total_chunks.max(1),
                display_name,
                chunk.len()
            ),
        );
        let delete_result = task
            .discord
            .delete_messages(
                channel_id,
                display_name,
                chunk,
                Some(&progress),
                false,
                |event| {
                    match event {
                        DeleteProgress::Deleted(message) => {
                            checkpoint.mark_deleted(&message.id);
                            report.record_deletions(channel_id, "first-pass", &[message]);
                        }
                        DeleteProgress::Skipped(skipped) => {
                            checkpoint.mark_deleted(&skipped.message.id);
                            report.record_skip(
                                channel_id,
                                &skipped.message,
                                skipped.status,
                                &skipped.reason,
                            );
                        }
                    }

                    task.tracker.add_deleted(1);
                    unsaved_progress += 1;
                    if unsaved_progress >= LIVE_CHECKPOINT_FLUSH_INTERVAL {
                        checkpoint.save()?;
                        unsaved_progress = 0;
                    }
                    Ok(())
                },
            )
            .await;

        if let Err(error) = delete_result {
            checkpoint
                .save()
                .with_context(|| format!("failed to flush checkpoint for {}", display_name))?;
            return Err(error)
                .with_context(|| format!("failed deleting messages in {}", display_name));
        }

        checkpoint
            .save()
            .with_context(|| format!("failed to flush checkpoint for {}", display_name))?;
        unsaved_progress = 0;
    }

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

fn filter_messages_by_type(
    messages: Vec<OwnedMessage>,
    delete_type: Option<WatchdogType>,
) -> Vec<OwnedMessage> {
    let Some(delete_type) = delete_type else {
        return messages;
    };

    let input_count = messages.len();
    let matched = messages
        .into_iter()
        .filter(|message| message_matches_delete_type(message, delete_type))
        .collect::<Vec<_>>();
    terminal::verbose(
        3,
        "kcordclient::app",
        format!(
            "applied delete-type {:?} filter against {} messages",
            delete_type, input_count
        ),
    );
    terminal::debug(
        "kcordclient::app",
        format!(
            "delete-type {:?} filtered {} -> {} messages",
            delete_type,
            input_count,
            matched.len()
        ),
    );
    matched
}

fn message_matches_delete_type(message: &OwnedMessage, delete_type: WatchdogType) -> bool {
    match delete_type {
        WatchdogType::Message => !message.has_media,
        WatchdogType::Media => message.has_media,
        WatchdogType::Links => message.has_link,
        WatchdogType::Files => message.has_file,
        WatchdogType::Videos => message.has_video,
    }
}
