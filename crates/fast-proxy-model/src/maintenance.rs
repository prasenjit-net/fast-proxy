use crate::{Cidr, FieldError, RouteId, SecretHash, SubsystemId};

/// Longest operator message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 1024;
/// Largest custom response body, in bytes.
pub const MAX_BODY_BYTES: usize = 16 * 1024;
/// Most bypass networks one scope may list.
pub const MAX_BYPASS_CIDRS: usize = 32;
/// Longest `Retry-After` accepted, in seconds (one day).
pub const MAX_RETRY_AFTER_SECS: u32 = 86_400;
/// `Retry-After` when neither a value nor a TTL gives one.
pub const DEFAULT_RETRY_AFTER_SECS: u32 = 120;
const MAX_HEADER_NAME_LEN: usize = 64;

/// What a maintenance scope covers. Subsystem and route scopes are removed
/// with their target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaintenanceTarget {
    /// The whole proxy.
    Global,
    Subsystem(SubsystemId),
    Route(RouteId),
}

/// The content types a custom maintenance body may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyContentType {
    Html,
    Text,
    Json,
}

impl BodyContentType {
    /// The MIME type: `text/html`, `text/plain` or `application/json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Html => "text/html",
            Self::Text => "text/plain",
            Self::Json => "application/json",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "text/html" => Ok(Self::Html),
            "text/plain" => Ok(Self::Text),
            "application/json" => Ok(Self::Json),
            _ => Err(FieldError::new(
                "contentType",
                "must be text/html, text/plain or application/json",
            )),
        }
    }
}

/// An operator-supplied response body replacing the built-in page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomBody {
    pub content_type: BodyContentType,
    pub text: String,
}

/// A request header whose secret value lets a request through maintenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BypassHeader {
    pub name: String,
    pub secret_hash: SecretHash,
}

/// Who may pass through a maintenance scope while it is active.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BypassRules {
    pub cidrs: Vec<Cidr>,
    pub header: Option<BypassHeader>,
}

/// One active maintenance scope. Times are unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceSettings {
    /// Shown on the built-in page; may be empty.
    pub message: String,
    pub body: Option<CustomBody>,
    /// `None` derives it from the expiry, else [`DEFAULT_RETRY_AFTER_SECS`].
    pub retry_after_secs: Option<u32>,
    pub bypass: BypassRules,
    /// `None` lasts until turned off.
    pub expires_at_ms: Option<i64>,
    pub set_by: String,
    pub set_at_ms: i64,
}

impl MaintenanceSettings {
    /// Checks every bound; errors name the field in the API's spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        if self.message.chars().count() > MAX_MESSAGE_CHARS {
            return Err(FieldError::new(
                "message",
                format!("must be at most {MAX_MESSAGE_CHARS} characters"),
            ));
        }
        if let Some(body) = &self.body {
            if body.text.len() > MAX_BODY_BYTES {
                return Err(FieldError::new(
                    "body",
                    format!("must be at most {MAX_BODY_BYTES} bytes"),
                ));
            }
        }
        if let Some(secs) = self.retry_after_secs {
            if !(1..=MAX_RETRY_AFTER_SECS).contains(&secs) {
                return Err(FieldError::new(
                    "retryAfterSecs",
                    format!("must be between 1 and {MAX_RETRY_AFTER_SECS}"),
                ));
            }
        }
        if self.bypass.cidrs.len() > MAX_BYPASS_CIDRS {
            return Err(FieldError::new(
                "bypass.cidrs",
                format!("must list at most {MAX_BYPASS_CIDRS} networks"),
            ));
        }
        if let Some(header) = &self.bypass.header {
            validate_header_name(&header.name)?;
        }
        Ok(())
    }

    /// Whether the scope still applies at `now_ms`. It ends at the exact
    /// millisecond of the expiry.
    pub fn is_active(&self, now_ms: i64) -> bool {
        self.expires_at_ms.is_none_or(|expires| now_ms < expires)
    }

    /// The `Retry-After` seconds to send at `now_ms`: the configured value,
    /// else the time left until expiry rounded up (at least 1, at most a day),
    /// else [`DEFAULT_RETRY_AFTER_SECS`].
    pub fn retry_after(&self, now_ms: i64) -> u32 {
        if let Some(secs) = self.retry_after_secs {
            return secs;
        }
        let Some(expires) = self.expires_at_ms else {
            return DEFAULT_RETRY_AFTER_SECS;
        };
        let left_ms = expires.saturating_sub(now_ms).max(0);
        let secs = (left_ms + 999) / 1000;
        u32::try_from(secs.clamp(1, i64::from(MAX_RETRY_AFTER_SECS)))
            .unwrap_or(MAX_RETRY_AFTER_SECS)
    }
}

/// An HTTP header name: RFC 9110 token characters, at most 64.
pub(crate) fn validate_header_name(name: &str) -> Result<(), FieldError> {
    const TOKEN_PUNCTUATION: &str = "!#$%&'*+-.^_`|~";
    let valid = !name.is_empty()
        && name.len() <= MAX_HEADER_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || TOKEN_PUNCTUATION.contains(c));
    if valid {
        return Ok(());
    }
    Err(FieldError::new(
        "bypass.header",
        "must be a valid header name of at most 64 characters",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> MaintenanceSettings {
        MaintenanceSettings {
            message: "back soon".into(),
            body: None,
            retry_after_secs: None,
            bypass: BypassRules::default(),
            expires_at_ms: None,
            set_by: "ops".into(),
            set_at_ms: 0,
        }
    }

    fn rejected_field(settings: &MaintenanceSettings) -> String {
        settings.validate().unwrap_err().field
    }

    #[test]
    fn defaults_are_valid() {
        assert!(settings().validate().is_ok());
    }

    #[test]
    fn message_length_is_bounded_in_characters() {
        let mut s = settings();
        s.message = "é".repeat(MAX_MESSAGE_CHARS);
        assert!(s.validate().is_ok());
        s.message.push('x');
        assert_eq!(rejected_field(&s), "message");
    }

    #[test]
    fn body_size_is_bounded_in_bytes() {
        let mut s = settings();
        let body = |len| CustomBody {
            content_type: BodyContentType::Html,
            text: "a".repeat(len),
        };
        s.body = Some(body(MAX_BODY_BYTES));
        assert!(s.validate().is_ok());
        s.body = Some(body(MAX_BODY_BYTES + 1));
        assert_eq!(rejected_field(&s), "body");
    }

    #[test]
    fn retry_after_bounds_are_inclusive() {
        let mut s = settings();
        for ok in [1, MAX_RETRY_AFTER_SECS] {
            s.retry_after_secs = Some(ok);
            assert!(s.validate().is_ok(), "{ok}");
        }
        for bad in [0, MAX_RETRY_AFTER_SECS + 1] {
            s.retry_after_secs = Some(bad);
            assert_eq!(rejected_field(&s), "retryAfterSecs", "{bad}");
        }
    }

    #[test]
    fn at_most_32_bypass_networks() {
        let mut s = settings();
        let net = Cidr::parse("bypass.cidrs", "10.0.0.0/8").unwrap();
        s.bypass.cidrs = vec![net; MAX_BYPASS_CIDRS];
        assert!(s.validate().is_ok());
        s.bypass.cidrs.push(net);
        assert_eq!(rejected_field(&s), "bypass.cidrs");
    }

    #[test]
    fn bypass_header_names_must_be_http_tokens() {
        let mut s = settings();
        let header = |name: &str| {
            Some(BypassHeader {
                name: name.into(),
                secret_hash: SecretHash::new([1; 32]),
            })
        };
        for ok in ["X-Maintenance-Pass", "a", &"a".repeat(64)] {
            s.bypass.header = header(ok);
            assert!(s.validate().is_ok(), "{ok}");
        }
        let too_long = "a".repeat(65);
        for bad in ["", "has space", "bad:colon", "é", too_long.as_str()] {
            s.bypass.header = header(bad);
            assert_eq!(rejected_field(&s), "bypass.header", "{bad:?}");
        }
    }

    #[test]
    fn content_types_round_trip_and_reject_others() {
        for kind in [
            BodyContentType::Html,
            BodyContentType::Text,
            BodyContentType::Json,
        ] {
            assert_eq!(BodyContentType::parse(kind.as_str()).unwrap(), kind);
        }
        for bad in ["", "text/css", "TEXT/HTML"] {
            assert_eq!(
                BodyContentType::parse(bad).unwrap_err().field,
                "contentType"
            );
        }
    }

    #[test]
    fn without_an_expiry_it_is_always_active() {
        assert!(settings().is_active(i64::MAX));
    }

    #[test]
    fn it_ends_at_the_exact_millisecond_of_the_expiry() {
        let mut s = settings();
        s.expires_at_ms = Some(10_000);
        assert!(s.is_active(9_999));
        assert!(!s.is_active(10_000));
        assert!(!s.is_active(10_001));
    }

    #[test]
    fn retry_after_prefers_the_configured_value() {
        let mut s = settings();
        s.retry_after_secs = Some(7);
        s.expires_at_ms = Some(60_000);
        assert_eq!(s.retry_after(0), 7);
    }

    #[test]
    fn retry_after_derives_from_the_time_left_rounded_up() {
        let mut s = settings();
        s.expires_at_ms = Some(60_000);
        assert_eq!(s.retry_after(0), 60);
        assert_eq!(s.retry_after(59_001), 1);
        assert_eq!(s.retry_after(59_999), 1);
        assert_eq!(s.retry_after(60_000), 1, "never below one second");
        assert_eq!(
            s.retry_after(70_000),
            1,
            "past expiry still sends a valid value"
        );
    }

    #[test]
    fn retry_after_is_capped_at_a_day() {
        let mut s = settings();
        s.expires_at_ms = Some(10 * 86_400_000);
        assert_eq!(s.retry_after(0), MAX_RETRY_AFTER_SECS);
    }

    #[test]
    fn retry_after_defaults_without_value_or_expiry() {
        assert_eq!(settings().retry_after(0), DEFAULT_RETRY_AFTER_SECS);
    }
}
