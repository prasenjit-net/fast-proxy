//! Reading durations written the way people write them (`"250ms"`, `"45s"`,
//! `"1h30m"`), in `config.toml` and on the command line.

use std::time::Duration;

use serde::{Deserialize, Deserializer};

/// Deserializes a human-readable duration (see [`parse_duration`]).
pub(super) fn deserialize_duration<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    let raw = String::deserialize(deserializer)?;
    parse_duration(&raw).map_err(serde::de::Error::custom)
}

/// Parses one or more `<whole number><unit>` terms, optionally separated by
/// spaces: `250ms`, `45s`, `10m`, `1h30m`, `2d 12h`. Units are `ms`, `s`, `m`,
/// `h` and `d`, or their long forms (`sec`, `minutes`, `hours`, `days`, ...).
pub fn parse_duration(raw: &str) -> Result<Duration, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("expected a duration such as 30s or 5m".into());
    }
    let mut total = Duration::ZERO;
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return Err(format!("{raw:?}: expected a number at {rest:?}"));
        }
        let (number, after) = rest.split_at(digits);
        let after = after.trim_start();
        let letters = after.len()
            - after
                .trim_start_matches(|c: char| c.is_ascii_alphabetic())
                .len();
        let (unit, after) = after.split_at(letters);
        let count: u64 = number
            .parse()
            .map_err(|_| format!("{raw:?}: {number} is too large"))?;
        let term = unit_duration(unit)
            .ok_or_else(|| {
                format!("{raw:?}: unknown unit {unit:?} (use ms, s, m, h or d, as in 30s)")
            })?
            .checked_mul(
                u32::try_from(count).map_err(|_| format!("{raw:?}: {number} is too large"))?,
            )
            .ok_or_else(|| format!("{raw:?}: too large"))?;
        total = total
            .checked_add(term)
            .ok_or_else(|| format!("{raw:?}: too large"))?;
        rest = after.trim_start();
    }
    Ok(total)
}

/// One of `unit`, or `None` for a unit that is not known.
fn unit_duration(unit: &str) -> Option<Duration> {
    let duration = match unit {
        "ms" | "msec" | "millis" | "millisecond" | "milliseconds" => Duration::from_millis(1),
        "s" | "sec" | "secs" | "second" | "seconds" => Duration::from_secs(1),
        "m" | "min" | "mins" | "minute" | "minutes" => Duration::from_secs(60),
        "h" | "hr" | "hrs" | "hour" | "hours" => Duration::from_secs(3600),
        "d" | "day" | "days" => Duration::from_secs(86_400),
        _ => return None,
    };
    Some(duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_the_documentation_uses_parses() {
        for (text, expected) in [
            ("0s", Duration::ZERO),
            ("250ms", Duration::from_millis(250)),
            ("45s", Duration::from_secs(45)),
            ("10m", Duration::from_secs(600)),
            ("1h", Duration::from_secs(3600)),
            ("2d", Duration::from_secs(172_800)),
            ("1h30m", Duration::from_secs(5400)),
            ("1h 30m 15s", Duration::from_secs(5415)),
            (" 5 minutes ", Duration::from_secs(300)),
            ("1day", Duration::from_secs(86_400)),
        ] {
            assert_eq!(parse_duration(text), Ok(expected), "{text}");
        }
    }

    #[test]
    fn a_bad_duration_says_what_is_wrong() {
        for (text, says) in [
            ("", "expected a duration"),
            ("30", "unknown unit"),
            ("5x", "unknown unit"),
            ("m5", "expected a number"),
            ("1.5s", "unknown unit"),
            ("-1s", "expected a number"),
            ("99999999999999999999s", "too large"),
            ("5000000000s", "too large"),
        ] {
            let error = parse_duration(text).unwrap_err();
            assert!(error.contains(says), "{text}: {error}");
        }
    }
}
