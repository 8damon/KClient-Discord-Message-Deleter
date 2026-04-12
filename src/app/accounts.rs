use anyhow::{anyhow, Context, Result};

use crate::{
    args::Args,
    models::Me,
    storage, terminal,
    token::{self, normalize_token},
};

use super::build_discord_client;

pub(crate) async fn add_account_from_args(args: &Args) -> Result<()> {
    let token = args
        .token
        .as_deref()
        .context("--token is required with --add-account")?;
    let token = normalize_token(token)?;
    token::validate_token(&token).await?;

    let discord = build_discord_client(&token, args.dproxy, 1, None)?;
    let me = discord.me().await?;
    let account = storage::save_account(&me.id, &me.username, None, &token)?;

    terminal::plain(format!(
        "Stored account {} ({})",
        account.display_name.unwrap_or(account.username),
        account.user_id
    ));

    Ok(())
}

pub(crate) fn remove_account_from_args(args: &Args) -> Result<()> {
    let selector = args
        .account
        .as_deref()
        .context("--account is required with --remove-account")?;
    let account = storage::remove_account(selector)?;

    terminal::plain(format!(
        "Removed account {} ({})",
        account.display_name.unwrap_or(account.username),
        account.user_id
    ));

    Ok(())
}

pub(crate) fn resolve_token_for_cli(args: &Args) -> Result<(String, Option<String>)> {
    if let Some(token) = args.token.as_deref() {
        return Ok((normalize_token(token)?, None));
    }

    if args.account.is_some()
        || storage::list_accounts()
            .map(|accounts| !accounts.is_empty())
            .unwrap_or(false)
    {
        let stored = storage::resolve_account(args.account.as_deref())?;
        let label = format_account_label(&stored.account);
        return Ok((stored.token, Some(label)));
    }

    Err(anyhow!(
        "no stored accounts found.\n\
use one of these exact commands:\n\
  kclient --add-account --token TOKEN_HERE\n\
  kclient --token TOKEN_HERE --delete --server SERVER_ID --tf 24h\n\
rules:\n\
  use double dashes like --token and --server\n\
  put the value after the option name"
    ))
}

pub(crate) fn format_account_identity(me: &Me) -> String {
    format!("@{} ({})", me.username, me.id)
}

fn format_account_label(account: &storage::StoredAccount) -> String {
    format!(
        "{} ({})",
        account
            .display_name
            .clone()
            .unwrap_or(account.username.clone()),
        account.user_id
    )
}
