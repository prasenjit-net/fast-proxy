//! One maintenance scope, ready to answer requests: its response bodies are
//! built once at compile time, its bypass rules parsed, and [`check`] is the
//! single pure decision the data plane and the admin resolver share, so they
//! cannot disagree (ADR 0018).
//!
//! [`check`]: CompiledMaintenance::check

use std::net::IpAddr;

use fast_proxy_auth::bypass_header_matches;
use fast_proxy_model::{BodyContentType, Cidr, MaintenanceSettings, SecretHash};

/// Shown on the built-in page and in JSON when the operator wrote no message.
const DEFAULT_MESSAGE: &str = "This site is down for maintenance. Please try again soon.";

/// What a scope covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaintenanceScope {
    Global,
    Subsystem,
    Route,
}

impl MaintenanceScope {
    /// The name used in metric labels, the bypass header and the API.
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Subsystem => "subsystem",
            Self::Route => "route",
        }
    }
}

/// How a request prefers to be answered, from its `Accept` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    Json,
    Html,
    Text,
}

/// What [`CompiledMaintenance::check`] decided for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceCheck {
    /// The scope is over (its expiry passed): serve as usual.
    Inactive,
    /// The scope is on but this request may pass: serve as usual and say so.
    Bypassed,
    /// Answer with the maintenance response.
    Respond,
}

/// A response body with its content type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedBody {
    pub content_type: &'static str,
    pub text: String,
}

/// The bypass header's lowercase name and the hash of its secret.
#[derive(Debug, Clone)]
struct BypassHeaderRule {
    name: String,
    secret_hash: SecretHash,
}

/// An active maintenance scope compiled into the snapshot.
#[derive(Debug, Clone)]
pub struct CompiledMaintenance {
    pub scope: MaintenanceScope,
    settings: MaintenanceSettings,
    cidrs: Vec<Cidr>,
    header: Option<BypassHeaderRule>,
    html: String,
    text: String,
    /// The message as a JSON string literal, quotes included.
    message_json: String,
}

impl CompiledMaintenance {
    pub fn new(scope: MaintenanceScope, settings: MaintenanceSettings) -> Self {
        let message = if settings.message.trim().is_empty() {
            DEFAULT_MESSAGE
        } else {
            settings.message.as_str()
        };
        Self {
            scope,
            cidrs: settings.bypass.cidrs.clone(),
            header: settings
                .bypass
                .header
                .as_ref()
                .map(|header| BypassHeaderRule {
                    name: header.name.to_ascii_lowercase(),
                    secret_hash: header.secret_hash.clone(),
                }),
            html: built_in_page(message),
            text: format!("{message}\n"),
            message_json: json_string(message),
            settings,
        }
    }

    /// The request header whose value may let a request through, if any (lowercase).
    pub fn bypass_header_name(&self) -> Option<&str> {
        self.header.as_ref().map(|header| header.name.as_str())
    }

    /// The decision for one request at `now_ms`: `client` is the effective
    /// client address (the peer, or the forwarded address of a trusted proxy)
    /// and `header_value` the value of [`bypass_header_name`](Self::bypass_header_name)
    /// on the request, if it carries one.
    pub fn check(
        &self,
        now_ms: i64,
        client: IpAddr,
        header_value: Option<&str>,
    ) -> MaintenanceCheck {
        if !self.settings.is_active(now_ms) {
            return MaintenanceCheck::Inactive;
        }
        let by_address = self.cidrs.iter().any(|cidr| cidr.contains(client));
        let by_header = self
            .header
            .as_ref()
            .is_some_and(|rule| bypass_header_matches(header_value, &rule.secret_hash));
        if by_address || by_header {
            MaintenanceCheck::Bypassed
        } else {
            MaintenanceCheck::Respond
        }
    }

    /// The `Retry-After` seconds to send at `now_ms`.
    pub fn retry_after(&self, now_ms: i64) -> u32 {
        self.settings.retry_after(now_ms)
    }

    /// The body to send: the operator's own when one is set (for every
    /// request), else the built-in page in the requested format.
    pub fn render(&self, format: ResponseFormat, now_ms: i64) -> RenderedBody {
        if let Some(body) = &self.settings.body {
            return RenderedBody {
                content_type: body.content_type.as_str(),
                text: body.text.clone(),
            };
        }
        match format {
            ResponseFormat::Html => RenderedBody {
                content_type: BodyContentType::Html.as_str(),
                text: self.html.clone(),
            },
            ResponseFormat::Text => RenderedBody {
                content_type: "text/plain; charset=utf-8",
                text: self.text.clone(),
            },
            ResponseFormat::Json => RenderedBody {
                content_type: BodyContentType::Json.as_str(),
                text: format!(
                    "{{\"error\":{{\"code\":\"maintenance\",\"message\":{},\"retryAfterSecs\":{}}}}}",
                    self.message_json,
                    self.retry_after(now_ms)
                ),
            },
        }
    }

    /// The settings this scope was compiled from.
    pub fn settings(&self) -> &MaintenanceSettings {
        &self.settings
    }

    /// When the scope ends, if it does.
    pub fn expires_at_ms(&self) -> Option<i64> {
        self.settings.expires_at_ms
    }
}

/// The page shown to the public. The message is operator text, so it is
/// escaped: it can never become markup.
fn built_in_page(message: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Down for maintenance</title></head>\
         <body style=\"font-family:system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem\">\
         <h1>Down for maintenance</h1><p>{}</p></body></html>",
        escape_html(message)
    )
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `text` as a JSON string literal.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use fast_proxy_auth::GeneratedSecret;
    use fast_proxy_model::{BypassHeader, BypassRules, CustomBody};

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

    fn compiled(settings: MaintenanceSettings) -> CompiledMaintenance {
        CompiledMaintenance::new(MaintenanceScope::Global, settings)
    }

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap()
    }

    fn cidr(raw: &str) -> Cidr {
        Cidr::parse("bypass.cidrs", raw).unwrap()
    }

    #[test]
    fn an_active_scope_with_no_bypass_answers_everyone() {
        let m = compiled(settings());
        assert_eq!(
            m.check(0, ip("203.0.113.9"), None),
            MaintenanceCheck::Respond
        );
    }

    #[test]
    fn it_is_inactive_from_the_exact_millisecond_of_its_expiry() {
        let mut s = settings();
        s.expires_at_ms = Some(1_000);
        let m = compiled(s);
        assert_eq!(m.check(999, ip("1.1.1.1"), None), MaintenanceCheck::Respond);
        assert_eq!(
            m.check(1_000, ip("1.1.1.1"), None),
            MaintenanceCheck::Inactive
        );
    }

    #[test]
    fn a_listed_network_passes_and_others_do_not() {
        let mut s = settings();
        s.bypass.cidrs = vec![cidr("10.0.0.0/8"), cidr("2001:db8::/32")];
        let m = compiled(s);
        assert_eq!(m.check(0, ip("10.9.9.9"), None), MaintenanceCheck::Bypassed);
        assert_eq!(
            m.check(0, ip("2001:db8::7"), None),
            MaintenanceCheck::Bypassed
        );
        assert_eq!(m.check(0, ip("11.0.0.1"), None), MaintenanceCheck::Respond);
        assert_eq!(
            m.check(0, ip("2001:db9::1"), None),
            MaintenanceCheck::Respond
        );
    }

    #[test]
    fn the_secret_header_passes_only_with_the_right_value() {
        let secret = GeneratedSecret::generate().unwrap();
        let mut s = settings();
        s.bypass.header = Some(BypassHeader {
            name: "X-Maintenance-Pass".into(),
            secret_hash: secret.hash().clone(),
        });
        let m = compiled(s);
        assert_eq!(m.bypass_header_name(), Some("x-maintenance-pass"));
        let who = ip("203.0.113.9");
        assert_eq!(
            m.check(0, who, Some(secret.value())),
            MaintenanceCheck::Bypassed
        );
        assert_eq!(m.check(0, who, Some("wrong")), MaintenanceCheck::Respond);
        assert_eq!(m.check(0, who, None), MaintenanceCheck::Respond);
    }

    #[test]
    fn either_rule_alone_is_enough() {
        let secret = GeneratedSecret::generate().unwrap();
        let mut s = settings();
        s.bypass.cidrs = vec![cidr("10.0.0.0/8")];
        s.bypass.header = Some(BypassHeader {
            name: "X-Pass".into(),
            secret_hash: secret.hash().clone(),
        });
        let m = compiled(s);
        assert_eq!(m.check(0, ip("10.0.0.1"), None), MaintenanceCheck::Bypassed);
        assert_eq!(
            m.check(0, ip("203.0.113.9"), Some(secret.value())),
            MaintenanceCheck::Bypassed
        );
        assert_eq!(
            m.check(0, ip("203.0.113.9"), Some("no")),
            MaintenanceCheck::Respond
        );
    }

    #[test]
    fn no_header_rule_means_no_header_name() {
        assert_eq!(compiled(settings()).bypass_header_name(), None);
    }

    #[test]
    fn the_message_is_escaped_in_the_page_so_it_cannot_become_markup() {
        let mut s = settings();
        s.message = "<script>alert(1)</script> & \"quotes\" 'x'".into();
        let m = compiled(s);
        let page = m.render(ResponseFormat::Html, 0);
        assert_eq!(page.content_type, "text/html");
        assert!(!page.text.contains("<script>"));
        assert!(page.text.contains(
            "&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quotes&quot; &#39;x&#39;"
        ));
    }

    #[test]
    fn json_names_the_code_the_message_and_the_retry_after() {
        let mut s = settings();
        s.message = "line\n\"two\"".into();
        s.retry_after_secs = Some(45);
        let body = compiled(s).render(ResponseFormat::Json, 0);
        assert_eq!(body.content_type, "application/json");
        assert_eq!(
            body.text,
            r#"{"error":{"code":"maintenance","message":"line\n\"two\"","retryAfterSecs":45}}"#
        );
    }

    #[test]
    fn the_derived_retry_after_follows_the_clock_in_json() {
        let mut s = settings();
        s.expires_at_ms = Some(60_000);
        let m = compiled(s);
        assert!(m
            .render(ResponseFormat::Json, 0)
            .text
            .contains("\"retryAfterSecs\":60"));
        assert!(m
            .render(ResponseFormat::Json, 30_000)
            .text
            .contains("\"retryAfterSecs\":30"));
        assert_eq!(m.retry_after(30_000), 30);
        assert_eq!(m.expires_at_ms(), Some(60_000));
    }

    #[test]
    fn plain_text_is_the_message_and_an_empty_message_gets_a_default() {
        let text = compiled(settings()).render(ResponseFormat::Text, 0);
        assert_eq!(text.text, "back soon\n");
        assert!(text.content_type.starts_with("text/plain"));
        let mut s = settings();
        s.message = "  ".into();
        let m = compiled(s);
        assert!(m
            .render(ResponseFormat::Text, 0)
            .text
            .contains("maintenance"));
        assert!(m
            .render(ResponseFormat::Json, 0)
            .text
            .contains("maintenance"));
    }

    #[test]
    fn a_custom_body_wins_for_every_format() {
        let mut s = settings();
        s.body = Some(CustomBody {
            content_type: BodyContentType::Html,
            text: "<h1>custom</h1>".into(),
        });
        let m = compiled(s);
        for format in [
            ResponseFormat::Json,
            ResponseFormat::Html,
            ResponseFormat::Text,
        ] {
            let body = m.render(format, 0);
            assert_eq!(body.text, "<h1>custom</h1>");
            assert_eq!(body.content_type, "text/html");
        }
    }

    #[test]
    fn json_strings_escape_control_characters() {
        assert_eq!(json_string("a\u{1}b\tc\\"), "\"a\\u0001b\\tc\\\\\"");
    }

    #[test]
    fn scopes_have_stable_labels() {
        assert_eq!(MaintenanceScope::Global.label(), "global");
        assert_eq!(MaintenanceScope::Subsystem.label(), "subsystem");
        assert_eq!(MaintenanceScope::Route.label(), "route");
    }
}
