use std::{fs, path::PathBuf, sync::OnceLock};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{models::OwnedMessage, paths, secure};

const DB_NAME: &str = "kcordclient.sqlite3";
const DB_PATH_OVERRIDE: &str = "KCLIENT_DB_PATH";

static DB_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccount {
    pub id: i64,
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub created_at: String,
    pub last_used_at: String,
}

#[derive(Debug, Clone)]
pub struct StoredAccountToken {
    pub account: StoredAccount,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct StoredWatchdog {
    pub name: String,
    pub account_id: i64,
    pub scope_kind: String,
    pub scope_id: Option<String>,
    pub kind: String,
    pub delay_seconds: i64,
    pub off_flag: String,
    pub default_on: bool,
    pub words: Vec<String>,
    pub requested_running: bool,
}

#[derive(Debug, Clone)]
pub struct WatchdogCandidate {
    pub watchdog_name: String,
    pub channel_id: String,
    pub message_id: String,
    pub due_at: i64,
}

#[derive(Debug, Clone)]
pub struct WatchdogRunner {
    pub account_id: i64,
    pub pid: i64,
    pub heartbeat_at: i64,
    pub last_error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct PersistedCheckpoint {
    channel_id: String,
    cutoff_key: String,
    found: Vec<PersistedMessage>,
    deleted_ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct PersistedMessage {
    id: String,
    timestamp: DateTime<Utc>,
    #[serde(default)]
    content: String,
    #[serde(default)]
    has_link: bool,
    #[serde(default)]
    has_media: bool,
    #[serde(default)]
    has_file: bool,
    #[serde(default)]
    has_video: bool,
}

pub fn initialize() -> Result<()> {
    paths::ensure_app_dirs()?;
    let db_path = current_db_path();
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create data dir {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok();
        }
    }
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open database {}", db_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600)).ok();
    }
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS accounts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL UNIQUE,
            username TEXT NOT NULL,
            display_name TEXT,
            token BLOB NOT NULL,
            created_at TEXT NOT NULL,
            last_used_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS checkpoints (
            channel_id TEXT NOT NULL,
            cutoff_key TEXT NOT NULL,
            found_json TEXT NOT NULL,
            deleted_ids_json TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (channel_id, cutoff_key)
        );
        CREATE TABLE IF NOT EXISTS watchdogs (
            name TEXT PRIMARY KEY,
            account_id INTEGER NOT NULL,
            scope_kind TEXT NOT NULL,
            scope_id TEXT,
            kind TEXT NOT NULL,
            delay_seconds INTEGER NOT NULL,
            off_flag TEXT NOT NULL,
            default_on INTEGER NOT NULL,
            words_json TEXT NOT NULL,
            requested_running INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS watchdog_candidates (
            watchdog_name TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            due_at INTEGER NOT NULL,
            PRIMARY KEY (watchdog_name, channel_id, message_id)
        );
        CREATE INDEX IF NOT EXISTS idx_watchdog_candidates_due
            ON watchdog_candidates (due_at);
        CREATE TABLE IF NOT EXISTS watchdog_runners (
            account_id INTEGER PRIMARY KEY,
            pid INTEGER NOT NULL,
            heartbeat_at INTEGER NOT NULL,
            last_error TEXT
        );
        ",
    )
    .context("failed to initialize schema")?;
    Ok(())
}

pub fn save_account(
    user_id: &str,
    username: &str,
    display_name: Option<&str>,
    token: &str,
) -> Result<StoredAccount> {
    initialize()?;
    let now = Local::now().to_rfc3339();
    let previous = find_account_by_user_id(user_id)?
        .and_then(|account| load_token_blob(account.id).ok().flatten());
    let encrypted = secure::protect_string(user_id, token)?;
    let conn = open()?;
    conn.execute(
        "
        INSERT INTO accounts (user_id, username, display_name, token, created_at, last_used_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        ON CONFLICT(user_id) DO UPDATE SET
            username = excluded.username,
            display_name = excluded.display_name,
            token = excluded.token,
            last_used_at = excluded.last_used_at
        ",
        params![user_id, username, display_name, encrypted, now],
    )
    .context("failed to save account")?;
    if let Some(previous_blob) = previous.as_deref() {
        secure::delete_protected(user_id, previous_blob).ok();
    }
    find_account_by_user_id(user_id)?
        .ok_or_else(|| anyhow!("saved account but failed to reload it"))
}

pub fn list_accounts() -> Result<Vec<StoredAccount>> {
    initialize()?;
    let conn = open()?;
    let mut stmt = conn.prepare(
        "
        SELECT id, user_id, username, display_name, created_at, last_used_at
        FROM accounts
        ORDER BY COALESCE(display_name, username), user_id
        ",
    )?;
    let rows = stmt.query_map([], map_account)?;
    let mut accounts = Vec::new();
    for row in rows {
        accounts.push(row?);
    }
    Ok(accounts)
}

pub fn resolve_account(selector: Option<&str>) -> Result<StoredAccountToken> {
    initialize()?;
    let accounts = list_accounts()?;
    let account = match selector {
        Some(value) => find_account_by_selector(accounts, value)?,
        None => match accounts.as_slice() {
            [only] => only.clone(),
            [] => {
                return Err(anyhow!(
                    "no stored accounts.\n\
store one first with:\n\
  kclient --add-account --token TOKEN_HERE\n\
or run once without storing with:\n\
  kclient --token TOKEN_HERE --delete --channel CHANNEL_ID --tf 24h"
                ))
            }
            _ => {
                return Err(anyhow!(
                    "multiple stored accounts found.\n\
pick one with:\n\
  kclient --account USERNAME_OR_USER_ID --delete --server SERVER_ID --tf 24h"
                ))
            }
        },
    };

    let token_blob = load_token_blob(account.id)?
        .ok_or_else(|| anyhow!("account {} has no token blob", account.user_id))?;
    mark_account_used(account.id)?;

    let token = secure::unprotect_string(&account.user_id, &token_blob)?;

    Ok(StoredAccountToken { account, token })
}

pub fn remove_account(selector: &str) -> Result<StoredAccount> {
    initialize()?;
    let account = find_account_by_selector(list_accounts()?, selector)?;
    if let Some(token_blob) = load_token_blob(account.id)? {
        secure::delete_protected(&account.user_id, &token_blob).ok();
    }
    let conn = open()?;
    conn.execute("DELETE FROM accounts WHERE id = ?1", params![account.id])
        .with_context(|| format!("failed to remove account {}", account.user_id))?;
    Ok(account)
}

pub fn cleanup_account_secrets() -> Result<usize> {
    initialize()?;
    let accounts = list_accounts()?;
    let mut removed = 0;

    for account in accounts {
        if let Some(token_blob) = load_token_blob(account.id)? {
            if secure::delete_protected(&account.user_id, &token_blob).is_ok() {
                removed += 1;
            }
        }
    }

    Ok(removed)
}

pub fn save_watchdog(watchdog: &StoredWatchdog) -> Result<()> {
    initialize()?;
    let words_json =
        serde_json::to_string(&watchdog.words).context("failed to encode watchdog word list")?;
    let now = Local::now().to_rfc3339();
    let conn = open()?;
    conn.execute(
        "
        INSERT INTO watchdogs (
            name, account_id, scope_kind, scope_id, kind, delay_seconds,
            off_flag, default_on, words_json, requested_running, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?10)
        ON CONFLICT(name) DO UPDATE SET
            account_id = excluded.account_id,
            scope_kind = excluded.scope_kind,
            scope_id = excluded.scope_id,
            kind = excluded.kind,
            delay_seconds = excluded.delay_seconds,
            off_flag = excluded.off_flag,
            default_on = excluded.default_on,
            words_json = excluded.words_json,
            updated_at = excluded.updated_at
        ",
        params![
            watchdog.name,
            watchdog.account_id,
            watchdog.scope_kind,
            watchdog.scope_id,
            watchdog.kind,
            watchdog.delay_seconds,
            watchdog.off_flag,
            i64::from(watchdog.default_on),
            words_json,
            now,
        ],
    )
    .context("failed to save watchdog")?;
    Ok(())
}

pub fn load_watchdog(name: &str) -> Result<Option<StoredWatchdog>> {
    initialize()?;
    let conn = open()?;
    conn.query_row(
        "
        SELECT name, account_id, scope_kind, scope_id, kind, delay_seconds,
               off_flag, default_on, words_json, requested_running
        FROM watchdogs WHERE name = ?1
        ",
        params![name],
        map_watchdog,
    )
    .optional()
    .context("failed to load watchdog")
}

pub fn list_watchdogs() -> Result<Vec<StoredWatchdog>> {
    initialize()?;
    let conn = open()?;
    let mut stmt = conn.prepare(
        "
        SELECT name, account_id, scope_kind, scope_id, kind, delay_seconds,
               off_flag, default_on, words_json, requested_running
        FROM watchdogs ORDER BY name
        ",
    )?;
    let result = stmt
        .query_map([], map_watchdog)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to list watchdogs");
    result
}

pub fn active_watchdogs_for_account(account_id: i64) -> Result<Vec<StoredWatchdog>> {
    initialize()?;
    let conn = open()?;
    let mut stmt = conn.prepare(
        "
        SELECT name, account_id, scope_kind, scope_id, kind, delay_seconds,
               off_flag, default_on, words_json, requested_running
        FROM watchdogs WHERE account_id = ?1 AND requested_running = 1
        ORDER BY name
        ",
    )?;
    let result = stmt
        .query_map(params![account_id], map_watchdog)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load active watchdogs");
    result
}

pub fn set_watchdog_running(name: &str, running: bool) -> Result<StoredWatchdog> {
    initialize()?;
    let conn = open()?;
    let changed = conn.execute(
        "UPDATE watchdogs SET requested_running = ?2, updated_at = ?3 WHERE name = ?1",
        params![name, i64::from(running), Local::now().to_rfc3339()],
    )?;
    if changed == 0 {
        return Err(anyhow!("no saved watchdog named `{name}`"));
    }
    load_watchdog(name)?.ok_or_else(|| anyhow!("saved watchdog `{name}` disappeared"))
}

pub fn delete_watchdog(name: &str) -> Result<()> {
    initialize()?;
    let conn = open()?;
    let changed = conn.execute("DELETE FROM watchdogs WHERE name = ?1", params![name])?;
    if changed == 0 {
        return Err(anyhow!("no saved watchdog named `{name}`"));
    }
    conn.execute(
        "DELETE FROM watchdog_candidates WHERE watchdog_name = ?1",
        params![name],
    )?;
    Ok(())
}

pub fn queue_watchdog_candidates(candidates: &[WatchdogCandidate]) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    initialize()?;
    let mut conn = open()?;
    let tx = conn.transaction()?;
    {
        let mut statement = tx.prepare_cached(
            "
            INSERT INTO watchdog_candidates (watchdog_name, channel_id, message_id, due_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(watchdog_name, channel_id, message_id) DO UPDATE SET due_at = excluded.due_at
            ",
        )?;
        for candidate in candidates {
            statement.execute(params![
                candidate.watchdog_name,
                candidate.channel_id,
                candidate.message_id,
                candidate.due_at
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn remove_watchdog_candidate(name: &str, channel_id: &str, message_id: &str) -> Result<()> {
    initialize()?;
    open()?.execute(
        "DELETE FROM watchdog_candidates WHERE watchdog_name = ?1 AND channel_id = ?2 AND message_id = ?3",
        params![name, channel_id, message_id],
    )?;
    Ok(())
}

pub fn remove_watchdog_candidates(name: &str) -> Result<()> {
    initialize()?;
    open()?.execute(
        "DELETE FROM watchdog_candidates WHERE watchdog_name = ?1",
        params![name],
    )?;
    Ok(())
}

pub fn reschedule_watchdog_candidate(
    name: &str,
    channel_id: &str,
    message_id: &str,
    due_at: i64,
) -> Result<()> {
    initialize()?;
    open()?.execute(
        "
        UPDATE watchdog_candidates SET due_at = ?4
        WHERE watchdog_name = ?1 AND channel_id = ?2 AND message_id = ?3
        ",
        params![name, channel_id, message_id, due_at],
    )?;
    Ok(())
}

pub fn stop_all_watchdogs() -> Result<()> {
    initialize()?;
    let conn = open()?;
    conn.execute("UPDATE watchdogs SET requested_running = 0", [])?;
    conn.execute("DELETE FROM watchdog_candidates", [])?;
    Ok(())
}

pub fn due_watchdog_candidates(now: i64) -> Result<Vec<WatchdogCandidate>> {
    initialize()?;
    let conn = open()?;
    let mut stmt = conn.prepare(
        "
        SELECT watchdog_name, channel_id, message_id, due_at
        FROM watchdog_candidates WHERE due_at <= ?1 ORDER BY due_at LIMIT 100
        ",
    )?;
    let result = stmt
        .query_map(params![now], |row| {
            Ok(WatchdogCandidate {
                watchdog_name: row.get(0)?,
                channel_id: row.get(1)?,
                message_id: row.get(2)?,
                due_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load due watchdog candidates");
    result
}

pub fn next_watchdog_due_at() -> Result<Option<i64>> {
    initialize()?;
    open()?
        .query_row("SELECT MIN(due_at) FROM watchdog_candidates", [], |row| {
            row.get(0)
        })
        .context("failed to load next watchdog due time")
}

pub fn update_watchdog_runner(account_id: i64, pid: u32, last_error: Option<&str>) -> Result<()> {
    initialize()?;
    open()?.execute(
        "
        INSERT INTO watchdog_runners (account_id, pid, heartbeat_at, last_error)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(account_id) DO UPDATE SET
            pid = excluded.pid, heartbeat_at = excluded.heartbeat_at, last_error = excluded.last_error
        ",
        params![account_id, i64::from(pid), Utc::now().timestamp(), last_error],
    )?;
    Ok(())
}

pub fn runner_is_fresh(account_id: i64, max_age_secs: i64) -> Result<bool> {
    initialize()?;
    let heartbeat = open()?
        .query_row(
            "SELECT heartbeat_at FROM watchdog_runners WHERE account_id = ?1",
            params![account_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(heartbeat.is_some_and(|value| value >= Utc::now().timestamp() - max_age_secs))
}

pub fn clear_watchdog_runner(account_id: i64) -> Result<()> {
    initialize()?;
    open()?.execute(
        "DELETE FROM watchdog_runners WHERE account_id = ?1",
        params![account_id],
    )?;
    Ok(())
}

pub fn list_watchdog_runners() -> Result<Vec<WatchdogRunner>> {
    initialize()?;
    let conn = open()?;
    let mut stmt = conn.prepare(
        "
        SELECT account_id, pid, heartbeat_at, last_error
        FROM watchdog_runners ORDER BY account_id
        ",
    )?;
    let result = stmt
        .query_map([], |row| {
            Ok(WatchdogRunner {
                account_id: row.get(0)?,
                pid: row.get(1)?,
                heartbeat_at: row.get(2)?,
                last_error: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to list watchdog runners");
    result
}

pub fn upsert_checkpoint(
    channel_id: &str,
    cutoff_key: &str,
    found: &[OwnedMessage],
    deleted_ids: &[String],
) -> Result<()> {
    initialize()?;
    let payload = PersistedCheckpoint {
        channel_id: channel_id.to_string(),
        cutoff_key: cutoff_key.to_string(),
        found: found
            .iter()
            .map(|message| PersistedMessage {
                id: message.id.clone(),
                timestamp: message.timestamp,
                content: message.content.clone(),
                has_link: message.has_link,
                has_media: message.has_media,
                has_file: message.has_file,
                has_video: message.has_video,
            })
            .collect(),
        deleted_ids: deleted_ids.to_vec(),
    };
    let found_json =
        serde_json::to_string(&payload.found).context("failed to encode checkpoint messages")?;
    let deleted_json = serde_json::to_string(&payload.deleted_ids)
        .context("failed to encode checkpoint deletions")?;
    let conn = open()?;
    conn.execute(
        "
        INSERT INTO checkpoints (channel_id, cutoff_key, found_json, deleted_ids_json, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(channel_id, cutoff_key) DO UPDATE SET
            found_json = excluded.found_json,
            deleted_ids_json = excluded.deleted_ids_json,
            updated_at = excluded.updated_at
        ",
        params![
            channel_id,
            cutoff_key,
            found_json,
            deleted_json,
            Local::now().to_rfc3339()
        ],
    )
    .context("failed to save checkpoint")?;
    Ok(())
}

pub fn load_checkpoint(
    channel_id: &str,
    cutoff_key: &str,
) -> Result<Option<(Vec<OwnedMessage>, Vec<String>)>> {
    initialize()?;
    let conn = open()?;
    let row = conn
        .query_row(
            "
            SELECT found_json, deleted_ids_json
            FROM checkpoints
            WHERE channel_id = ?1 AND cutoff_key = ?2
            ",
            params![channel_id, cutoff_key],
            |row| {
                let found_json: String = row.get(0)?;
                let deleted_json: String = row.get(1)?;
                Ok((found_json, deleted_json))
            },
        )
        .optional()?;

    let Some((found_json, deleted_json)) = row else {
        return Ok(None);
    };

    let found = serde_json::from_str::<Vec<PersistedMessage>>(&found_json)
        .context("failed to decode checkpoint messages")?
        .into_iter()
        .map(|message| OwnedMessage {
            id: message.id,
            timestamp: message.timestamp,
            content: message.content,
            has_link: message.has_link,
            has_media: message.has_media,
            has_file: message.has_file,
            has_video: message.has_video,
        })
        .collect();
    let deleted_ids = serde_json::from_str::<Vec<String>>(&deleted_json)
        .context("failed to decode checkpoint deleted ids")?;
    Ok(Some((found, deleted_ids)))
}

pub fn remove_checkpoint(channel_id: &str, cutoff_key: &str) -> Result<()> {
    initialize()?;
    let conn = open()?;
    conn.execute(
        "DELETE FROM checkpoints WHERE channel_id = ?1 AND cutoff_key = ?2",
        params![channel_id, cutoff_key],
    )?;
    Ok(())
}

pub fn clear_checkpoints() -> Result<()> {
    initialize()?;
    let conn = open()?;
    conn.execute("DELETE FROM checkpoints", [])?;
    Ok(())
}

fn default_db_path() -> PathBuf {
    paths::data_dir().join(DB_NAME)
}

fn current_db_path() -> PathBuf {
    if let Some(path) = std::env::var_os(DB_PATH_OVERRIDE) {
        return PathBuf::from(path);
    }
    DB_PATH.get_or_init(default_db_path).clone()
}

fn open() -> Result<Connection> {
    let path = current_db_path();
    Connection::open(&path).with_context(|| format!("failed to open database {}", path.display()))
}

fn map_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAccount> {
    Ok(StoredAccount {
        id: row.get(0)?,
        user_id: row.get(1)?,
        username: row.get(2)?,
        display_name: row.get(3)?,
        created_at: row.get(4)?,
        last_used_at: row.get(5)?,
    })
}

fn map_watchdog(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredWatchdog> {
    let words_json: String = row.get(8)?;
    let words = serde_json::from_str(&words_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(StoredWatchdog {
        name: row.get(0)?,
        account_id: row.get(1)?,
        scope_kind: row.get(2)?,
        scope_id: row.get(3)?,
        kind: row.get(4)?,
        delay_seconds: row.get(5)?,
        off_flag: row.get(6)?,
        default_on: row.get::<_, i64>(7)? != 0,
        words,
        requested_running: row.get::<_, i64>(9)? != 0,
    })
}

fn matches_selector(account: &StoredAccount, selector: &str) -> bool {
    account.user_id.eq_ignore_ascii_case(selector)
        || account.username.eq_ignore_ascii_case(selector)
        || account
            .display_name
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case(selector))
            .unwrap_or(false)
}

fn find_account_by_selector(accounts: Vec<StoredAccount>, selector: &str) -> Result<StoredAccount> {
    let needle = selector.trim().to_ascii_lowercase();
    accounts
        .into_iter()
        .find(|account| matches_selector(account, &needle))
        .ok_or_else(|| anyhow!("no stored account matched `{selector}`"))
}

fn find_account_by_user_id(user_id: &str) -> Result<Option<StoredAccount>> {
    let conn = open()?;
    conn.query_row(
        "
        SELECT id, user_id, username, display_name, created_at, last_used_at
        FROM accounts
        WHERE user_id = ?1
        ",
        params![user_id],
        map_account,
    )
    .optional()
    .context("failed to fetch account")
}

fn load_token_blob(account_id: i64) -> Result<Option<Vec<u8>>> {
    let conn = open()?;
    conn.query_row(
        "SELECT token FROM accounts WHERE id = ?1",
        params![account_id],
        |row| row.get(0),
    )
    .optional()
    .context("failed to fetch account token")
}

pub fn resolve_account_by_id(account_id: i64) -> Result<StoredAccountToken> {
    initialize()?;
    let conn = open()?;
    let account = conn
        .query_row(
            "
            SELECT id, user_id, username, display_name, created_at, last_used_at
            FROM accounts WHERE id = ?1
            ",
            params![account_id],
            map_account,
        )
        .optional()?
        .ok_or_else(|| anyhow!("watchdog account {account_id} no longer exists"))?;
    let token_blob = load_token_blob(account.id)?
        .ok_or_else(|| anyhow!("watchdog account {} has no token blob", account.user_id))?;
    let token = secure::unprotect_string(&account.user_id, &token_blob)?;
    Ok(StoredAccountToken { account, token })
}

fn mark_account_used(account_id: i64) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "UPDATE accounts SET last_used_at = ?2 WHERE id = ?1",
        params![account_id, Local::now().to_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[test]
    fn initialize_creates_database_in_overridden_location() {
        let _guard = TEST_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("lock test env");
        let sandbox = unique_test_dir("storage-init");
        let app_root = sandbox.join("app-root");
        let db_path = sandbox.join("db").join("kcordclient.sqlite3");

        fs::create_dir_all(&sandbox).expect("create sandbox");
        std::env::set_var("KCLIENT_APP_ROOT", &app_root);
        std::env::set_var("KCLIENT_DB_PATH", &db_path);

        initialize().expect("initialize storage");

        assert!(app_root.is_dir(), "app root should exist");
        assert!(app_root.join("data").exists() || db_path.parent().is_some());
        assert!(db_path.is_file(), "database file should exist");

        std::env::remove_var("KCLIENT_APP_ROOT");
        std::env::remove_var("KCLIENT_DB_PATH");
        fs::remove_dir_all(&sandbox).ok();
    }

    #[test]
    fn save_and_resolve_account_round_trip() {
        let _guard = TEST_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("lock test env");
        let sandbox = unique_test_dir("storage-account");
        let app_root = sandbox.join("app-root");
        let db_path = sandbox.join("db").join("kcordclient.sqlite3");

        fs::create_dir_all(&sandbox).expect("create sandbox");
        std::env::set_var("KCLIENT_APP_ROOT", &app_root);
        std::env::set_var("KCLIENT_DB_PATH", &db_path);

        let saved =
            save_account("123", "damon", Some("8damon"), "token-abc").expect("save account");
        let resolved = resolve_account(Some("damon")).expect("resolve account");

        assert_eq!(saved.user_id, "123");
        assert_eq!(resolved.account.user_id, "123");
        assert_eq!(resolved.account.username, "damon");
        assert_eq!(resolved.token, "token-abc");

        std::env::remove_var("KCLIENT_APP_ROOT");
        std::env::remove_var("KCLIENT_DB_PATH");
        fs::remove_dir_all(&sandbox).ok();
    }

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time ok")
            .as_nanos();
        std::env::temp_dir().join(format!("kclient-{prefix}-{}-{stamp}", std::process::id()))
    }
}
