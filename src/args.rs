use std::{ffi::OsString, thread};

use clap::{ArgAction, Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum WatchdogType {
    Message,
    Media,
    Links,
    Files,
    Videos,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum WatchdogDefault {
    On,
    Off,
}

#[derive(Parser, Debug)]
#[command(
    name = "kclient",
    author,
    version,
    about = "kcordclient - Discord message deletion client",
    after_help = "Examples:\n  kclient --add-account --token TOKEN_HERE\n  kclient --account myuser --delete --channel CHANNEL_ID --tf 24h --type links\n  kclient --help-token\n  kclient --set-watchdog --save-watchdog media-expiry --account myuser --type media --tf 1hr\n\nWatchdog config commands are available via `kclient wd` shortcuts.\nUse `kclient --help-token` for token setup details."
)]
pub struct Args {
    #[arg(long, action = ArgAction::SetTrue, help = "Run a delete job")]
    pub delete: bool,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        conflicts_with = "delete",
        help = "Store an account token locally"
    )]
    pub add_account: bool,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        conflicts_with_all = ["delete", "add_account", "uninstall"],
        help = "Remove a stored account"
    )]
    pub remove_account: bool,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        conflicts_with_all = ["delete", "add_account", "remove_account"],
        help = "Remove local kcordclient data"
    )]
    pub uninstall: bool,

    #[arg(long, help = "Discord token for add-account or one-off delete runs")]
    pub token: Option<String>,

    #[arg(long = "help-token", action = ArgAction::SetTrue, help = "Show how to obtain a Discord token")]
    pub help_token: bool,

    #[arg(
        long,
        help = "Stored account selector: user id, username, or display name"
    )]
    pub account: Option<String>,

    #[arg(long, conflicts_with_all = ["channel", "dm"], help = "Target a server by guild id")]
    pub server: Option<String>,

    #[arg(long, conflicts_with_all = ["server", "dm"], help = "Target a single channel by id")]
    pub channel: Option<String>,

    #[arg(long, conflicts_with_all = ["server", "channel"], help = "Target a DM by user id")]
    pub dm: Option<String>,

    #[arg(
        long,
        conflicts_with = "all",
        help = "Timeframe like 30m, 24h, 1hr, or 7d"
    )]
    pub tf: Option<String>,

    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "tf", help = "Delete all available messages")]
    pub all: bool,

    #[arg(long, default_value_t = default_concurrency(), help = "Number of concurrent channel workers")]
    pub concurrency: usize,

    #[arg(
        short = 'v',
        long,
        action = ArgAction::Count,
        help = "Increase output verbosity. Repeat for more details."
    )]
    pub verbose: u8,

    #[arg(long, action = ArgAction::SetTrue, help = "Drop saved checkpoints before starting")]
    pub reload: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "Enable debug output for detailed troubleshooting")]
    pub debug: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "Load proxy.txt and rotate proxies")]
    pub dproxy: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "List stored accounts and last-used times")]
    pub list_accounts: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "List the 20 most recent local logs")]
    pub list_logs: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "List saved watchdogs and their requested state")]
    pub list_watchdogs: bool,

    #[arg(
        long,
        value_name = "NAME",
        help = "Show the complete saved configuration for NAME"
    )]
    pub show_watchdog: Option<String>,

    #[arg(long, action = ArgAction::SetTrue, help = "Show watchdog worker sessions and recent errors")]
    pub watchdog_status: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "Show a compact local account, watchdog, worker, and log summary")]
    pub stat: bool,

    #[arg(
        long,
        value_name = "WHAT",
        help = "Guided setup alias; use --set watchdog"
    )]
    pub set: Option<String>,

    #[arg(long, action = ArgAction::SetTrue, help = "Configure a watchdog; combine with --save-watchdog NAME")]
    pub set_watchdog: bool,

    #[arg(
        long,
        value_name = "NAME",
        help = "Save the --set-watchdog configuration under NAME"
    )]
    pub save_watchdog: Option<String>,

    #[arg(
        long,
        value_name = "NAME",
        help = "Replace the saved configuration for NAME; show it first with --show-watchdog"
    )]
    pub edit_watchdog: Option<String>,

    #[arg(
        long,
        value_name = "NAME",
        help = "Stop, remove queued messages, and delete saved watchdog NAME"
    )]
    pub delete_watchdog: Option<String>,

    #[arg(
        long,
        value_name = "NAME",
        help = "Start a saved watchdog in the background (shared per account)"
    )]
    pub start_watchdog: Option<String>,

    #[arg(long, value_name = "NAME", help = "Gracefully stop a running watchdog")]
    pub stop_watchdog: Option<String>,

    #[arg(long, hide = true, value_name = "ACCOUNT_ID")]
    pub run_watchdog_account: Option<i64>,

    #[arg(
        long,
        value_enum,
        help = "Filter by message type. For --delete use message/links/media/files/videos. For watchdog use media or message"
    )]
    pub r#type: Option<WatchdogType>,

    #[arg(
        long,
        help = "Case-insensitive marker: opts out by default, opts in with --default off"
    )]
    pub off_flag: Option<String>,

    #[arg(long, value_enum, default_value_t = WatchdogDefault::On, help = "on: delete matching messages unless marked; off: delete only marked matches")]
    pub default: WatchdogDefault,

    #[arg(long = "word-list", value_name = "PATH", action = ArgAction::Append, help = "Word-list file (.txt, .json, .toml, or .yaml); repeatable")]
    pub word_lists: Vec<String>,
}

pub fn parse_cli() -> Args {
    Args::parse_from(normalize_shortcuts(std::env::args_os().collect()))
}

fn normalize_shortcuts(args: Vec<OsString>) -> Vec<OsString> {
    let Some(shortcut) = args.get(1).and_then(|value| value.to_str()) else {
        return args;
    };
    let replacement = match shortcut {
        "stat" => Some(vec!["--stat"]),
        "help-token" => Some(vec!["--help-token"]),
        "wd" | "watchdog" => match args.get(2).and_then(|value| value.to_str()) {
            None | Some("help") => Some(vec!["--set", "watchdog"]),
            Some("set") => Some(vec!["--set", "watchdog"]),
            Some("list") => Some(vec!["--list-watchdogs"]),
            Some("status") | Some("stat") => Some(vec!["--watchdog-status"]),
            Some("show") => Some(vec!["--show-watchdog"]),
            Some("start") => Some(vec!["--start-watchdog"]),
            Some("stop") => Some(vec!["--stop-watchdog"]),
            Some("delete") | Some("rm") => Some(vec!["--delete-watchdog"]),
            Some("edit") => Some(vec!["--edit-watchdog"]),
            _ => None,
        },
        _ => None,
    };
    let Some(replacement) = replacement else {
        return args;
    };
    let consumed = if matches!(shortcut, "wd" | "watchdog") {
        3
    } else {
        2
    };
    let mut normalized = vec![args[0].clone()];
    normalized.extend(replacement.into_iter().map(OsString::from));
    normalized.extend(args.into_iter().skip(consumed));
    normalized
}

fn default_concurrency() -> usize {
    thread::available_parallelism()
        .map(|count| count.get().saturating_mul(2))
        .unwrap_or(8)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Args;

    #[test]
    fn parses_delete_command_with_timeframe() {
        let args = Args::try_parse_from([
            "kclient",
            "--account",
            "damon",
            "--delete",
            "--server",
            "123",
            "--tf",
            "24h",
        ])
        .expect("parse delete args");

        assert_eq!(args.account.as_deref(), Some("damon"));
        assert!(args.delete);
        assert_eq!(args.server.as_deref(), Some("123"));
        assert_eq!(args.tf.as_deref(), Some("24h"));
        assert!(!args.all);
    }

    #[test]
    fn rejects_conflicting_target_flags() {
        let error = Args::try_parse_from([
            "kclient",
            "--delete",
            "--server",
            "123",
            "--channel",
            "456",
            "--all",
        ])
        .expect_err("target flags should conflict");

        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn rejects_conflicting_timeframe_flags() {
        let error = Args::try_parse_from([
            "kclient",
            "--delete",
            "--channel",
            "456",
            "--tf",
            "24h",
            "--all",
        ])
        .expect_err("timeframe flags should conflict");

        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_global_watchdog_configuration() {
        let args = Args::try_parse_from([
            "kclient",
            "--set-watchdog",
            "--save-watchdog",
            "media-expiry",
            "--account",
            "damon",
            "--type",
            "media",
            "--tf",
            "1h",
            "--off-flag",
            ".",
        ])
        .expect("parse watchdog args");

        assert!(args.set_watchdog);
        assert_eq!(args.save_watchdog.as_deref(), Some("media-expiry"));
        assert_eq!(args.r#type, Some(super::WatchdogType::Media));
    }

    #[test]
    fn parses_delete_type_filter() {
        let args = Args::try_parse_from([
            "kclient",
            "--delete",
            "--channel",
            "456",
            "--tf",
            "24h",
            "--type",
            "links",
        ])
        .expect("parse delete filter args");

        assert!(args.delete);
        assert_eq!(args.r#type, Some(super::WatchdogType::Links));
    }
}
