use std::thread;

use clap::{ArgAction, Parser};

#[derive(Parser, Debug)]
#[command(
    name = "kclient",
    author,
    version,
    about = "kcordclient - Discord message deletion client",
    after_help = "Examples:\n  kclient --add-account --token TOKEN_HERE\n  kclient --remove-account --account myuser\n  kclient --account myuser --delete --channel 680459914828972076 --tf 24h\n  kclient --account 123456789012345678 --delete --server 123456789012345678 --all\n  kclient --account myuser --delete --dm 123456789012345678 --tf 30m\n  kclient --uninstall"
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

    #[arg(long, conflicts_with = "all", help = "Timeframe like 30m, 24h, or 7d")]
    pub tf: Option<String>,

    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "tf", help = "Delete all available messages")]
    pub all: bool,

    #[arg(long, default_value_t = default_concurrency(), help = "Number of concurrent channel workers")]
    pub concurrency: usize,

    #[arg(long, action = ArgAction::SetTrue, help = "Drop saved checkpoints before starting")]
    pub reload: bool,

    #[arg(long, action = ArgAction::SetTrue, help = "Load proxy.txt and rotate proxies")]
    pub dproxy: bool,
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
}
