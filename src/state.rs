use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

use chrono::Local;
use serde::Serialize;

const MAX_LOG_LINES: usize = 250;
const MAX_RATE_LIMIT_SAMPLES: usize = 120;

static DASHBOARD: OnceLock<Arc<Mutex<DashboardState>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Default)]
pub struct DashboardState {
    pub mode: String,
    pub target_name: String,
    pub status: String,
    pub started_at: Option<String>,
    pub found: u64,
    pub deleted: u64,
    pub remaining: u64,
    pub active: usize,
    pub fetching: usize,
    pub eta: String,
    pub logs: Vec<String>,
    pub rate_limits: Vec<f64>,
    pub last_error: Option<String>,
    pub running: bool,
}

pub fn reset_run(target_name: &str, mode: &str) {
    if let Ok(mut state) = dashboard().lock() {
        *state = DashboardState {
            mode: mode.to_string(),
            target_name: target_name.to_string(),
            status: "starting".to_string(),
            started_at: Some(Local::now().to_rfc3339()),
            running: true,
            ..DashboardState::default()
        };
    }
}

pub fn finish_run(status: &str) {
    if let Ok(mut state) = dashboard().lock() {
        state.status = status.to_string();
        state.running = false;
    }
}

pub fn set_error(error: &str) {
    if let Ok(mut state) = dashboard().lock() {
        state.last_error = Some(error.to_string());
        state.status = "failed".to_string();
        state.running = false;
    }
}

pub fn update_progress(
    found: u64,
    deleted: u64,
    remaining: u64,
    active: usize,
    fetching: usize,
    eta: &str,
    status: &str,
) {
    if let Ok(mut state) = dashboard().lock() {
        state.found = found;
        state.deleted = deleted;
        state.remaining = remaining;
        state.active = active;
        state.fetching = fetching;
        state.eta = eta.to_string();
        state.status = status.to_string();
    }
}

pub fn push_log(line: &str) {
    if let Ok(mut state) = dashboard().lock() {
        let mut logs = VecDeque::from(std::mem::take(&mut state.logs));
        logs.push_back(line.to_string());
        while logs.len() > MAX_LOG_LINES {
            logs.pop_front();
        }
        state.logs = logs.into_iter().collect();
    }
}

pub fn push_rate_limit(wait_secs: f64) {
    if let Ok(mut state) = dashboard().lock() {
        let mut samples = VecDeque::from(std::mem::take(&mut state.rate_limits));
        samples.push_back(wait_secs);
        while samples.len() > MAX_RATE_LIMIT_SAMPLES {
            samples.pop_front();
        }
        state.rate_limits = samples.into_iter().collect();
    }
}

fn dashboard() -> &'static Arc<Mutex<DashboardState>> {
    DASHBOARD.get_or_init(|| Arc::new(Mutex::new(DashboardState::default())))
}
