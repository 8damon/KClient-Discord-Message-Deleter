mod accounts;
mod cli;
mod processing;
mod progress;
mod targets;

use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use chrono::Local;
use clap::Parser;

use crate::{
    args::Args,
    checkpoint,
    discord::DiscordClient,
    paths, proxy,
    report::RunReport,
    state, storage, terminal,
    timeframe::{describe_timeframe, resolve_cutoff},
};

use self::{
    accounts::{
        add_account_from_args, format_account_identity, remove_account_from_args,
        resolve_token_for_cli,
    },
    cli::{confirm_deletion, ensure_concurrency, ensure_delete_mode, print_cli_help},
    processing::{process_channels, process_server_channels, ServerProcessContext},
    targets::{resolve_channels, resolve_target, TargetKind},
};

pub async fn run() -> Result<()> {
    storage::initialize()?;
    terminal::set_rate_limit_graph_enabled(true);

    if std::env::args_os().len() == 1 {
        return print_cli_help();
    }

    let args = Args::parse();
    if args.uninstall {
        return uninstall_local_state();
    }
    if args.add_account {
        return add_account_from_args(&args).await;
    }
    if args.remove_account {
        return remove_account_from_args(&args);
    }

    ensure_delete_mode(&args)?;
    ensure_concurrency(&args)?;

    if args.reload {
        checkpoint::clear_all()?;
        terminal::info(
            "kcordclient::app",
            "cleared saved checkpoints because --reload was provided",
        );
    }

    let (token, account_label) = resolve_token_for_cli(&args)?;
    let discord = Arc::new(build_discord_client(
        &token,
        args.dproxy,
        args.concurrency,
        None,
    )?);
    let me = discord.me().await?;
    terminal::info(
        "kcordclient::app",
        format!("logged in as {}", format_account_identity(&me)),
    );

    if let Some(label) = account_label {
        terminal::info(
            "kcordclient::app",
            format!("selected stored account {}", label),
        );
    }

    let mut target = resolve_target(&discord, &args).await?;
    let cutoff = resolve_cutoff(args.all, args.tf.as_deref())?;
    let timeframe_desc = describe_timeframe(args.all, args.tf.as_deref());
    let channels = resolve_channels(&discord, &target).await?;

    if matches!(target.kind, TargetKind::Dm) {
        if let Some((_, display_name)) = channels.first() {
            target.name = display_name.clone();
            target.prompt_name = display_name.clone();
        }
    }

    let report = Arc::new(RunReport::new(target.name.clone(), timeframe_desc.clone()));
    confirm_deletion(&timeframe_desc, &target.prompt_name)?;
    state::reset_run(&target.name, &timeframe_desc);

    match target.kind {
        TargetKind::Server => {
            process_server_channels(ServerProcessContext {
                discord: discord.clone(),
                guild_id: target.id.clone(),
                guild_name: target.name.clone(),
                my_id: me.id.clone(),
                channels,
                cutoff,
                concurrency: args.concurrency,
                report: report.clone(),
            })
            .await?;
        }
        _ => {
            let aggressive_delete = channels.len() == 1;
            process_channels(
                discord,
                &me.id,
                channels,
                cutoff,
                args.concurrency,
                aggressive_delete,
                report.clone(),
            )
            .await?;
        }
    }

    terminal::set_progress_root(None);
    let log_path = write_log(&report)?;
    state::finish_run("completed");
    terminal::plain(format!("Done. Log written to {}", log_path.display()));
    Ok(())
}

pub(crate) fn build_discord_client(
    token: &str,
    use_proxy: bool,
    concurrency: usize,
    custom_proxy_list: Option<&str>,
) -> Result<DiscordClient> {
    if use_proxy || custom_proxy_list.is_some() {
        let (proxies, source) = if let Some(proxy_list) = custom_proxy_list {
            (
                proxy::parse_proxy_list(proxy_list)?,
                String::from("custom proxy list"),
            )
        } else {
            let proxy_path = find_proxy_file()?;
            (
                proxy::load_proxy_file(&proxy_path)?,
                proxy_path.display().to_string(),
            )
        };

        anyhow::ensure!(
            !proxies.is_empty(),
            "proxy list is empty; add at least one ip:port:user:pass entry"
        );

        terminal::info(
            "kcordclient::app",
            format!(
                "proxy mode: {} proxies loaded (+source IP) from {}",
                proxies.len(),
                source
            ),
        );

        let clients = proxy::build_client_pool(token, &proxies)?;
        Ok(DiscordClient::from_clients(clients, concurrency))
    } else {
        DiscordClient::new(token, concurrency)
    }
}

fn find_proxy_file() -> Result<PathBuf> {
    if let Ok(current_dir) = std::env::current_dir() {
        let path = current_dir.join("proxy.txt");
        if path.is_file() {
            return Ok(path);
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        for ancestor in exe_path.ancestors() {
            let path = ancestor.join("proxy.txt");
            if path.is_file() {
                return Ok(path);
            }
        }
    }

    bail!("proxy.txt not found (checked current directory and executable directory)")
}

fn write_log(report: &RunReport) -> Result<PathBuf> {
    let log_dir = paths::logs_dir();
    fs::create_dir_all(&log_dir)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&log_dir, fs::Permissions::from_mode(0o700)).ok();
    }

    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    let log_path = log_dir.join(format!("delete_{timestamp}.log"));
    let mut file = File::create(&log_path)?;
    file.write_all(report.render().as_bytes())?;
    Ok(log_path)
}

fn uninstall_local_state() -> Result<()> {
    let mut removed_any = false;
    let cleared_secrets = storage::cleanup_account_secrets().unwrap_or(0);

    if cleared_secrets > 0 {
        terminal::plain(format!(
            "Cleared {} stored token entrie(s).",
            cleared_secrets
        ));
        removed_any = true;
    }

    for path in paths::uninstall_paths() {
        if path.exists() {
            fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
            terminal::plain(format!("Removed {}", path.display()));
            removed_any = true;
        }
    }

    if !removed_any {
        terminal::plain("No local kcordclient data or stored token entries were present.");
    }

    terminal::plain(
        "Uninstall complete. Run the platform uninstall script if you also want the installed binary removed.",
    );
    Ok(())
}
