use std::{fs, time::SystemTime};

use anyhow::{bail, Result};
use chrono::{DateTime, Local, Utc};

use crate::{args::Args, paths, storage, terminal};

const LOG_LIST_LIMIT: usize = 20;
const RUNNER_FRESH_SECS: i64 = 20;

pub(crate) fn handle_command(args: &Args) -> Result<bool> {
    let requested = args.list_accounts
        || args.list_logs
        || args.list_watchdogs
        || args.show_watchdog.is_some()
        || args.watchdog_status
        || args.stat;
    if !requested {
        return Ok(false);
    }
    if args.delete
        || args.add_account
        || args.remove_account
        || args.uninstall
        || args.set.is_some()
        || args.set_watchdog
        || args.save_watchdog.is_some()
        || args.edit_watchdog.is_some()
        || args.delete_watchdog.is_some()
        || args.start_watchdog.is_some()
        || args.stop_watchdog.is_some()
        || args.run_watchdog_account.is_some()
    {
        bail!("inspection commands cannot be combined with another command mode");
    }

    if args.list_accounts {
        print_accounts()?;
    }
    if args.list_logs {
        print_logs()?;
    }
    if args.list_watchdogs {
        print_watchdogs()?;
    }
    if let Some(name) = args.show_watchdog.as_deref() {
        print_watchdog(name)?;
    }
    if args.watchdog_status {
        print_watchdog_status()?;
    }
    if args.stat {
        print_stat()?;
    }
    Ok(true)
}

fn print_stat() -> Result<()> {
    let accounts = storage::list_accounts()?;
    let watchdogs = storage::list_watchdogs()?;
    let runners = storage::list_watchdog_runners()?;
    let active_runners = runners
        .iter()
        .filter(|runner| Utc::now().timestamp() - runner.heartbeat_at <= RUNNER_FRESH_SECS)
        .count();
    let running_watchdogs = watchdogs
        .iter()
        .filter(|watchdog| watchdog.requested_running)
        .count();
    let log_count = fs::read_dir(paths::logs_dir())
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .count();
    terminal::plain("KCLIENT local status");
    terminal::plain(format!("- stored accounts: {}", accounts.len()));
    terminal::plain(format!(
        "- saved watchdogs: {} ({} requested running)",
        watchdogs.len(),
        running_watchdogs
    ));
    terminal::plain(format!(
        "- worker sessions: {} ({} active)",
        runners.len(),
        active_runners
    ));
    terminal::plain(format!("- local logs: {log_count}"));
    terminal::plain(format!(
        "- data directory: {}",
        paths::app_root_dir().display()
    ));
    Ok(())
}

fn print_accounts() -> Result<()> {
    let accounts = storage::list_accounts()?;
    if accounts.is_empty() {
        terminal::plain(
            "No stored accounts. Add one with: kclient --add-account --token TOKEN_HERE",
        );
        return Ok(());
    }
    terminal::plain(format!("Stored accounts ({})", accounts.len()));
    for account in accounts {
        let display = account.display_name.as_deref().unwrap_or(&account.username);
        terminal::plain(format!(
            "- {} (@{}, id={}) | created {} | last used {}",
            display, account.username, account.user_id, account.created_at, account.last_used_at
        ));
    }
    Ok(())
}

fn print_logs() -> Result<()> {
    let directory = paths::logs_dir();
    let mut logs = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                metadata.is_file().then_some((entry.path(), metadata))
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    logs.sort_by_key(|(_, metadata)| {
        std::cmp::Reverse(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH))
    });

    if logs.is_empty() {
        terminal::plain(
            "No local logs yet. Logs are created after a delete run or watchdog activity.",
        );
        return Ok(());
    }
    terminal::plain(format!(
        "Latest local logs (showing up to {LOG_LIST_LIMIT})"
    ));
    for (path, metadata) in logs.into_iter().take(LOG_LIST_LIMIT) {
        let modified = metadata
            .modified()
            .ok()
            .map(|time| {
                DateTime::<Local>::from(time)
                    .format("%Y-%m-%d %H:%M:%S %Z")
                    .to_string()
            })
            .unwrap_or_else(|| "unknown time".to_string());
        terminal::plain(format!(
            "- {} | {} bytes | {}",
            path.display(),
            metadata.len(),
            modified
        ));
    }
    Ok(())
}

fn print_watchdogs() -> Result<()> {
    let accounts = storage::list_accounts()?;
    let watchdogs = storage::list_watchdogs()?;
    if watchdogs.is_empty() {
        terminal::plain("No saved watchdogs. Run `kclient --set watchdog` for a guided setup.");
        return Ok(());
    }
    terminal::plain(format!("Saved watchdogs ({})", watchdogs.len()));
    for watchdog in watchdogs {
        terminal::plain(format!(
            "- {} | {} | {} | delay {} | default {} | account {} | {}",
            watchdog.name,
            watchdog.kind,
            describe_scope(&watchdog),
            describe_delay(watchdog.delay_seconds),
            if watchdog.default_on { "on" } else { "off" },
            account_label(&accounts, watchdog.account_id),
            if watchdog.requested_running {
                "requested: running"
            } else {
                "requested: stopped"
            },
        ));
    }
    Ok(())
}

fn print_watchdog(name: &str) -> Result<()> {
    let Some(watchdog) = storage::load_watchdog(name)? else {
        bail!("no saved watchdog named `{name}`");
    };
    let accounts = storage::list_accounts()?;
    terminal::plain(format!("Watchdog `{}`", watchdog.name));
    terminal::plain(format!(
        "  account: {}",
        account_label(&accounts, watchdog.account_id)
    ));
    terminal::plain(format!("  type: {}", watchdog.kind));
    terminal::plain(format!("  scope: {}", describe_scope(&watchdog)));
    terminal::plain(format!(
        "  deletion delay: {}",
        describe_delay(watchdog.delay_seconds)
    ));
    terminal::plain(format!(
        "  default: {}",
        if watchdog.default_on { "on" } else { "off" }
    ));
    terminal::plain(format!("  off flag: {}", watchdog.off_flag));
    terminal::plain(format!(
        "  requested state: {}",
        if watchdog.requested_running {
            "running"
        } else {
            "stopped"
        }
    ));
    if watchdog.kind == "message" {
        terminal::plain(format!("  word list ({}):", watchdog.words.len()));
        for word in watchdog.words {
            terminal::plain(format!("    - {word}"));
        }
    }
    Ok(())
}

fn print_watchdog_status() -> Result<()> {
    let accounts = storage::list_accounts()?;
    let runners = storage::list_watchdog_runners()?;
    if runners.is_empty() {
        terminal::plain("No watchdog worker sessions have reported in. Start one with: kclient --start-watchdog NAME");
        return Ok(());
    }
    terminal::plain(format!("Watchdog worker sessions ({})", runners.len()));
    for runner in runners {
        let age = (Utc::now().timestamp() - runner.heartbeat_at).max(0);
        let state = if age <= RUNNER_FRESH_SECS {
            "active"
        } else {
            "stale"
        };
        terminal::plain(format!(
            "- {} | pid {} | {} | heartbeat {}s ago{}",
            account_label(&accounts, runner.account_id),
            runner.pid,
            state,
            age,
            runner
                .last_error
                .as_deref()
                .map(|error| format!(" | last error: {error}"))
                .unwrap_or_default(),
        ));
    }
    Ok(())
}

fn account_label(accounts: &[storage::StoredAccount], account_id: i64) -> String {
    accounts
        .iter()
        .find(|account| account.id == account_id)
        .map(|account| {
            format!(
                "{} (@{}, id={})",
                account.display_name.as_deref().unwrap_or(&account.username),
                account.username,
                account.user_id
            )
        })
        .unwrap_or_else(|| format!("removed account (database id={account_id})"))
}

fn describe_scope(watchdog: &storage::StoredWatchdog) -> String {
    match watchdog.scope_id.as_deref() {
        Some(id) => format!("{} {id}", watchdog.scope_kind),
        None => "global".to_string(),
    }
}

fn describe_delay(seconds: i64) -> String {
    if seconds % 86_400 == 0 {
        format!("{}d", seconds / 86_400)
    } else if seconds % 3_600 == 0 {
        format!("{}h", seconds / 3_600)
    } else if seconds % 60 == 0 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}
