//! `fast-proxy limits`, `fast-proxy rate-limit show|set|clear` (M7, A3) and
//! `fast-proxy status --local`: the limits in force on a running proxy, the
//! rate-limit policy of a subsystem or route, and a token-free liveness check.

use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::cli::{RateLimitSet, RateLimitTarget};
use crate::commands::client::AdminClient;
use crate::commands::failure::{Failure, Outcome};
use crate::commands::remote::{duration_text, find_id, json_text};

/// The process limits, the key table's fill, and what was refused so far.
pub async fn status(client: &AdminClient, json: bool) -> Outcome {
    let answer = client.get("/api/limits").await?;
    if json {
        return Ok(json_text(&answer));
    }
    Ok(describe_limits(&answer["data"]))
}

/// The policy of a subsystem or route, or that there is none.
pub async fn show(client: &AdminClient, target: &RateLimitTarget) -> Outcome {
    let path = policy_path(client, target).await?;
    let answer = match client.get(&path).await {
        Ok(answer) => answer,
        Err(Failure::Refused(why)) if why.contains("(404)") => {
            return Ok(format!("{}: no rate limit", target_words(target)));
        }
        Err(other) => return Err(other),
    };
    if target.remote.json {
        return Ok(json_text(&answer));
    }
    Ok(format!(
        "{}: {}",
        target_words(target),
        describe_policy(&answer["data"])
    ))
}

/// Sets the policy, replacing the one there.
pub async fn set(client: &AdminClient, args: &RateLimitSet) -> Outcome {
    let body = policy_body(args)?;
    let path = policy_path(client, &args.target).await?;
    let answer = client.put(&path, &body).await?;
    if args.target.remote.json {
        return Ok(json_text(&answer));
    }
    Ok(format!(
        "{}: {} (version {})",
        target_words(&args.target),
        describe_policy(&answer["data"]),
        answer["version"]
    ))
}

/// Removes the policy.
pub async fn clear(client: &AdminClient, target: &RateLimitTarget) -> Outcome {
    let path = policy_path(client, target).await?;
    let answer = client.delete(&path).await?;
    if target.remote.json {
        return Ok(json_text(&answer));
    }
    let after = if target.route.is_some() {
        "; the route uses its subsystem's policy, if any"
    } else {
        ""
    };
    Ok(format!(
        "{}: rate limit cleared (version {}){after}",
        target_words(target),
        answer["version"]
    ))
}

/// `status --local`: the admin plane answers `/api/health`, with no token.
pub async fn health(client: &AdminClient) -> Outcome {
    let answer = client.get("/api/health").await?;
    if answer["status"] != "ok" {
        return Err(Failure::Refused(format!(
            "the proxy is not healthy: {answer}"
        )));
    }
    Ok(format!("ok {}", answer["version"].as_str().unwrap_or("")))
}

/// The request body of `set`; the period must be whole seconds.
fn policy_body(args: &RateLimitSet) -> Result<Value, Failure> {
    if args.per.subsec_nanos() != 0 || args.per.as_secs() == 0 {
        return Err(Failure::Usage(
            "--per must be whole seconds, such as 1s, 1m or 1h".into(),
        ));
    }
    let mut body = json!({ "rate": args.rate, "periodSecs": args.per.as_secs() });
    if let Some(burst) = args.burst {
        body["burst"] = json!(burst);
    }
    if let Some(header) = &args.header {
        body["key"] = json!("header");
        body["header"] = json!(header);
    }
    if let Some(prefix) = args.ipv6_prefix {
        body["ipv6Prefix"] = json!(prefix);
    }
    Ok(body)
}

/// The API path of the policy the target names, by subsystem name or id and route prefix.
async fn policy_path(client: &AdminClient, target: &RateLimitTarget) -> Result<String, Failure> {
    let subsystems = client.get("/api/subsystems").await?;
    let id = find_id(&subsystems["data"], &target.subsystem, "name").ok_or_else(|| {
        Failure::Refused(format!(
            "no subsystem \"{}\" (name or id)",
            target.subsystem
        ))
    })?;
    let Some(route) = &target.route else {
        return Ok(format!("/api/subsystems/{id}/rate-limit"));
    };
    let routes = client.get(&format!("/api/subsystems/{id}/routes")).await?;
    let route_id = find_id(&routes["data"], route, "prefix").ok_or_else(|| {
        Failure::Refused(format!(
            "subsystem \"{}\" has no route \"{route}\"",
            target.subsystem
        ))
    })?;
    Ok(format!("/api/subsystems/{id}/routes/{route_id}/rate-limit"))
}

fn target_words(target: &RateLimitTarget) -> String {
    match &target.route {
        Some(route) => format!("route {route} of {}", target.subsystem),
        None => format!("subsystem {}", target.subsystem),
    }
}

/// `10 per 1m, burst 20, by client (IPv6 /64), per instance`.
fn describe_policy(policy: &Value) -> String {
    let key = match policy["header"].as_str() {
        Some(header) => format!("by header {header}"),
        None => format!("by client (IPv6 /{})", policy["ipv6Prefix"]),
    };
    format!(
        "{} per {}, burst {}, {key}, per instance",
        policy["rate"],
        duration_text(policy["periodSecs"].as_u64().unwrap_or(0)),
        policy["burst"]
    )
}

fn describe_limits(status: &Value) -> String {
    let settings = &status["settings"];
    let mut out = String::new();
    let per_client = match settings["maxConnectionsPerClient"].as_u64() {
        Some(cap) => format!("{cap} per client"),
        None => "no cap per client".to_string(),
    };
    let _ = writeln!(
        out,
        "connections: {} at most, {per_client}",
        settings["maxConnections"]
    );
    let _ = writeln!(
        out,
        "request head: {} headers, {} bytes of headers, {} bytes of target",
        settings["maxHeaderCount"], settings["maxHeaderBytes"], settings["maxUriBytes"]
    );
    let body = match settings["maxBodyBytes"].as_u64() {
        Some(cap) => format!("{cap} bytes"),
        None => "unlimited".to_string(),
    };
    let _ = writeln!(out, "request body: {body} unless a route sets its own");
    let tunnel = match settings["tunnelIdleTimeoutMs"].as_u64() {
        Some(ms) => millis(ms),
        None => "off".to_string(),
    };
    let _ = writeln!(
        out,
        "timeouts: first byte {}, body {}, send {}, idle tunnel {tunnel}",
        millis(settings["headerReadTimeoutMs"].as_u64().unwrap_or(0)),
        millis(settings["bodyReadTimeoutMs"].as_u64().unwrap_or(0)),
        millis(settings["sendTimeoutMs"].as_u64().unwrap_or(0)),
    );
    let keys = &status["rateLimitKeys"];
    let _ = writeln!(
        out,
        "rate-limit keys: {} of {} in use, {} new clients let through while full",
        keys["used"], keys["capacity"], status["rateLimitTableFull"]
    );
    let _ = writeln!(
        out,
        "connections refused: {}",
        reasons(&status["connectionsRefused"])
    );
    let _ = writeln!(
        out,
        "requests refused: {}",
        reasons(&status["requestsRefused"])
    );
    let _ = write!(
        out,
        "client timeouts: {}",
        reasons(&status["clientTimeouts"])
    );
    out
}

/// `rate_limited 3, body_too_large 1`, or `none`.
fn reasons(counts: &Value) -> String {
    let listed: Vec<String> = counts
        .as_array()
        .map(|all| {
            all.iter()
                .filter(|entry| entry["count"].as_u64().unwrap_or(0) > 0)
                .map(|entry| {
                    format!(
                        "{} {}",
                        entry["reason"].as_str().unwrap_or("?"),
                        entry["count"]
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if listed.is_empty() {
        return "none".to_string();
    }
    listed.join(", ")
}

/// Whole seconds as `30s` or `1h`; anything else in milliseconds.
fn millis(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        return duration_text(ms / 1000);
    }
    format!("{ms}ms")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::cli::RemoteArgs;

    use super::*;

    fn set_args(per: Duration) -> RateLimitSet {
        RateLimitSet {
            target: RateLimitTarget {
                remote: RemoteArgs::default(),
                subsystem: "shop".into(),
                route: None,
            },
            rate: 10,
            per,
            burst: None,
            header: None,
            ipv6_prefix: None,
        }
    }

    #[test]
    fn the_body_sends_only_what_was_given() {
        assert_eq!(
            policy_body(&set_args(Duration::from_secs(60))).unwrap(),
            json!({ "rate": 10, "periodSecs": 60 })
        );
        let mut args = set_args(Duration::from_secs(1));
        args.burst = Some(20);
        args.header = Some("x-customer".into());
        args.ipv6_prefix = Some(56);
        assert_eq!(
            policy_body(&args).unwrap(),
            json!({ "rate": 10, "periodSecs": 1, "burst": 20, "key": "header",
                    "header": "x-customer", "ipv6Prefix": 56 })
        );
    }

    #[test]
    fn a_period_that_is_not_whole_seconds_is_a_usage_error() {
        for per in [Duration::from_millis(1500), Duration::ZERO] {
            assert!(matches!(
                policy_body(&set_args(per)),
                Err(Failure::Usage(_))
            ));
        }
    }

    #[test]
    fn a_policy_reads_as_a_sentence() {
        let client = json!({ "rate": 10, "periodSecs": 60, "burst": 20, "key": "client",
                             "header": null, "ipv6Prefix": 64 });
        assert_eq!(
            describe_policy(&client),
            "10 per 1m, burst 20, by client (IPv6 /64), per instance"
        );
        let header = json!({ "rate": 1, "periodSecs": 1, "burst": 1, "key": "header",
                             "header": "x-customer", "ipv6Prefix": 64 });
        assert!(describe_policy(&header).contains("by header x-customer"));
    }

    #[test]
    fn the_limits_list_settings_fill_and_refusals() {
        let status = json!({
            "settings": {
                "maxConnections": 10000, "maxConnectionsPerClient": null,
                "headerReadTimeoutMs": 10000, "bodyReadTimeoutMs": 30000,
                "sendTimeoutMs": 1500, "tunnelIdleTimeoutMs": null,
                "maxHeaderCount": 100, "maxHeaderBytes": 65536, "maxUriBytes": 8192,
                "maxBodyBytes": 1048576
            },
            "rateLimitKeys": { "used": 3, "capacity": 100000 },
            "connectionsRefused": [],
            "requestsRefused": [{ "reason": "rate_limited", "count": 2 },
                                { "reason": "uri_too_long", "count": 0 }],
            "clientTimeouts": [{ "reason": "first_byte", "count": 1 }],
            "rateLimitTableFull": 0
        });
        let text = describe_limits(&status);
        for expected in [
            "connections: 10000 at most, no cap per client",
            "request body: 1048576 bytes",
            "timeouts: first byte 10s, body 30s, send 1500ms, idle tunnel off",
            "rate-limit keys: 3 of 100000 in use",
            "connections refused: none",
            "requests refused: rate_limited 2\n",
            "client timeouts: first_byte 1",
        ] {
            assert!(text.contains(expected), "{expected:?} in\n{text}");
        }
    }
}
