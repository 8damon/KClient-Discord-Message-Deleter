use std::io::{self, Write};

use anyhow::{anyhow, bail, Result};
use clap::CommandFactory;
use console::Style;

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

fn help_tips() -> String {
    let heading = Style::new().bold().cyan();
    let label = Style::new().bold().yellow();
    let command = Style::new().bold().green();
    let note = Style::new().bold().magenta();

    format!(
        "{tips}\n  {double_dash}: {double_dash_body}\n  {single_dash}: {single_dash_body}\n  {store}\n    {store_cmd}\n  {token_note}\n    {token_note_body}\n  {server_delete}\n    {server_delete_cmd}\n  {one_off}\n    {one_off_cmd}\n  {remove}\n    {remove_cmd}",
        tips = heading.apply_to("TIPS"),
        double_dash = label.apply_to("LONG OPTIONS USE DOUBLE DASHES"),
        double_dash_body = "--token VALUE  --server VALUE  --tf VALUE",
        single_dash = label.apply_to("SHORT OPTIONS USE A SINGLE DASH"),
        single_dash_body = "-h  -V",
        store = label.apply_to("COPY THIS EXACTLY TO STORE AN ACCOUNT"),
        store_cmd = command.apply_to("kclient --add-account --token TOKEN_HERE"),
        token_note = note.apply_to("IF YOU DO NOT KNOW HOW TO GET YOUR TOKEN"),
        token_note_body = "DISCORD WEB -> CTRL+SHIFT+I -> APPLICATION -> LOCAL STORAGE -> FILTER \"token\" -> COPY VALUE",
        server_delete = label.apply_to("COPY THIS EXACTLY TO DELETE THE LAST 24 HOURS FROM A SERVER WITH A STORED ACCOUNT"),
        server_delete_cmd = command.apply_to("kclient --account myuser --delete --server SERVER_ID --tf 24h"),
        one_off = label.apply_to("COPY THIS EXACTLY TO RUN ONCE WITHOUT STORING THE TOKEN"),
        one_off_cmd = command.apply_to("kclient --token TOKEN_HERE --delete --channel CHANNEL_ID --tf 24h"),
        remove = label.apply_to("COPY THIS EXACTLY TO REMOVE A STORED ACCOUNT"),
        remove_cmd = command.apply_to("kclient --remove-account --account myuser"),
    )
}
