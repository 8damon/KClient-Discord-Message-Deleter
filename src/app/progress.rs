use std::{
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::{state, terminal};

pub(crate) struct ScanEtaTracker {
    started_at: Instant,
    max_total_seen: u64,
    last_collected: u64,
    last_update: Instant,
    smoothed_secs_per_hit: Option<f64>,
}

impl ScanEtaTracker {
    pub(crate) fn new() -> Self {
        let now = Instant::now();
        Self {
            started_at: now,
            max_total_seen: 0,
            last_collected: 0,
            last_update: now,
            smoothed_secs_per_hit: None,
        }
    }

    pub(crate) fn estimate(&mut self, collected: u64, reported_total: u64) -> (u64, u64, String) {
        self.max_total_seen = self.max_total_seen.max(reported_total).max(collected);
        let total = self.max_total_seen;
        let remaining = total.saturating_sub(collected);
        let now = Instant::now();

        if collected > self.last_collected {
            let delta_hits = collected - self.last_collected;
            let delta_secs = now.duration_since(self.last_update).as_secs_f64();
            if delta_secs > 0.0 {
                let sample = delta_secs / delta_hits as f64;
                self.smoothed_secs_per_hit = Some(match self.smoothed_secs_per_hit {
                    Some(previous) => previous * 0.7 + sample * 0.3,
                    None => sample,
                });
            }
            self.last_collected = collected;
            self.last_update = now;
        } else if self.smoothed_secs_per_hit.is_none() && collected > 0 {
            let elapsed = self.started_at.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                self.smoothed_secs_per_hit = Some(elapsed / collected as f64);
            }
        }

        let eta = self
            .smoothed_secs_per_hit
            .map(|secs_per_hit| format_duration((secs_per_hit * remaining as f64).round() as u64))
            .unwrap_or_else(|| String::from("--:--:--"));

        (total, remaining, eta)
    }
}

pub(crate) struct ProgressTracker {
    started_at: Instant,
    summary: ProgressBar,
    discovered_total: AtomicU64,
    deleted_total: AtomicU64,
    active_channels: AtomicUsize,
    fetching_channels: AtomicUsize,
}

impl ProgressTracker {
    pub(crate) fn new(progress_root: &MultiProgress) -> Arc<Self> {
        let summary = progress_root.add(ProgressBar::new_spinner());
        let style = ProgressStyle::with_template("\n\n  {spinner:.cyan} {msg}")
            .expect("valid summary template")
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
        summary.set_style(style);
        summary.enable_steady_tick(std::time::Duration::from_millis(80));

        let tracker = Arc::new(Self {
            started_at: Instant::now(),
            summary,
            discovered_total: AtomicU64::new(0),
            deleted_total: AtomicU64::new(0),
            active_channels: AtomicUsize::new(0),
            fetching_channels: AtomicUsize::new(0),
        });

        tracker.refresh();
        tracker
    }

    pub(crate) fn channel_started(&self) {
        self.active_channels.fetch_add(1, Ordering::Relaxed);
        self.fetching_channels.fetch_add(1, Ordering::Relaxed);
        self.refresh();
    }

    pub(crate) fn channel_finished(&self) {
        let previous_active = self.active_channels.fetch_sub(1, Ordering::Relaxed);
        let new_active = previous_active.saturating_sub(1);
        self.clamp_fetching_channels(new_active);
        self.refresh();
    }

    pub(crate) fn add_discovered(&self, count: usize) {
        self.discovered_total
            .fetch_add(count as u64, Ordering::Relaxed);
        self.refresh();
    }

    pub(crate) fn add_deleted(&self, count: usize) {
        self.deleted_total
            .fetch_add(count as u64, Ordering::Relaxed);
        self.refresh();
    }

    pub(crate) fn fetch_finished(&self) {
        let previous = self.fetching_channels.load(Ordering::Relaxed);
        if previous > 0 {
            self.fetching_channels.fetch_sub(1, Ordering::Relaxed);
        }
        if self.fetching_channels.load(Ordering::Relaxed) == 0 {
            terminal::set_rate_limit_graph_enabled(false);
        }
        self.refresh();
    }

    pub(crate) fn finish(&self) {
        self.refresh();
        self.summary.finish_and_clear();
    }

    fn refresh(&self) {
        let discovered = self.discovered_total.load(Ordering::Relaxed);
        let deleted = self.deleted_total.load(Ordering::Relaxed);
        let remaining = discovered.saturating_sub(deleted);
        let active = self.active_channels.load(Ordering::Relaxed);
        let fetching = self.fetching_channels.load(Ordering::Relaxed);
        let status = if fetching > 0 {
            format!("Scanning | Remaining {}", remaining)
        } else if deleted > 0 && discovered >= deleted {
            let secs_per_msg = self.started_at.elapsed().as_secs_f64() / deleted as f64;
            let rate = if secs_per_msg > 0.0 {
                1.0 / secs_per_msg
            } else {
                0.0
            };
            format!(
                "Delete ETA {} | {:.1} msg/s",
                format_duration((secs_per_msg * remaining as f64).round() as u64),
                rate
            )
        } else if discovered > 0 {
            String::from("Delete ETA pending")
        } else {
            String::from("ETA --:--:--")
        };

        self.summary.set_message(format!(
            "Found: {} | Deleted: {} | Remaining: {} | Active: {} | Fetching: {} | {}",
            discovered, deleted, remaining, active, fetching, status
        ));

        state::update_progress(
            discovered, deleted, remaining, active, fetching, &status, &status,
        );

        self.summary.tick();
    }

    fn clamp_fetching_channels(&self, max_fetching: usize) {
        loop {
            let current = self.fetching_channels.load(Ordering::Relaxed);
            if current <= max_fetching {
                break;
            }
            if self
                .fetching_channels
                .compare_exchange(current, max_fetching, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                if max_fetching == 0 {
                    terminal::set_rate_limit_graph_enabled(false);
                }
                break;
            }
        }
    }
}

pub(crate) fn create_spinner_bar(progress_root: &MultiProgress, display_name: &str) -> ProgressBar {
    let progress = progress_root.add(ProgressBar::new_spinner());
    let style =
        ProgressStyle::with_template("  {spinner:.cyan} {msg:<46.46}  {elapsed_precise:>10}")
            .expect("valid spinner template")
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
    progress.set_style(style);
    progress.set_message(format!("Collecting {}", display_name));
    progress.enable_steady_tick(std::time::Duration::from_millis(120));
    progress
}

pub(crate) fn configure_deletion_bar(
    progress: &ProgressBar,
    total: u64,
    display_name: &str,
    retry: bool,
) {
    let style = ProgressStyle::with_template(
        "  {msg:<34.34} [{wide_bar:.cyan/blue}] {pos:>4}/{len:<4} {percent:>3}%  eta {eta_precise}",
    )
    .expect("valid progress template")
    .progress_chars("=>-");
    progress.set_style(style);
    progress.set_length(total);
    progress.set_position(0);
    let prefix = if retry { "Retrying" } else { "Deleting" };
    progress.set_message(format!("{prefix} {}", display_name));
}

pub(crate) fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    format!("{hours:02}:{minutes:02}:{secs:02}")
}
