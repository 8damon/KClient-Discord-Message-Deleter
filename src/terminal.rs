use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

use chrono::Local;
use console::{Style, Term};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::state;

static PROGRESS_ROOT: OnceLock<Mutex<Option<Arc<MultiProgress>>>> = OnceLock::new();
static RATE_LIMIT_GRAPH: OnceLock<Mutex<RateLimitGraph>> = OnceLock::new();

struct RateLimitGraph {
    bar: Option<ProgressBar>,
    waits: VecDeque<f64>,
    total_hits: u64,
    max_wait: f64,
    last_wait: f64,
    enabled: bool,
}

impl RateLimitGraph {
    fn new() -> Self {
        Self {
            bar: None,
            waits: VecDeque::new(),
            total_hits: 0,
            max_wait: 0.0,
            last_wait: 0.0,
            enabled: true,
        }
    }

    fn attach(&mut self, progress_root: &Arc<MultiProgress>) {
        if !self.enabled {
            return;
        }
        let bar = progress_root.add(ProgressBar::new_spinner());
        let style = ProgressStyle::with_template("  {spinner:.yellow} {msg}")
            .expect("valid rate-limit graph template")
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
        bar.set_style(style);
        bar.enable_steady_tick(std::time::Duration::from_millis(160));
        bar.set_message(render_placeholder_graph());
        self.bar = Some(bar);
    }

    fn detach(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
    }

    fn push_wait(&mut self, wait_secs: f64) {
        if !self.enabled {
            return;
        }
        self.total_hits += 1;
        self.last_wait = wait_secs;
        self.max_wait = self.max_wait.max(wait_secs);

        let graph_width = graph_width();
        self.waits.push_back(wait_secs);
        while self.waits.len() > graph_width {
            self.waits.pop_front();
        }

        if let Some(bar) = &self.bar {
            bar.set_message(self.render_message(graph_width));
            bar.tick();
        }
    }

    fn render_message(&self, graph_width: usize) -> String {
        let avg_wait = if self.waits.is_empty() {
            0.0
        } else {
            self.waits.iter().copied().sum::<f64>() / self.waits.len() as f64
        };
        format!(
            "Rate Limits  hits {:>4}  avg {:>4.1}s  max {:>4.1}s  last {:>4.1}s  {}",
            self.total_hits,
            avg_wait,
            self.max_wait,
            self.last_wait,
            render_sparkline(&self.waits, graph_width),
        )
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.detach();
        }
    }
}

pub fn set_progress_root(progress_root: Option<Arc<MultiProgress>>) {
    let slot = PROGRESS_ROOT.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        if guard.is_none() && progress_root.is_some() {
            println!();
            println!();
        }
        if let Ok(mut graph) = RATE_LIMIT_GRAPH
            .get_or_init(|| Mutex::new(RateLimitGraph::new()))
            .lock()
        {
            graph.detach();
            if let Some(root) = progress_root.as_ref() {
                graph.attach(root);
            }
        }
        *guard = progress_root;
    }
}

pub fn info(target: &str, message: impl AsRef<str>) {
    emit("INFO", Style::new().green(), target, message.as_ref());
}

pub fn warn(target: &str, message: impl AsRef<str>) {
    emit("WARN", Style::new().yellow(), target, message.as_ref());
}

pub fn plain(message: impl AsRef<str>) {
    let line = message.as_ref().to_string();
    state::push_log(&line);
    if !print_via_progress(&line) {
        println!("{}", line);
    }
}

pub fn record_rate_limit(wait_secs: f64) {
    state::push_rate_limit(wait_secs);
    let slot = RATE_LIMIT_GRAPH.get_or_init(|| Mutex::new(RateLimitGraph::new()));
    if let Ok(mut graph) = slot.lock() {
        graph.push_wait(wait_secs);
    }
}

pub fn set_rate_limit_graph_enabled(enabled: bool) {
    let graph_slot = RATE_LIMIT_GRAPH.get_or_init(|| Mutex::new(RateLimitGraph::new()));
    let root_slot = PROGRESS_ROOT.get_or_init(|| Mutex::new(None));

    let Ok(mut graph) = graph_slot.lock() else {
        return;
    };
    graph.set_enabled(enabled);

    if enabled {
        let Ok(root_guard) = root_slot.lock() else {
            return;
        };
        if let Some(root) = root_guard.as_ref() {
            graph.attach(root);
        }
    }
}

fn emit(level: &str, level_style: Style, target: &str, message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let plain_line = format!("[{} {:>5} {}] {}", timestamp, level, target, message);
    state::push_log(&plain_line);

    let rendered_line = format!(
        "[{} {} {}] {}",
        Style::new().dim().apply_to(&timestamp),
        level_style.apply_to(format!("{:>5}", level)),
        Style::new().dim().apply_to(target),
        message
    );

    if !print_via_progress(&rendered_line) {
        println!("{}", rendered_line);
    }
}

fn print_via_progress(line: &str) -> bool {
    let Some(slot) = PROGRESS_ROOT.get() else {
        return false;
    };

    let Ok(guard) = slot.lock() else {
        return false;
    };

    let Some(progress_root) = guard.as_ref() else {
        return false;
    };

    progress_root.println(line).is_ok()
}

fn graph_width() -> usize {
    let (columns, _) = Term::stdout().size();
    let columns = usize::from(columns).max(60);
    columns.saturating_sub(4).clamp(40, 200)
}

fn render_placeholder_graph() -> String {
    format!(
        "Rate Limits  waiting for data  {}",
        "·".repeat(graph_width().min(24))
    )
}

fn render_sparkline(samples: &VecDeque<f64>, width: usize) -> String {
    const BARS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    if samples.is_empty() {
        return "·".repeat(width.min(24));
    }

    let max = samples.iter().copied().fold(0.0_f64, f64::max).max(0.1);
    let start = samples.len().saturating_sub(width);

    samples
        .iter()
        .skip(start)
        .map(|sample| {
            let idx = ((sample / max) * (BARS.len() - 1) as f64).round() as usize;
            BARS[idx.min(BARS.len() - 1)]
        })
        .collect()
}
