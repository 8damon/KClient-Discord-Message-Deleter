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
    "TIPS:\n  COMMANDS USE DOUBLE DASHES FOR LONG OPTIONS: --token VALUE  --server VALUE  --tf VALUE\n  SINGLE DASHES ARE ONLY FOR SHORT OPTIONS LIKE -h OR -V\n  COPY THIS EXACTLY TO STORE AN ACCOUNT:\n    kclient --add-account --token TOKEN_HERE\n  COPY THIS EXACTLY TO DELETE THE LAST 24 HOURS FROM A SERVER WITH A STORED ACCOUNT:\n    kclient --account myuser --delete --server SERVER_ID --tf 24h\n  COPY THIS EXACTLY TO RUN ONCE WITHOUT STORING THE TOKEN:\n    kclient --token TOKEN_HERE --delete --channel CHANNEL_ID --tf 24h\n  COPY THIS EXACTLY TO REMOVE A STORED ACCOUNT:\n    kclient --remove-account --account myuser"
}
