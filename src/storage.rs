use std::{fs, path::PathBuf, sync::OnceLock};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{models::OwnedMessage, paths, secure};

const DB_NAME: &str = "kcordclient.sqlite3";

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
}

pub fn initialize() -> Result<()> {
    paths::ensure_app_dirs()?;
    let db_path = DB_PATH.get_or_init(default_db_path).clone();
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
            content: String::new(),
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

fn open() -> Result<Connection> {
    let path = DB_PATH.get_or_init(default_db_path);
    Connection::open(path).with_context(|| format!("failed to open database {}", path.display()))
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

fn mark_account_used(account_id: i64) -> Result<()> {
    let conn = open()?;
    conn.execute(
        "UPDATE accounts SET last_used_at = ?2 WHERE id = ?1",
        params![account_id, Local::now().to_rfc3339()],
    )?;
    Ok(())
}
