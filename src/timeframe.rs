use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};

pub fn parse_timeframe(value: &str) -> Result<Duration> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.len() < 2 {
        return Err(anyhow!(
            "invalid timeframe, use values like 24h, 30m, or 7d"
        ));
    }

    let (number, unit) = trimmed.split_at(trimmed.len() - 1);
    let amount = number
        .parse::<i64>()
        .map_err(|_| anyhow!("invalid number in timeframe"))?;

    if amount <= 0 {
        return Err(anyhow!("timeframe must be greater than zero"));
    }

    match unit {
        "s" => Ok(Duration::seconds(amount)),
        "m" => Ok(Duration::minutes(amount)),
        "h" => Ok(Duration::hours(amount)),
        "d" => Ok(Duration::days(amount)),
        _ => Err(anyhow!("unknown timeframe unit, use s, m, h, or d")),
    }
}

pub fn resolve_cutoff(all: bool, timeframe: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    if all {
        return Ok(None);
    }

    let timeframe = timeframe.ok_or_else(|| anyhow!("must use --tf <time> or --all"))?;
    Ok(Some(Utc::now() - parse_timeframe(timeframe)?))
}

pub fn describe_timeframe(all: bool, timeframe: Option<&str>) -> String {
    if all {
        "ALL messages".to_string()
    } else {
        format!(
            "messages in the last {}",
            timeframe.expect("timeframe must exist when --all is false")
        )
    }
}
