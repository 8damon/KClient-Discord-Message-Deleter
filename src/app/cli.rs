use std::io::{self, Write};

use anyhow::{anyhow, bail, Result};
use clap::CommandFactory;
use console::Style;

use crate::args::Args;

pub(crate) fn ensure_delete_mode(args: &Args) -> Result<()> {
    if args.delete {
        Ok(())
    } else {
        bail!("use --delete, account management, --uninstall, or a watchdog command")
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
        "{tips}\n  {double_dash}: {double_dash_body}\n  {single_dash}: {single_dash_body}\n  {store}\n    {store_cmd}\n  {account_list}\n    {account_list_cmd}\n  {token_note}\n    {token_note_body}\n  {id_note}\n    {id_note_body}\n  {server_delete}\n    {server_delete_cmd}\n  {one_off}\n    {one_off_cmd}\n  {delete_type}\n    {delete_type_cmd}\n  {verbosity}\n    {verbosity_cmd}\n  {debug}\n    {debug_cmd}\n  {watchdog_guide}\n    {watchdog_guide_cmd}\n  {watchdog}\n    {watchdog_cmd}\n  {scoped_watchdog}\n    {scoped_watchdog_cmd}\n  {watchdog_start}\n    {watchdog_start_cmd}\n  {watchdog_inspect}\n    {watchdog_inspect_cmd}\n  {watchdog_status}\n    {watchdog_status_cmd}\n  {log_list}\n    {log_list_cmd}\n  {watchdog_stop}\n    {watchdog_stop_cmd}\n  {watchdog_note}\n    {watchdog_note_body}\n  {remove}\n    {remove_cmd}",
        tips = heading.apply_to("Tips"),
        double_dash = label.apply_to("Long options use double dashes"),
        double_dash_body = "--token VALUE  --server VALUE  --tf VALUE",
        single_dash = label.apply_to("Short options use a single dash"),
        single_dash_body = "-h  -V",
        store = label.apply_to("Copy this exactly to store an account"),
        store_cmd = command.apply_to("kclient --add-account --token TOKEN_HERE"),
        account_list = label.apply_to("List stored accounts (tokens are never shown)"),
        account_list_cmd = command.apply_to("kclient --list-accounts"),
        token_note = note.apply_to("If you do not know how to get your token"),
        token_note_body = "Discord web -> Ctrl+Shift+I -> Application -> Local Storage -> filter \"token\" -> copy value",
        id_note = note.apply_to("If you do not know how to get a user ID or server ID"),
        id_note_body = "Discord -> User Settings -> Advanced -> turn on Developer Mode -> right-click user or server icon -> Copy User ID or Copy Server ID",
        server_delete = label.apply_to("Copy this exactly to delete the last 24 hours from a server with a stored account"),
        server_delete_cmd = command.apply_to("kclient --account myuser --delete --server SERVER_ID --tf 24h"),
        one_off = label.apply_to("Copy this exactly to run once without storing the token"),
        one_off_cmd = command.apply_to("kclient --token TOKEN_HERE --delete --channel CHANNEL_ID --tf 24h"),
        delete_type = label.apply_to("Delete only a specific content type"),
        delete_type_cmd = command.apply_to("kclient --delete --type links --channel CHANNEL_ID --tf 24h"),
        verbosity = label.apply_to("Increase output verbosity"),
        verbosity_cmd = command.apply_to("kclient --delete --type message --channel CHANNEL_ID --tf 24h -vv"),
        debug = label.apply_to("Enable debug logging"),
        debug_cmd = command.apply_to("kclient --delete --type message --channel CHANNEL_ID --tf 24h --debug"),
        watchdog_guide = label.apply_to("Need help configuring a watchdog?"),
        watchdog_guide_cmd = command.apply_to("kclient --set watchdog"),
        watchdog = label.apply_to("Save a global media watchdog (delete after one hour unless marked with .)"),
        watchdog_cmd = command.apply_to("kclient --set-watchdog --save-watchdog media-expiry --account myuser --type media --tf 1hr --off-flag ."),
        scoped_watchdog = label.apply_to("Save a server word-list watchdog"),
        scoped_watchdog_cmd = command.apply_to("kclient --set-watchdog --save-watchdog cleanup-words --account myuser --server SERVER_ID --type message --tf 5m --off-flag dnd --word-list words.txt"),
        watchdog_start = label.apply_to("Start a saved watchdog"),
        watchdog_start_cmd = command.apply_to("kclient --start-watchdog media-expiry"),
        watchdog_inspect = label.apply_to("List saved watchdogs or inspect one full configuration"),
        watchdog_inspect_cmd = command.apply_to("kclient --list-watchdogs    kclient --show-watchdog media-expiry"),
        watchdog_status = label.apply_to("Show active watchdog worker sessions"),
        watchdog_status_cmd = command.apply_to("kclient --watchdog-status"),
        log_list = label.apply_to("List the newest local deletion and watchdog logs"),
        log_list_cmd = command.apply_to("kclient --list-logs"),
        watchdog_stop = label.apply_to("Stop a saved watchdog"),
        watchdog_stop_cmd = command.apply_to("kclient --stop-watchdog media-expiry"),
        watchdog_note = note.apply_to("Watchdog scopes and defaults"),
        watchdog_note_body = "No target flag means global. --default on deletes matching messages unless --off-flag is present; --default off requires the flag.",
        remove = label.apply_to("Copy this exactly to remove a stored account"),
        remove_cmd = command.apply_to("kclient --remove-account --account myuser"),
    )
}
