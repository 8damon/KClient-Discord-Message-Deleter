use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::time::{self, MissedTickBehavior};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

use crate::{
    app::build_discord_client,
    args::{Args, WatchdogDefault, WatchdogType},
    discord::DiscordClient,
    models::{Message, OwnedMessage},
    paths, storage, terminal,
    timeframe::parse_timeframe,
};

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
const CONTROL_INTERVAL: Duration = Duration::from_secs(10);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(60);
const FAILED_DELETE_RETRY_DELAY: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct GatewayEnvelope {
    op: i64,
    #[serde(default)]
    d: Value,
    #[serde(default)]
    s: Option<u64>,
    #[serde(default)]
    t: Option<String>,
}

pub async fn handle_command(args: &Args) -> Result<bool> {
    if let Some(account_id) = args.run_watchdog_account {
        run_account(account_id).await?;
        return Ok(true);
    }

    let alias_set = match args.set.as_deref() {
        Some("watchdog") => true,
        Some(value) => bail!("unknown --set target `{value}`; use `kclient --set watchdog`"),
        None => false,
    };
    let set_requested = args.set_watchdog || alias_set;
    let requested = set_requested
        || args.save_watchdog.is_some()
        || args.edit_watchdog.is_some()
        || args.delete_watchdog.is_some()
        || args.start_watchdog.is_some()
        || args.stop_watchdog.is_some();
    if !requested {
        return Ok(false);
    }

    if args.delete || args.add_account || args.remove_account || args.uninstall {
        bail!("watchdog commands cannot be combined with another command mode");
    }
    let lifecycle_count = usize::from(set_requested || args.save_watchdog.is_some())
        + usize::from(args.edit_watchdog.is_some())
        + usize::from(args.delete_watchdog.is_some())
        + usize::from(args.start_watchdog.is_some())
        + usize::from(args.stop_watchdog.is_some());
    if lifecycle_count != 1 {
        bail!("use exactly one watchdog action: save, edit, delete, start, or stop");
    }

    if set_requested || args.save_watchdog.is_some() {
        if args.save_watchdog.is_none() {
            print_setup_help();
            return Ok(true);
        }
        save_from_args(args, args.save_watchdog.as_deref().unwrap()).await?;
    } else if let Some(name) = args.edit_watchdog.as_deref() {
        if storage::load_watchdog(name)?.is_none() {
            bail!("no saved watchdog named `{name}`; create one with --set-watchdog --save-watchdog NAME");
        }
        save_from_args(args, name).await?;
    } else if let Some(name) = args.delete_watchdog.as_deref() {
        delete(name)?;
    } else if let Some(name) = args.start_watchdog.as_deref() {
        start(name)?;
    } else if let Some(name) = args.stop_watchdog.as_deref() {
        stop(name)?;
    }
    Ok(true)
}

async fn save_from_args(args: &Args, name: &str) -> Result<()> {
    let name = name.trim();
    validate_name(name)?;
    let kind = match args.r#type {
        Some(WatchdogType::Media) => WatchdogType::Media,
        Some(WatchdogType::Message) => WatchdogType::Message,
        Some(_) => bail!("watchdog configuration only accepts --type media or --type message"),
        None => bail!("watchdog configuration requires --type media or --type message"),
    };
    terminal::debug(
        "kcordclient::watchdog",
        format!("watchdog {name} requested with kind {:?}", kind),
    );
    let off_flag = args
        .off_flag
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("watchdog configuration requires a non-empty --off-flag"))?;
    let timeframe = args
        .tf
        .as_deref()
        .ok_or_else(|| anyhow!("watchdog configuration requires --tf (for example 5m or 1h)"))?;
    let delay_seconds = parse_timeframe(timeframe)?.num_seconds();
    let account = storage::resolve_account(args.account.as_deref())?;
    terminal::debug(
        "kcordclient::watchdog",
        format!(
            "resolved account id {} for watchdog {}",
            account.account.id, name
        ),
    );
    let discord = build_discord_client(&account.token, false, 2, None)?;
    let (scope_kind, scope_id) = resolve_scope(&discord, args).await?;
    terminal::debug(
        "kcordclient::watchdog",
        format!("resolved scope kind {} for watchdog {}", scope_kind, name),
    );

    let words = match kind {
        WatchdogType::Media => {
            if !args.word_lists.is_empty() {
                bail!("--word-list is only valid with --type message");
            }
            Vec::new()
        }
        WatchdogType::Message => {
            if args.word_lists.is_empty() {
                bail!("--type message requires at least one --word-list PATH");
            }
            load_words(&args.word_lists)?
        }
        _ => unreachable!("invalid watchdog type"),
    };
    let word_count = words.len();

    storage::save_watchdog(&storage::StoredWatchdog {
        name: name.to_string(),
        account_id: account.account.id,
        scope_kind,
        scope_id,
        kind: watchdog_kind_name(kind).to_string(),
        delay_seconds,
        off_flag: off_flag.to_lowercase(),
        default_on: args.default == WatchdogDefault::On,
        words,
        requested_running: false,
    })?;
    terminal::debug(
        "kcordclient::watchdog",
        format!(
            "saved watchdog '{}' with delay {}s and {} word filter(s)",
            name, delay_seconds, word_count
        ),
    );
    println!("Saved watchdog `{name}`. Start it with: kclient --start-watchdog {name}");
    Ok(())
}

fn print_setup_help() {
    println!(
        "Watchdog setup\n\
\n\
Save first, then start it. A watchdog uses your stored account and watches new messages in real time.\n\
\n\
Global media: delete your uploaded media after one hour unless its message contains a period:\n\
  kclient --set-watchdog --save-watchdog media-expiry --account myuser --type media --tf 1hr --off-flag .\n\
\n\
Server word list: delete matching messages after five minutes unless they contain dnd:\n\
  kclient --set-watchdog --save-watchdog cleanup-words --account myuser --server SERVER_ID --type message --tf 5m --off-flag dnd --word-list words.txt\n\
\n\
Start it:\n\
  kclient --start-watchdog media-expiry\n\
\n\
Scopes: omit --server/--channel/--dm for global; use one of them to limit the watchdog.\n\
Defaults: --default on deletes matching messages unless --off-flag is present. --default off deletes matching messages only when the flag is present.\n\
Edit: show the current configuration, then replace it with the full updated configuration:\n\
  kclient --show-watchdog media-expiry\n\
  kclient --edit-watchdog media-expiry --account myuser --type media --tf 2h --off-flag .\n\
\n\
Delete: kclient --delete-watchdog media-expiry\n\
\n\
Shortcuts: kclient wd, kclient wd list, kclient wd status, kclient stat\n\
Inspect: kclient --list-watchdogs, kclient --show-watchdog NAME, kclient --watchdog-status"
    );
}

async fn resolve_scope(discord: &DiscordClient, args: &Args) -> Result<(String, Option<String>)> {
    match (&args.server, &args.channel, &args.dm) {
        (None, None, None) => Ok(("global".to_string(), None)),
        (Some(id), None, None) => {
            validate_snowflake(id, "--server")?;
            discord.guild(id).await?;
            Ok(("server".to_string(), Some(id.clone())))
        }
        (None, Some(id), None) => {
            validate_snowflake(id, "--channel")?;
            discord.channel(id).await?;
            Ok(("channel".to_string(), Some(id.clone())))
        }
        (None, None, Some(user_id)) => {
            validate_snowflake(user_id, "--dm")?;
            let channel = discord.create_dm(user_id).await?;
            Ok(("dm".to_string(), Some(channel.id)))
        }
        _ => bail!("watchdog scope accepts at most one of --server, --channel, or --dm"),
    }
}

fn start(name: &str) -> Result<()> {
    let watchdog = storage::set_watchdog_running(name, true)?;
    terminal::debug(
        "kcordclient::watchdog",
        format!("start requested for watchdog {name}"),
    );
    if storage::runner_is_fresh(watchdog.account_id, CONTROL_INTERVAL.as_secs() as i64 * 2)? {
        println!("Watchdog `{name}` is active.");
        return Ok(());
    }

    let log_dir = paths::logs_dir();
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("watchdog-account-{}.log", watchdog.account_id));
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let error_log = log.try_clone()?;
    let executable = std::env::current_exe().context("failed to resolve kclient executable")?;
    let mut command = Command::new(executable);
    command
        .arg("--run-watchdog-account")
        .arg(watchdog.account_id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log));
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    #[cfg(windows)]
    command.creation_flags(0x0000_0008 | 0x0800_0000);
    command
        .spawn()
        .context("failed to start watchdog background worker")?;
    println!(
        "Started watchdog `{name}` in the background. Log: {}",
        log_path.display()
    );
    Ok(())
}

fn stop(name: &str) -> Result<()> {
    storage::set_watchdog_running(name, false)?;
    storage::remove_watchdog_candidates(name)?;
    terminal::debug(
        "kcordclient::watchdog",
        format!("stop requested for watchdog {name}"),
    );
    println!("Stop requested for watchdog `{name}`.");
    Ok(())
}

fn delete(name: &str) -> Result<()> {
    storage::delete_watchdog(name)?;
    terminal::debug("kcordclient::watchdog", format!("deleted watchdog {name}"));
    println!("Deleted watchdog `{name}` and its queued messages.");
    Ok(())
}

async fn run_account(account_id: i64) -> Result<()> {
    let account = storage::resolve_account_by_id(account_id)?;
    terminal::debug(
        "kcordclient::watchdog",
        format!("starting watchdog runtime for account {}", account_id),
    );
    let discord = build_discord_client(&account.token, false, 2, None)?;
    let me = discord.me().await?;
    let pid = std::process::id();
    let mut reconnect_delay = RECONNECT_DELAY;

    loop {
        let rules = storage::active_watchdogs_for_account(account_id)?;
        if rules.is_empty() {
            storage::clear_watchdog_runner(account_id)?;
            return Ok(());
        }
        storage::update_watchdog_runner(account_id, pid, None)?;
        let connected_at = Instant::now();
        if let Err(error) =
            run_gateway_session(account_id, &account.token, &me.id, &discord, rules).await
        {
            storage::update_watchdog_runner(account_id, pid, Some(&format!("{error:#}")))?;
            eprintln!(
                "watchdog gateway reconnect in {}s: {error:#}",
                reconnect_delay.as_secs()
            );
            time::sleep(reconnect_delay).await;
            reconnect_delay = if connected_at.elapsed() >= Duration::from_secs(60) {
                RECONNECT_DELAY
            } else {
                reconnect_delay.saturating_mul(2).min(MAX_RECONNECT_DELAY)
            };
        }
    }
}

async fn run_gateway_session(
    account_id: i64,
    token: &str,
    my_id: &str,
    discord: &DiscordClient,
    mut rules: Vec<storage::StoredWatchdog>,
) -> Result<()> {
    let (socket, _) = connect_async(GATEWAY_URL)
        .await
        .context("gateway connection failed")?;
    let (mut write, mut read) = socket.split();
    terminal::debug(
        "kcordclient::watchdog",
        format!("websocket connected for account {}", account_id),
    );
    let hello = next_envelope(&mut read).await?;
    if hello.op != 10 {
        bail!("gateway did not send hello");
    }
    let heartbeat_ms = hello
        .d
        .get("heartbeat_interval")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("gateway hello omitted heartbeat interval"))?;
    send_gateway(
        &mut write,
        2,
        serde_json::json!({
            "token": token,
            "intents": 37376,
            "properties": { "os": std::env::consts::OS, "browser": "kclient", "device": "kclient" }
        }),
    )
    .await?;

    let mut heartbeat = time::interval(Duration::from_millis(heartbeat_ms));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut control = time::interval(CONTROL_INTERVAL);
    control.set_missed_tick_behavior(MissedTickBehavior::Delay);
    control.tick().await;
    let mut sequence: Option<u64> = None;
    let mut next_due = storage::next_watchdog_due_at()?;

    loop {
        let due_wait = duration_until(next_due);
        tokio::select! {
            _ = heartbeat.tick() => {
                send_gateway(&mut write, 1, sequence.map(Value::from).unwrap_or(Value::Null)).await?;
            }
            _ = control.tick() => {
                rules = storage::active_watchdogs_for_account(account_id)?;
                storage::update_watchdog_runner(account_id, std::process::id(), None)?;
                if rules.is_empty() {
                    let _ = write.send(WsMessage::Close(None)).await;
                    return Ok(());
                }
                next_due = storage::next_watchdog_due_at()?;
            }
            _ = time::sleep(due_wait) => {
                process_due(my_id, discord, &rules).await?;
                next_due = storage::next_watchdog_due_at()?;
            }
            packet = read.next() => {
                let packet = packet.ok_or_else(|| anyhow!("gateway connection closed"))??;
                let WsMessage::Text(text) = packet else { continue; };
                let envelope: GatewayEnvelope = serde_json::from_str(&text).context("invalid gateway payload")?;
                if let Some(value) = envelope.s { sequence = Some(value); }
                match envelope.op {
                    0 if envelope.t.as_deref() == Some("MESSAGE_CREATE") => {
                        let message: Message = serde_json::from_value(envelope.d).context("invalid MESSAGE_CREATE payload")?;
                        if message.author.id == my_id {
                            if let Some(queued_due) = queue_matching(&rules, &message)? {
                                next_due = Some(next_due.map_or(queued_due, |due| due.min(queued_due)));
                            }
                        }
                    }
                    1 => send_gateway(&mut write, 1, sequence.map(Value::from).unwrap_or(Value::Null)).await?,
                    7 | 9 => bail!("gateway requested reconnect"),
                    _ => {}
                }
            }
        }
    }
}

async fn process_due(
    my_id: &str,
    discord: &DiscordClient,
    rules: &[storage::StoredWatchdog],
) -> Result<()> {
    let by_name = rules
        .iter()
        .map(|rule| (rule.name.as_str(), rule))
        .collect::<HashMap<_, _>>();
    terminal::debug(
        "kcordclient::watchdog",
        format!("evaluating {} due candidates", rules.len()),
    );
    for candidate in storage::due_watchdog_candidates(Utc::now().timestamp())? {
        let Some(rule) = by_name.get(candidate.watchdog_name.as_str()) else {
            storage::remove_watchdog_candidate(
                &candidate.watchdog_name,
                &candidate.channel_id,
                &candidate.message_id,
            )?;
            terminal::debug(
                "kcordclient::watchdog",
                format!("dropping stale candidate {}", candidate.message_id),
            );
            continue;
        };
        let message = match discord
            .message(&candidate.channel_id, &candidate.message_id)
            .await
        {
            Ok(message) => message,
            Err(error) => {
                eprintln!(
                    "watchdog could not refresh {}: {error:#}",
                    candidate.message_id
                );
                storage::reschedule_watchdog_candidate(
                    &candidate.watchdog_name,
                    &candidate.channel_id,
                    &candidate.message_id,
                    Utc::now().timestamp() + FAILED_DELETE_RETRY_DELAY.as_secs() as i64,
                )?;
                continue;
            }
        };
        if message.author.id != my_id || !matches_rule(rule, &message, false) {
            terminal::debug(
                "kcordclient::watchdog",
                format!(
                    "candidate {} no longer matches rule {}",
                    candidate.message_id, candidate.watchdog_name
                ),
            );
            storage::remove_watchdog_candidate(
                &candidate.watchdog_name,
                &candidate.channel_id,
                &candidate.message_id,
            )?;
            continue;
        }
        let owned = owned_message_metadata(&message);
        let delete_result = discord
            .delete_messages(
                &candidate.channel_id,
                &candidate.channel_id,
                &[owned],
                None,
                false,
                |_| Ok(()),
            )
            .await;
        match delete_result {
            Ok(()) => storage::remove_watchdog_candidate(
                &candidate.watchdog_name,
                &candidate.channel_id,
                &candidate.message_id,
            )?,
            Err(error) => {
                eprintln!(
                    "watchdog could not delete {}: {error:#}",
                    candidate.message_id
                );
                storage::reschedule_watchdog_candidate(
                    &candidate.watchdog_name,
                    &candidate.channel_id,
                    &candidate.message_id,
                    Utc::now().timestamp() + FAILED_DELETE_RETRY_DELAY.as_secs() as i64,
                )?;
            }
        }
    }
    Ok(())
}

fn owned_message_metadata(message: &Message) -> OwnedMessage {
    let has_video = message.attachments.iter().any(attachment_has_video);
    let has_media = !message.attachments.is_empty();
    OwnedMessage {
        id: message.id.clone(),
        timestamp: message.timestamp,
        content: message.content.clone(),
        has_link: content_has_link(&message.content),
        has_media,
        has_file: has_media
            && message
                .attachments
                .iter()
                .any(|attachment| !attachment_has_video(attachment)),
        has_video,
    }
}

fn attachment_has_video(attachment: &crate::models::Attachment) -> bool {
    if let Some(content_type) = attachment.content_type.as_deref() {
        if content_type.to_ascii_lowercase().starts_with("video/") {
            return true;
        }
    }

    let Some(filename) = attachment.filename.as_deref() else {
        return false;
    };

    let extension = filename
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "mp4"
            | "mov"
            | "webm"
            | "mkv"
            | "avi"
            | "m4v"
            | "flv"
            | "gifv"
            | "m4p"
            | "mpeg"
            | "mpg"
            | "m2ts"
            | "wmv"
            | "ogv"
            | "3gp"
            | "ts",
    )
}

fn content_has_link(content: &str) -> bool {
    let lowered = content.to_ascii_lowercase();
    lowered.contains("http://")
        || lowered.contains("https://")
        || lowered.contains("www.")
        || lowered.contains("discordapp.com")
        || lowered.contains("discord.com")
}

fn queue_matching(rules: &[storage::StoredWatchdog], message: &Message) -> Result<Option<i64>> {
    let channel_id = message
        .channel_id
        .as_deref()
        .ok_or_else(|| anyhow!("gateway message omitted channel id"))?;
    let normalized_content = message.content.to_lowercase();
    let mut candidates = Vec::new();
    for rule in rules {
        if !rule.requested_running {
            continue;
        }
        if matches_rule_with_content(rule, message, &normalized_content, true) {
            candidates.push(storage::WatchdogCandidate {
                watchdog_name: rule.name.clone(),
                channel_id: channel_id.to_string(),
                message_id: message.id.clone(),
                due_at: message.timestamp.timestamp() + rule.delay_seconds,
            });
        }
    }
    let earliest_due = candidates.iter().map(|candidate| candidate.due_at).min();
    terminal::debug(
        "kcordclient::watchdog",
        format!(
            "gateway message {} matched {} watchdog(s)",
            message.id,
            candidates.len()
        ),
    );
    storage::queue_watchdog_candidates(&candidates)?;
    Ok(earliest_due)
}

fn matches_rule(rule: &storage::StoredWatchdog, message: &Message, check_scope: bool) -> bool {
    let normalized_content = message.content.to_lowercase();
    matches_rule_with_content(rule, message, &normalized_content, check_scope)
}

fn matches_rule_with_content(
    rule: &storage::StoredWatchdog,
    message: &Message,
    normalized_content: &str,
    check_scope: bool,
) -> bool {
    if check_scope && !scope_matches(rule, message) {
        return false;
    }
    let has_flag = normalized_content.contains(&rule.off_flag);
    let selected = match rule.kind.as_str() {
        "media" => !message.attachments.is_empty(),
        "message" => rule
            .words
            .iter()
            .any(|word| normalized_content.contains(word)),
        _ => false,
    };
    selected && if rule.default_on { !has_flag } else { has_flag }
}

fn scope_matches(rule: &storage::StoredWatchdog, message: &Message) -> bool {
    match rule.scope_kind.as_str() {
        "global" => true,
        "server" => message.guild_id.as_deref() == rule.scope_id.as_deref(),
        "channel" | "dm" => message.channel_id.as_deref() == rule.scope_id.as_deref(),
        _ => false,
    }
}

async fn next_envelope<S>(read: &mut S) -> Result<GatewayEnvelope>
where
    S: futures_util::Stream<
            Item = std::result::Result<WsMessage, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    loop {
        let packet = read
            .next()
            .await
            .ok_or_else(|| anyhow!("gateway connection closed"))??;
        if let WsMessage::Text(text) = packet {
            return serde_json::from_str(&text).context("invalid gateway payload");
        }
    }
}

async fn send_gateway<S>(write: &mut S, op: i64, data: Value) -> Result<()>
where
    S: futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let payload = serde_json::to_string(&serde_json::json!({ "op": op, "d": data }))?;
    write.send(WsMessage::Text(payload)).await?;
    Ok(())
}

fn duration_until(next_due: Option<i64>) -> Duration {
    match next_due {
        Some(timestamp) => Duration::from_secs((timestamp - Utc::now().timestamp()).max(0) as u64),
        None => Duration::from_secs(60 * 60 * 24),
    }
}

fn watchdog_kind_name(kind: WatchdogType) -> &'static str {
    match kind {
        WatchdogType::Media => "media",
        WatchdogType::Message => "message",
        _ => unreachable!("unsupported watchdog type"),
    }
}

fn validate_name(value: &str) -> Result<()> {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        Ok(())
    } else {
        bail!("watchdog name must use only letters, numbers, '-' or '_'")
    }
}

fn validate_snowflake(value: &str, flag: &str) -> Result<()> {
    if !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()) {
        Ok(())
    } else {
        bail!("{flag} must be a Discord snowflake ID")
    }
}

fn load_words(paths: &[String]) -> Result<Vec<String>> {
    let mut words = Vec::new();
    for path in paths {
        words.extend(parse_word_list(Path::new(path))?);
    }
    words.sort();
    words.dedup();
    if words.is_empty() {
        bail!("word list is empty")
    }
    Ok(words)
}

fn parse_word_list(path: &Path) -> Result<Vec<String>> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let raw = match extension.as_str() {
        "txt" => source
            .lines()
            .filter_map(|line| {
                let value = line.trim();
                (!value.is_empty() && !value.starts_with('#')).then_some(value.to_string())
            })
            .collect(),
        "json" => parse_structured_words(
            serde_json::from_str(&source).context("invalid JSON word list")?,
        )?,
        "toml" => {
            parse_structured_words(toml::from_str(&source).context("invalid TOML word list")?)?
        }
        "yaml" | "yml" => parse_structured_words(
            serde_yaml::from_str(&source).context("invalid YAML word list")?,
        )?,
        _ => bail!(
            "unsupported word-list extension for {}; use .txt, .json, .toml, .yaml, or .yml",
            path.display()
        ),
    };
    Ok(raw
        .into_iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect())
}

fn parse_structured_words(value: Value) -> Result<Vec<String>> {
    let values = match value {
        Value::Array(values) => values,
        Value::Object(mut object) => object
            .remove("words")
            .and_then(|value| value.as_array().cloned())
            .ok_or_else(|| {
                anyhow!("structured word list must be an array or an object with a words array")
            })?,
        _ => bail!("structured word list must be an array or an object with a words array"),
    };
    values
        .into_iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("word-list entries must be strings"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn message(content: &str, media: bool) -> Message {
        Message {
            id: "1".into(),
            channel_id: Some("c".into()),
            guild_id: Some("g".into()),
            author: crate::models::Author { id: "me".into() },
            timestamp: Utc.timestamp_opt(1, 0).unwrap(),
            content: content.into(),
            attachments: if media {
                vec![crate::models::Attachment {
                    filename: Some("media.bin".into()),
                    content_type: Some("application/octet-stream".into()),
                }]
            } else {
                vec![]
            },
        }
    }

    #[test]
    fn matches_media_and_message_defaults() {
        let media = storage::StoredWatchdog {
            name: "m".into(),
            account_id: 1,
            scope_kind: "global".into(),
            scope_id: None,
            kind: "media".into(),
            delay_seconds: 60,
            off_flag: ".".into(),
            default_on: true,
            words: vec![],
            requested_running: true,
        };
        assert!(matches_rule(&media, &message("photo", true), true));
        assert!(!matches_rule(&media, &message("photo.", true), true));
        let words = storage::StoredWatchdog {
            name: "w".into(),
            account_id: 1,
            scope_kind: "global".into(),
            scope_id: None,
            kind: "message".into(),
            delay_seconds: 60,
            off_flag: "dnd".into(),
            default_on: false,
            words: vec!["remove".into()],
            requested_running: true,
        };
        assert!(matches_rule(&words, &message("remove dnd", false), true));
        assert!(!matches_rule(&words, &message("remove", false), true));
    }

    #[test]
    fn parses_supported_word_list_formats() {
        let root = std::env::temp_dir().join(format!(
            "kclient-watchdog-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time ok")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let cases = [
            ("words.txt", "# comment\nAlpha\nBeta\n"),
            ("words.json", "{\"words\":[\"Alpha\",\"Beta\"]}"),
            ("words.toml", "words = [\"Alpha\", \"Beta\"]"),
            ("words.yaml", "words:\n  - Alpha\n  - Beta\n"),
        ];
        for (name, contents) in cases {
            let path = root.join(name);
            fs::write(&path, contents).expect("write word list");
            assert_eq!(
                parse_word_list(&path).expect("parse word list"),
                vec!["alpha", "beta"]
            );
        }
        fs::remove_dir_all(root).ok();
    }
}
