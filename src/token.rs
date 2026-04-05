use anyhow::{bail, Context, Result};

use crate::discord::DiscordClient;

pub fn normalize_token(raw: &str) -> Result<String> {
    let mut value = raw.trim();
    if value.is_empty() {
        bail!("token is empty");
    }

    value = strip_authorization_wrapper(value);
    value = strip_matching_quotes(value);
    value = strip_angle_brackets(value);
    value = strip_matching_quotes(value);

    if value.is_empty() {
        bail!("token is empty after trimming wrappers");
    }

    if let Some(rest) = strip_scheme(value, "bot") {
        let token = normalize_bare_token(rest)?;
        return Ok(format!("Bot {}", token));
    }
    if let Some(rest) = strip_scheme(value, "bearer") {
        let token = normalize_bare_token(rest)?;
        return Ok(format!("Bearer {}", token));
    }

    Ok(normalize_bare_token(value)?.to_string())
}

pub async fn validate_token(token: &str) -> Result<()> {
    let client =
        DiscordClient::new(token, 1).context("failed to build client for token validation")?;
    client
        .me()
        .await
        .map(|_| ())
        .context("token validation failed")
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].trim();
        }
    }
    value
}

fn strip_authorization_wrapper(value: &str) -> &str {
    let trimmed = value.trim();
    let lowered = trimmed.to_ascii_lowercase();

    for prefix in ["authorization:", "authorization=", "token=", "token:"] {
        if lowered.starts_with(prefix) {
            return trimmed[prefix.len()..].trim();
        }
    }

    trimmed
}

fn strip_angle_brackets(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('<') && trimmed.ends_with('>') {
        return trimmed[1..trimmed.len() - 1].trim();
    }
    trimmed
}

fn strip_scheme<'a>(value: &'a str, scheme: &str) -> Option<&'a str> {
    let trimmed = value.trim();
    let lowered = trimmed.to_ascii_lowercase();
    let prefix = format!("{scheme} ");
    lowered
        .starts_with(&prefix)
        .then(|| trimmed[prefix.len()..].trim())
}

fn normalize_bare_token(value: &str) -> Result<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("token is empty");
    }
    if trimmed.chars().any(|ch| ch.is_ascii_control()) {
        bail!("token contains control characters");
    }
    if trimmed.chars().any(|ch| ch.is_whitespace()) {
        bail!("token contains whitespace");
    }
    Ok(trimmed)
}
