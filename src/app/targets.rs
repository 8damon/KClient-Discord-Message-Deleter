use anyhow::{bail, Result};

use crate::{args::Args, discord::DiscordClient, models::Channel};

pub(crate) struct Target {
    pub(crate) kind: TargetKind,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) prompt_name: String,
}

pub(crate) enum TargetKind {
    Server,
    Channel,
    Dm,
}

pub(crate) async fn resolve_target(discord: &DiscordClient, args: &Args) -> Result<Target> {
    match (&args.server, &args.channel, &args.dm) {
        (Some(server_id), None, None) => {
            validate_discord_id(server_id, "--server")?;
            let guild = discord.guild(server_id).await?;
            Ok(Target {
                kind: TargetKind::Server,
                id: server_id.clone(),
                name: guild.name.clone(),
                prompt_name: guild.name,
            })
        }
        (None, Some(channel_id), None) => {
            validate_discord_id(channel_id, "--channel")?;
            let channel = discord.channel(channel_id).await?;
            let channel_name = channel.name.unwrap_or_else(|| channel_id.clone());
            let prompt_name = if let Some(guild_id) = &channel.guild_id {
                let guild = discord.guild(guild_id).await?;
                format!("#{} in {}", channel_name, guild.name)
            } else {
                format!("#{}", channel_name)
            };
            Ok(Target {
                kind: TargetKind::Channel,
                id: channel_id.clone(),
                name: channel_name,
                prompt_name,
            })
        }
        (None, None, Some(user_id)) => {
            validate_discord_id(user_id, "--dm")?;
            let label = format!("DM with user {}", user_id);
            Ok(Target {
                kind: TargetKind::Dm,
                id: user_id.clone(),
                name: label.clone(),
                prompt_name: label,
            })
        }
        _ => bail!("exactly one of --server, --channel, or --dm is required"),
    }
}

pub(crate) async fn resolve_channels(
    discord: &DiscordClient,
    target: &Target,
) -> Result<Vec<(String, String)>> {
    match target.kind {
        TargetKind::Server => resolve_server_channels(discord, &target.id, &target.name).await,
        TargetKind::Channel => Ok(vec![(target.id.clone(), format!("#{}", target.name))]),
        TargetKind::Dm => {
            let dm_channel = discord.create_dm(&target.id).await?;
            let display_name = describe_dm_channel(&dm_channel, &target.id);
            Ok(vec![(dm_channel.id, display_name)])
        }
    }
}

fn describe_dm_channel(channel: &Channel, fallback_user_id: &str) -> String {
    let Some(recipient) = channel
        .recipients
        .iter()
        .find(|recipient| recipient.id == fallback_user_id)
        .or_else(|| channel.recipients.first())
    else {
        return format!("DM with user {}", fallback_user_id);
    };

    if let Some(global_name) = recipient
        .global_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != recipient.username)
    {
        format!("DM with {} (@{})", global_name, recipient.username)
    } else {
        format!("DM with @{}", recipient.username)
    }
}

async fn resolve_server_channels(
    discord: &DiscordClient,
    guild_id: &str,
    guild_name: &str,
) -> Result<Vec<(String, String)>> {
    Ok(discord
        .guild_channels(guild_id)
        .await?
        .into_iter()
        .filter(is_text_channel)
        .map(|channel| {
            let name = channel.name.unwrap_or_else(|| channel.id.clone());
            (channel.id, format!("#{} in {}", name, guild_name))
        })
        .collect())
}

fn is_text_channel(channel: &Channel) -> bool {
    matches!(channel.kind, 0 | 5 | 10 | 11 | 12)
}

fn validate_discord_id(value: &str, flag_name: &str) -> Result<()> {
    if !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()) {
        Ok(())
    } else {
        bail!("{flag_name} must be a Discord snowflake ID")
    }
}
