//! Sampling settings: one global row and optional overrides per subsystem and
//! per route. They are not part of `Config` (a rollback never changes them),
//! like maintenance and tokens. See ADR 0020 to 0023.

use crate::bounds::check_range;
use crate::maintenance::validate_header_name;
use crate::{FieldError, RouteId, SubsystemId, TriggerSet};

/// Most names one redaction or header list may hold.
pub const MAX_LIST_NAMES: usize = 64;
/// Longest name in a redaction list.
pub const MAX_LIST_NAME_LEN: usize = 64;
/// Largest body cap, in bytes.
pub const MAX_BODY_CAP: u32 = 64 * 1024;
/// Most samples per second one instance may keep.
pub const MAX_RATE_PER_SEC: u32 = 10_000;
/// Longest retention, in days.
pub const MAX_RETENTION_DAYS: u32 = 365;
/// Most rows the shared table may be allowed to hold.
pub const MAX_ROWS_LIMIT: u64 = 10_000_000;
/// Fewest rows the table may be limited to.
pub const MIN_ROWS_LIMIT: u64 = 100;
/// Most bytes the shared table may be allowed to hold (16 GiB).
pub const MAX_BYTES_LIMIT: u64 = 16 * 1024 * 1024 * 1024;
/// Fewest bytes the table may be limited to (1 MiB).
pub const MIN_BYTES_LIMIT: u64 = 1024 * 1024;

/// The global sampling settings. Times are unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingSettings {
    /// Whether sampling is on anywhere that does not override it.
    pub enabled: bool,
    /// Which triggers are on.
    pub triggers: TriggerSet,
    /// The `slow` trigger's threshold.
    pub slow_threshold_ms: u32,
    /// The `slow_p99` trigger's factor, times ten (30 is 3.0).
    pub slow_p99_factor_x10: u32,
    /// The `slow_p99` trigger never fires below this latency.
    pub slow_p99_floor_ms: u32,
    /// The `client_errors` trigger's 4xx count per client per minute.
    pub client_errors_per_min: u32,
    /// The `oversized_request` threshold, bytes.
    pub oversized_request_bytes: u64,
    /// The `oversized_response` threshold, bytes.
    pub oversized_response_bytes: u64,
    /// The baseline sample, in parts per thousand (1 is 0.1%).
    pub baseline_per_mille: u32,
    /// Samples an instance may keep per second.
    pub rate_per_sec: u32,
    /// The rate limiter's burst.
    pub rate_burst: u32,
    /// The storm guard: samples kept per key before the interval applies.
    pub storm_burst: u32,
    /// The storm guard: one sample per key per this many seconds afterwards.
    pub storm_interval_secs: u32,
    /// Retention of the shared table, days.
    pub retention_days: u32,
    /// Retention of the shared table, rows.
    pub max_rows: u64,
    /// Retention of the shared table, bytes of stored samples.
    pub max_bytes: u64,
    /// The most request body bytes captured, where bodies are on.
    pub body_request_cap: u32,
    /// The most response body bytes captured, where bodies are on.
    pub body_response_cap: u32,
    /// Headers kept in a sample besides the built-in allow-list.
    pub extra_headers: Vec<String>,
    /// Header names redacted besides the built-in list. Additions only.
    pub redact_headers: Vec<String>,
    /// Query parameter names redacted besides the built-in list.
    pub redact_query_params: Vec<String>,
    /// Body field names redacted besides the built-in list.
    pub redact_body_fields: Vec<String>,
    pub updated_by: String,
    pub updated_at_ms: i64,
}

impl Default for SamplingSettings {
    /// The defaults of ADRs 0020 to 0023.
    fn default() -> Self {
        Self {
            enabled: true,
            triggers: TriggerSet::defaults(),
            slow_threshold_ms: 2000,
            slow_p99_factor_x10: 30,
            slow_p99_floor_ms: 50,
            client_errors_per_min: 100,
            oversized_request_bytes: 8 * 1024 * 1024,
            oversized_response_bytes: 32 * 1024 * 1024,
            baseline_per_mille: 1,
            rate_per_sec: 20,
            rate_burst: 40,
            storm_burst: 3,
            storm_interval_secs: 30,
            retention_days: 7,
            max_rows: 100_000,
            max_bytes: 256 * 1024 * 1024,
            body_request_cap: 4096,
            body_response_cap: 4096,
            extra_headers: Vec::new(),
            redact_headers: Vec::new(),
            redact_query_params: Vec::new(),
            redact_body_fields: Vec::new(),
            updated_by: "system".into(),
            updated_at_ms: 0,
        }
    }
}

impl SamplingSettings {
    /// Checks every bound; errors name the field in the API's spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        check_range("slowThresholdMs", self.slow_threshold_ms, 1, 3_600_000)?;
        check_range("slowP99FactorX10", self.slow_p99_factor_x10, 11, 1000)?;
        check_range("slowP99FloorMs", self.slow_p99_floor_ms, 1, 3_600_000)?;
        check_range(
            "clientErrorsPerMin",
            self.client_errors_per_min,
            1,
            1_000_000,
        )?;
        check_range("baselinePerMille", self.baseline_per_mille, 0, 1000)?;
        check_range("ratePerSec", self.rate_per_sec, 1, MAX_RATE_PER_SEC)?;
        check_range("rateBurst", self.rate_burst, 1, MAX_RATE_PER_SEC)?;
        check_range("stormBurst", self.storm_burst, 1, 100)?;
        check_range("stormIntervalSecs", self.storm_interval_secs, 1, 3600)?;
        check_range("retentionDays", self.retention_days, 1, MAX_RETENTION_DAYS)?;
        check_range("bodyRequestCap", self.body_request_cap, 1, MAX_BODY_CAP)?;
        check_range("bodyResponseCap", self.body_response_cap, 1, MAX_BODY_CAP)?;
        check_bytes("oversizedRequestBytes", self.oversized_request_bytes)?;
        check_bytes("oversizedResponseBytes", self.oversized_response_bytes)?;
        check_u64("maxRows", self.max_rows, MIN_ROWS_LIMIT, MAX_ROWS_LIMIT)?;
        check_u64("maxBytes", self.max_bytes, MIN_BYTES_LIMIT, MAX_BYTES_LIMIT)?;
        for (field, names) in [
            ("extraHeaders", &self.extra_headers),
            ("redactHeaders", &self.redact_headers),
        ] {
            check_list(field, names, |name| {
                validate_header_name(name).map_err(|_| header_message())
            })?;
        }
        for (field, names) in [
            ("redactQueryParams", &self.redact_query_params),
            ("redactBodyFields", &self.redact_body_fields),
        ] {
            check_list(field, names, check_plain_name)?;
        }
        Ok(())
    }
}

fn header_message() -> String {
    "each name must be a valid header name of at most 64 characters".into()
}

fn check_plain_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_LIST_NAME_LEN
        && !name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '=' | '&' | '"'));
    if valid {
        return Ok(());
    }
    Err(
        "each name must be 1 to 64 characters without spaces, control characters, '=', '&' or '\"'"
            .into(),
    )
}

fn check_list(
    field: &str,
    names: &[String],
    check_name: impl Fn(&str) -> Result<(), String>,
) -> Result<(), FieldError> {
    if names.len() > MAX_LIST_NAMES {
        return Err(FieldError::new(
            field,
            format!("must list at most {MAX_LIST_NAMES} names"),
        ));
    }
    for name in names {
        check_name(name).map_err(|message| FieldError::new(field, message))?;
    }
    Ok(())
}

fn check_u64(field: &str, value: u64, min: u64, max: u64) -> Result<(), FieldError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(FieldError::new(
        field,
        format!("must be between {min} and {max}"),
    ))
}

fn check_bytes(field: &str, value: u64) -> Result<(), FieldError> {
    check_u64(field, value, 1, 64 * 1024 * 1024 * 1024)
}

/// What an override applies to. Overrides are removed with their target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SamplingTarget {
    Subsystem(SubsystemId),
    Route(RouteId),
}

/// A subsystem's or route's override of the global settings. `None` inherits.
/// An override cannot touch redaction: only the global settings hold the lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingOverride {
    pub enabled: Option<bool>,
    /// Whether bodies are captured here. Off unless somebody turns it on.
    pub capture_bodies: Option<bool>,
    pub baseline_per_mille: Option<u32>,
    pub slow_threshold_ms: Option<u32>,
    pub slow_p99_factor_x10: Option<u32>,
    pub set_by: String,
    pub set_at_ms: i64,
}

impl SamplingOverride {
    pub fn validate(&self) -> Result<(), FieldError> {
        if let Some(value) = self.baseline_per_mille {
            check_range("baselinePerMille", value, 0, 1000)?;
        }
        if let Some(value) = self.slow_threshold_ms {
            check_range("slowThresholdMs", value, 1, 3_600_000)?;
        }
        if let Some(value) = self.slow_p99_factor_x10 {
            check_range("slowP99FactorX10", value, 11, 1000)?;
        }
        Ok(())
    }
}

/// What applies to one route after the overrides are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveSampling {
    pub enabled: bool,
    pub capture_bodies: bool,
    pub baseline_per_mille: u32,
    pub slow_threshold_ms: u32,
    pub slow_p99_factor_x10: u32,
}

/// Resolves the settings for one route: the route's override over its
/// subsystem's over the global settings; an empty field inherits.
pub fn effective(
    global: &SamplingSettings,
    subsystem: Option<&SamplingOverride>,
    route: Option<&SamplingOverride>,
) -> EffectiveSampling {
    let layers = [route, subsystem];
    let pick = |field: fn(&SamplingOverride) -> Option<u32>, fallback: u32| {
        layers
            .iter()
            .flatten()
            .find_map(|layer| field(layer))
            .unwrap_or(fallback)
    };
    let flag = |field: fn(&SamplingOverride) -> Option<bool>, fallback: bool| {
        layers
            .iter()
            .flatten()
            .find_map(|layer| field(layer))
            .unwrap_or(fallback)
    };
    EffectiveSampling {
        enabled: flag(|o| o.enabled, global.enabled),
        capture_bodies: flag(|o| o.capture_bodies, false),
        baseline_per_mille: pick(|o| o.baseline_per_mille, global.baseline_per_mille),
        slow_threshold_ms: pick(|o| o.slow_threshold_ms, global.slow_threshold_ms),
        slow_p99_factor_x10: pick(|o| o.slow_p99_factor_x10, global.slow_p99_factor_x10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(settings: &SamplingSettings) -> String {
        settings.validate().unwrap_err().field
    }

    fn empty_override() -> SamplingOverride {
        SamplingOverride {
            enabled: None,
            capture_bodies: None,
            baseline_per_mille: None,
            slow_threshold_ms: None,
            slow_p99_factor_x10: None,
            set_by: "ops".into(),
            set_at_ms: 1,
        }
    }

    #[test]
    fn the_defaults_are_valid_and_match_the_adrs() {
        let defaults = SamplingSettings::default();
        assert!(defaults.validate().is_ok());
        assert!(defaults.enabled);
        assert_eq!((defaults.rate_per_sec, defaults.rate_burst), (20, 40));
        assert_eq!(
            (defaults.storm_burst, defaults.storm_interval_secs),
            (3, 30)
        );
        assert_eq!(defaults.retention_days, 7);
        assert_eq!(defaults.max_rows, 100_000);
        assert_eq!(defaults.max_bytes, 256 * 1024 * 1024);
        assert_eq!(defaults.baseline_per_mille, 1);
        assert_eq!(
            (defaults.body_request_cap, defaults.body_response_cap),
            (4096, 4096)
        );
    }

    macro_rules! bound {
        ($name:ident, $field:ident, $api:literal, $low_ok:expr, $high_ok:expr, $low_bad:expr, $high_bad:expr) => {
            #[test]
            fn $name() {
                let mut s = SamplingSettings::default();
                for ok in [$low_ok, $high_ok] {
                    s.$field = ok;
                    assert!(s.validate().is_ok(), "{} should accept {}", $api, ok);
                }
                for bad in [$low_bad, $high_bad] {
                    s.$field = bad;
                    assert_eq!(rejected(&s), $api, "{} should reject {}", $api, bad);
                }
            }
        };
    }

    bound!(
        slow_threshold,
        slow_threshold_ms,
        "slowThresholdMs",
        1,
        3_600_000,
        0,
        3_600_001
    );
    bound!(
        p99_factor,
        slow_p99_factor_x10,
        "slowP99FactorX10",
        11,
        1000,
        10,
        1001
    );
    bound!(
        p99_floor,
        slow_p99_floor_ms,
        "slowP99FloorMs",
        1,
        3_600_000,
        0,
        3_600_001
    );
    bound!(
        client_errors,
        client_errors_per_min,
        "clientErrorsPerMin",
        1,
        1_000_000,
        0,
        1_000_001
    );
    bound!(
        baseline,
        baseline_per_mille,
        "baselinePerMille",
        0,
        1000,
        1001,
        5000
    );
    bound!(rate, rate_per_sec, "ratePerSec", 1, 10_000, 0, 10_001);
    bound!(burst, rate_burst, "rateBurst", 1, 10_000, 0, 10_001);
    bound!(storm_burst, storm_burst, "stormBurst", 1, 100, 0, 101);
    bound!(
        storm_interval,
        storm_interval_secs,
        "stormIntervalSecs",
        1,
        3600,
        0,
        3601
    );
    bound!(
        retention_days,
        retention_days,
        "retentionDays",
        1,
        365,
        0,
        366
    );
    bound!(
        request_cap,
        body_request_cap,
        "bodyRequestCap",
        1,
        65_536,
        0,
        65_537
    );
    bound!(
        response_cap,
        body_response_cap,
        "bodyResponseCap",
        1,
        65_536,
        0,
        65_537
    );
    bound!(max_rows, max_rows, "maxRows", 100, 10_000_000, 99, 10_000_001);
    bound!(
        max_bytes,
        max_bytes,
        "maxBytes",
        1024 * 1024,
        16 * 1024 * 1024 * 1024,
        1024 * 1024 - 1,
        16 * 1024 * 1024 * 1024 + 1
    );
    bound!(
        oversized_request,
        oversized_request_bytes,
        "oversizedRequestBytes",
        1,
        64 << 30,
        0,
        (64 << 30) + 1
    );
    bound!(
        oversized_response,
        oversized_response_bytes,
        "oversizedResponseBytes",
        1,
        64 << 30,
        0,
        (64 << 30) + 1
    );

    #[test]
    fn header_lists_take_header_names_only() {
        let mut s = SamplingSettings {
            redact_headers: vec!["X-Internal-Key".into(), "x-other".into()],
            extra_headers: vec!["x-tenant".into()],
            ..SamplingSettings::default()
        };
        assert!(s.validate().is_ok());
        for bad in ["", "has space", "bad:name", &"a".repeat(65)] {
            s.redact_headers = vec![bad.into()];
            assert_eq!(rejected(&s), "redactHeaders", "{bad:?}");
        }
        s.redact_headers.clear();
        s.extra_headers = vec!["bad name".into()];
        assert_eq!(rejected(&s), "extraHeaders");
    }

    #[test]
    fn query_and_body_names_reject_separators_and_control_characters() {
        let mut s = SamplingSettings {
            redact_query_params: vec!["my_token".into(), "X-Key".into()],
            redact_body_fields: vec!["cardNumber".into()],
            ..SamplingSettings::default()
        };
        assert!(s.validate().is_ok());
        for bad in ["", "a b", "a=b", "a&b", "a\"b", "a\nb", &"a".repeat(65)] {
            s.redact_query_params = vec![bad.into()];
            assert_eq!(rejected(&s), "redactQueryParams", "{bad:?}");
        }
        s.redact_query_params.clear();
        s.redact_body_fields = vec!["a=b".into()];
        assert_eq!(rejected(&s), "redactBodyFields");
    }

    #[test]
    fn a_list_may_hold_at_most_64_names() {
        let mut s = SamplingSettings {
            redact_body_fields: (0..64).map(|i| format!("f{i}")).collect(),
            ..SamplingSettings::default()
        };
        assert!(s.validate().is_ok());
        s.redact_body_fields.push("one_more".into());
        assert_eq!(rejected(&s), "redactBodyFields");
    }

    #[test]
    fn an_override_validates_only_what_it_sets() {
        assert!(empty_override().validate().is_ok());
        let mut o = empty_override();
        o.baseline_per_mille = Some(1001);
        assert_eq!(o.validate().unwrap_err().field, "baselinePerMille");
        o.baseline_per_mille = Some(1000);
        o.slow_threshold_ms = Some(0);
        assert_eq!(o.validate().unwrap_err().field, "slowThresholdMs");
        o.slow_threshold_ms = Some(1);
        o.slow_p99_factor_x10 = Some(10);
        assert_eq!(o.validate().unwrap_err().field, "slowP99FactorX10");
    }

    #[test]
    fn with_no_overrides_the_global_settings_apply_and_bodies_are_off() {
        let global = SamplingSettings::default();
        let effective = effective(&global, None, None);
        assert!(effective.enabled);
        assert!(!effective.capture_bodies);
        assert_eq!(effective.baseline_per_mille, 1);
        assert_eq!(effective.slow_threshold_ms, 2000);
        assert_eq!(effective.slow_p99_factor_x10, 30);
    }

    #[test]
    fn the_route_overrides_the_subsystem_which_overrides_the_global_settings() {
        let global = SamplingSettings::default();
        let mut subsystem = empty_override();
        subsystem.baseline_per_mille = Some(10);
        subsystem.slow_threshold_ms = Some(500);
        subsystem.capture_bodies = Some(true);
        let mut route = empty_override();
        route.baseline_per_mille = Some(0);
        route.enabled = Some(false);

        let both = effective(&global, Some(&subsystem), Some(&route));
        assert!(!both.enabled, "the route's own value wins");
        assert_eq!(both.baseline_per_mille, 0);
        assert_eq!(
            both.slow_threshold_ms, 500,
            "the route inherits from the subsystem"
        );
        assert!(both.capture_bodies, "bodies inherit from the subsystem");
        assert_eq!(
            both.slow_p99_factor_x10, 30,
            "and the rest from the global settings"
        );

        let only_subsystem = effective(&global, Some(&subsystem), None);
        assert!(only_subsystem.enabled);
        assert_eq!(only_subsystem.baseline_per_mille, 10);
    }

    #[test]
    fn a_route_can_turn_bodies_off_under_a_subsystem_that_turned_them_on() {
        let global = SamplingSettings::default();
        let mut subsystem = empty_override();
        subsystem.capture_bodies = Some(true);
        let mut route = empty_override();
        route.capture_bodies = Some(false);
        assert!(!effective(&global, Some(&subsystem), Some(&route)).capture_bodies);
    }
}
