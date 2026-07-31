use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};

pub fn parse_timeframe(value: &str) -> Result<Duration> {
    let trimmed = value.trim().to_ascii_lowercase();
    if trimmed.len() < 2 {
        return Err(anyhow!(
            "invalid timeframe, use values like 24h, 1hr, 30m, or 7d"
        ));
    }

    let (number, seconds_per_unit) = [
        ("seconds", 1_i64),
        ("second", 1),
        ("secs", 1),
        ("sec", 1),
        ("minutes", 60),
        ("minute", 60),
        ("mins", 60),
        ("min", 60),
        ("hours", 60 * 60),
        ("hour", 60 * 60),
        ("hrs", 60 * 60),
        ("hr", 60 * 60),
        ("days", 60 * 60 * 24),
        ("day", 60 * 60 * 24),
        ("s", 1),
        ("m", 60),
        ("h", 60 * 60),
        ("d", 60 * 60 * 24),
    ]
    .into_iter()
    .find_map(|(suffix, seconds)| trimmed.strip_suffix(suffix).map(|number| (number, seconds)))
    .ok_or_else(|| anyhow!("unknown timeframe unit, use s, m, h, hr, or d"))?;
    let amount = number
        .parse::<i64>()
        .map_err(|_| anyhow!("invalid number in timeframe"))?;

    if amount <= 0 {
        return Err(anyhow!("timeframe must be greater than zero"));
    }

    Ok(Duration::seconds(
        amount
            .checked_mul(seconds_per_unit)
            .ok_or_else(|| anyhow!("timeframe is too large"))?,
    ))
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

#[cfg(test)]
mod tests {
    use super::parse_timeframe;

    #[test]
    fn accepts_long_timeframe_units() {
        assert_eq!(
            parse_timeframe("1hr").expect("parse hour").num_seconds(),
            3_600
        );
        assert_eq!(
            parse_timeframe("5min").expect("parse minute").num_seconds(),
            300
        );
    }
}
