use std::io::{self, Write};

use anyhow::{anyhow, bail, Result};
use clap::CommandFactory;

use crate::args::Args;

pub(crate) fn ensure_delete_mode(args: &Args) -> Result<()> {
    if args.delete {
        Ok(())
    } else {
        bail!("use --delete, --add-account, --remove-account, or --uninstall")
    }
}

pub(crate) fn ensure_concurrency(args: &Args) -> Result<()> {
    if args.concurrency > 0 {
        Ok(())
    } else {
        bail!("--concurrency must be greater than zero")
    }
}

pub(crate) fn confirm_deletion(timeframe_desc: &str, target_name: &str) -> Result<()> {
    println!(
        "You want to DELETE {} in {}? (Y/N)",
        timeframe_desc, target_name
    );
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if input.trim().eq_ignore_ascii_case("y") {
        Ok(())
    } else {
        Err(anyhow!("cancelled"))
    }
}

pub(crate) fn print_cli_help() -> Result<()> {
    let mut command = Args::command();
    command.print_long_help()?;
    println!();
    println!();
    println!("{}", help_tips());
    Ok(())
}

fn help_tips() -> &'static str {
    "TIPS:\n  STORE AN ACCOUNT: kclient --add-account --token TOKEN_HERE\n  REMOVE AN ACCOUNT: kclient --remove-account --account myuser\n  ONE-OFF RUN WITH TOKEN: kclient --token TOKEN_HERE --delete --channel CHANNEL_ID --tf 24h\n  USE A STORED ACCOUNT: kclient --account myuser --delete --server SERVER_ID --all"
}
