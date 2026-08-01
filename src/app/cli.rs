use std::io::{self, Write};

use anyhow::{anyhow, bail, Result};
use clap::CommandFactory;
use console::Style;

use crate::args::Args;

pub(crate) fn ensure_delete_mode(args: &Args) -> Result<()> {
    if args.delete {
        Ok(())
    } else {
        bail!("use --delete, account management, --uninstall, --help-token, or a watchdog command")
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

pub(crate) fn print_help_token() -> Result<()> {
    let heading = Style::new().bold().cyan();
    let label = Style::new().bold().yellow();
    let command = Style::new().bold().green();

    println!();
    println!("{}", heading.apply_to("How to get your Discord token"));
    println!();
    println!("{}", label.apply_to("Quick method (Discord web):"));
    println!("  - Open Discord in your browser");
    println!("  - Open Developer Tools (Ctrl+Shift+I)");
    println!("  - Open Application → Local Storage");
    println!("  - Filter for \"token\" and copy the value");
    println!();
    println!("{}", label.apply_to("Notes:"));
    println!("  - Copy only the token value (no extra text)");
    println!("  - Keep it private; treat it like a password");
    println!(
        "  - Store it once with: {}",
        command.apply_to("kclient --add-account --token TOKEN_HERE")
    );
    println!(
        "  - Run once with: {}",
        command.apply_to("kclient --token TOKEN_HERE --delete --channel CHANNEL_ID --tf 24h")
    );
    Ok(())
}

fn help_tips() -> String {
    let heading = Style::new().bold().cyan();
    let label = Style::new().bold().yellow();
    let command = Style::new().bold().green();
    format!(
        "{tips}\n\
  {double_dash}: {double_dash_body}\n\
  {single_dash}: {single_dash_body}\n\
  {token_help}\n\
    {token_help_cmd}\n\
  {store}\n\
    {store_cmd}\n\
  {account_list}\n\
    {account_list_cmd}\n\
  {server_delete}\n\
    {server_delete_cmd}\n\
  {delete_type}\n\
    {delete_type_cmd}\n\
  {verbosity}\n\
    {verbosity_cmd}\n\
  {debug}\n\
    {debug_cmd}\n\
  {watchdog_guide}\n\
    {watchdog_guide_cmd}\n\
  {watchdog_start}\n\
    {watchdog_start_cmd}\n\
  {watchdog_inspect}\n\
    {watchdog_list_cmd}\n\
    {watchdog_show_cmd}\n\
  {watchdog_status}\n\
    {watchdog_status_cmd}\n\
  {log_list}\n\
    {log_list_cmd}\n\
  {remove}\n\
    {remove_cmd}",
        tips = heading.apply_to("Tips"),
        double_dash = label.apply_to("Long options use double dashes"),
        double_dash_body = "--token VALUE  --server VALUE  --tf VALUE",
        single_dash = label.apply_to("Short options use a single dash"),
        single_dash_body = "-h  -V",
        token_help = label.apply_to("Need token instructions"),
        token_help_cmd = command.apply_to("kclient help-token"),
        store = label.apply_to("Copy this exactly to store an account"),
        store_cmd = command.apply_to("kclient --add-account --token TOKEN_HERE"),
        account_list = label.apply_to("List stored accounts (tokens are never shown)"),
        account_list_cmd = command.apply_to("kclient --list-accounts"),
        server_delete = label.apply_to("Copy this to delete the last 24 hours from one server"),
        server_delete_cmd =
            command.apply_to("kclient --account myuser --delete --server SERVER_ID --tf 24h"),
        delete_type = label.apply_to("Delete only a specific content type"),
        delete_type_cmd =
            command.apply_to("kclient --delete --type links --channel CHANNEL_ID --tf 24h"),
        verbosity = label.apply_to("Increase output verbosity"),
        verbosity_cmd = command.apply_to("kclient --delete --channel CHANNEL_ID --tf 24h -vv"),
        debug = label.apply_to("Enable debug logging"),
        debug_cmd = command.apply_to("kclient --delete --channel CHANNEL_ID --tf 24h --debug"),
        watchdog_guide = label.apply_to("Need help configuring a watchdog?"),
        watchdog_guide_cmd = command.apply_to("kclient --set watchdog"),
        watchdog_start = label.apply_to("Start a saved watchdog"),
        watchdog_start_cmd = command.apply_to("kclient --start-watchdog media-expiry"),
        watchdog_inspect = label.apply_to("Saved watchdogs"),
        watchdog_list_cmd = command.apply_to("kclient --list-watchdogs"),
        watchdog_show_cmd = command.apply_to("kclient --show-watchdog media-expiry"),
        watchdog_status = label.apply_to("Show active watchdog worker sessions"),
        watchdog_status_cmd = command.apply_to("kclient --watchdog-status"),
        log_list = label.apply_to("List recent logs"),
        log_list_cmd = command.apply_to("kclient --list-logs"),
        remove = label.apply_to("Copy this exactly to remove a stored account"),
        remove_cmd = command.apply_to("kclient --remove-account --account myuser"),
    )
}
