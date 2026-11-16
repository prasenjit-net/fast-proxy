//! `fast-proxy acme status` and `fast-proxy acme renew <subsystem>` (M4, CL1):
//! what ACME is doing on a running proxy, and asking it to renew now.

use std::fmt::Write as _;

use serde_json::Value;

use crate::cli::AcmeRenewArgs;
use crate::commands::client::AdminClient;
use crate::commands::failure::{Failure, Outcome};
use crate::commands::remote::{duration_text, find_id, json_text};

/// The process settings, the lease and one line per subsystem.
pub async fn status(client: &AdminClient, json: bool, now_ms: i64) -> Outcome {
    let answer = client.get("/api/acme").await?;
    if json {
        return Ok(json_text(&answer));
    }
    Ok(describe(&answer["data"], now_ms))
}

/// Asks the proxy to order a certificate for the subsystem now.
pub async fn renew(client: &AdminClient, args: &AcmeRenewArgs) -> Outcome {
    let subsystems = client.get("/api/subsystems").await?;
    let id = find_id(&subsystems["data"], &args.subsystem, "name").ok_or_else(|| {
        Failure::Refused(format!("no subsystem \"{}\" (name or id)", args.subsystem))
    })?;
    client
        .post(&format!("/api/subsystems/{id}/acme/renew"))
        .await?;
    Ok(format!(
        "renewal requested for {}; follow it with `fast-proxy acme status`",
        args.subsystem
    ))
}

fn describe(data: &Value, now_ms: i64) -> String {
    if data["enabled"] != true {
        return "ACME is off for this proxy ([data_plane.acme] enabled)".to_string();
    }
    let mut out = String::new();
    let staging = if data["staging"] == true {
        " (staging: certificates are not publicly trusted)"
    } else {
        ""
    };
    let _ = writeln!(out, "directory: {}{staging}", text(&data["directory"]));
    let account = if data["accountRegistered"] == true {
        "registered"
    } else {
        "not yet (created on the first order)"
    };
    let _ = writeln!(out, "account: {account}");
    let _ = writeln!(out, "{}", lease_line(&data["lease"], now_ms));
    let subsystems = data["subsystems"].as_array().cloned().unwrap_or_default();
    if subsystems.is_empty() {
        out.push_str("no subsystem uses ACME yet");
    }
    for subsystem in &subsystems {
        let _ = writeln!(out, "{}", subsystem_line(subsystem, now_ms));
    }
    out.trim_end().to_string()
}

fn lease_line(lease: &Value, now_ms: i64) -> String {
    if lease.is_null() {
        return "lease: free".to_string();
    }
    let here = if lease["heldHere"] == true {
        " (this instance)"
    } else {
        ""
    };
    format!(
        "lease: {}{here}, {}",
        text(&lease["holder"]),
        relative("expires", lease["expiresAtMs"].as_i64(), now_ms)
    )
}

fn subsystem_line(status: &Value, now_ms: i64) -> String {
    let names: Vec<&str> = status["eligibleNames"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut line = format!(
        "{}: {} [{}]",
        text(&status["subsystem"]),
        text(&status["state"]),
        names.join(", ")
    );
    if let Some(not_after) = status["notAfterMs"].as_i64() {
        let _ = write!(
            line,
            ", certificate {}",
            relative("expires", Some(not_after), now_ms)
        );
    }
    if let Some(next) = status["nextAttemptAtMs"].as_i64() {
        let _ = write!(line, ", {}", relative("next attempt", Some(next), now_ms));
    }
    let error = text(&status["lastError"]);
    if !error.is_empty() {
        let _ = write!(line, ", last error: {error}");
    }
    let skipped: Vec<String> = status["skippedNames"]
        .as_array()
        .map(|all| {
            all.iter()
                .map(|s| format!("{} ({})", text(&s["name"]), text(&s["reason"])))
                .collect()
        })
        .unwrap_or_default();
    if !skipped.is_empty() {
        let _ = write!(line, ", skipped: {}", skipped.join(", "));
    }
    line
}

/// `expires in 3d 4h` or `expires 2h ago`.
fn relative(what: &str, at_ms: Option<i64>, now_ms: i64) -> String {
    let Some(at) = at_ms else {
        return format!("{what} unknown");
    };
    let secs = (at - now_ms) / 1000;
    if secs >= 0 {
        format!("{what} in {}", duration_text(secs.unsigned_abs()))
    } else {
        format!("{what} {} ago", duration_text(secs.unsigned_abs()))
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn off_says_so() {
        assert!(describe(&json!({ "enabled": false }), 0).contains("ACME is off"));
    }

    #[test]
    fn on_lists_the_settings_the_lease_and_each_subsystem() {
        let data = json!({
            "enabled": true,
            "directory": "https://acme.example/dir",
            "staging": true,
            "accountRegistered": true,
            "lease": { "holder": "42-abc", "expiresAtMs": 120_000, "heldHere": true },
            "subsystems": [{
                "subsystem": "shop",
                "state": "failed",
                "eligibleNames": ["shop.example.com", "www.shop.example.com"],
                "skippedNames": [{ "name": "*.cdn.example.com", "reason": "wildcard_needs_dns01" }],
                "notAfterMs": 86_400_000 + 60_000,
                "nextAttemptAtMs": 30_000,
                "lastError": "dns: NXDOMAIN"
            }]
        });
        let text = describe(&data, 60_000);
        assert!(
            text.contains("directory: https://acme.example/dir (staging"),
            "{text}"
        );
        assert!(text.contains("account: registered"), "{text}");
        assert!(
            text.contains("lease: 42-abc (this instance), expires in 1m"),
            "{text}"
        );
        assert!(
            text.contains("shop: failed [shop.example.com, www.shop.example.com]"),
            "{text}"
        );
        assert!(text.contains("certificate expires in 1day"), "{text}");
        assert!(text.contains("next attempt 30s ago"), "{text}");
        assert!(text.contains("last error: dns: NXDOMAIN"), "{text}");
        assert!(
            text.contains("skipped: *.cdn.example.com (wildcard_needs_dns01)"),
            "{text}"
        );
    }

    #[test]
    fn a_free_lease_and_no_subsystems_are_said_plainly() {
        let data = json!({
            "enabled": true, "directory": "https://a/dir", "staging": false,
            "accountRegistered": false, "lease": null, "subsystems": []
        });
        let text = describe(&data, 0);
        assert!(text.contains("lease: free"), "{text}");
        assert!(text.contains("account: not yet"), "{text}");
        assert!(text.ends_with("no subsystem uses ACME yet"), "{text}");
        assert_eq!(relative("x", None, 0), "x unknown");
    }
}
