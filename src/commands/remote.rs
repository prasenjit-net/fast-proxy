//! The commands that talk to a running proxy: maintenance, reload, status and
//! the audit tail. Each returns the text to print (or the proxy's JSON with
//! `--json`), so what a command says is testable without a terminal.

use std::fmt::Write as _;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::cli::{
    AuditArgs, MaintenanceOff, MaintenanceOn, SampleFilters, SamplesExportArgs, SamplesListArgs,
    ScopeArgs,
};
use crate::commands::client::AdminClient;
use crate::commands::failure::{Failure, Outcome};

/// Pretty JSON for `--json`.
fn json_text(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

pub async fn maintenance_status(client: &AdminClient, json: bool) -> Outcome {
    let answer = client.get("/api/maintenance").await?;
    if json {
        return Ok(json_text(&answer));
    }
    let scopes = answer["data"].as_array().cloned().unwrap_or_default();
    if scopes.is_empty() {
        return Ok("maintenance is off everywhere".to_string());
    }
    let mut out = String::new();
    for scope in &scopes {
        let _ = writeln!(out, "{}", describe_line(scope));
    }
    Ok(out.trim_end().to_string())
}

pub async fn maintenance_on(client: &AdminClient, args: &MaintenanceOn) -> Outcome {
    let path = scope_path(client, &args.scope).await?;
    let body = request_body(args);
    let answer = client.put(&path, &body).await?;
    if args.remote.json {
        return Ok(json_text(&answer));
    }
    let mut out = format!("maintenance on: {}", describe_line(&answer["data"]));
    if let Some(secret) = answer["bypassSecret"].as_str() {
        let header = answer["data"]["bypass"]["header"]
            .as_str()
            .unwrap_or("the bypass header");
        let _ = write!(
            out,
            "\nbypass secret for {header} (shown once, store it now): {secret}"
        );
    }
    Ok(out)
}

pub async fn maintenance_off(client: &AdminClient, args: &MaintenanceOff) -> Outcome {
    let path = scope_path(client, &args.scope).await?;
    let answer = client.delete(&path).await?;
    if args.remote.json {
        return Ok(json_text(&answer));
    }
    Ok(format!("maintenance off: {}", scope_words(&args.scope)))
}

pub async fn reload(client: &AdminClient, json: bool) -> Outcome {
    let answer = client.post("/api/reload").await?;
    if json {
        return Ok(json_text(&answer));
    }
    Ok(format!(
        "reloaded: configuration version {} is live",
        answer["version"]
    ))
}

pub async fn status(client: &AdminClient, json: bool) -> Outcome {
    let answer = client.get("/api/status").await?;
    if json {
        return Ok(json_text(&answer));
    }
    let data = &answer["data"];
    let listeners: Vec<String> = data["listeners"]
        .as_array()
        .map(|all| {
            all.iter()
                .map(|l| {
                    format!(
                        "{} {}",
                        l["name"].as_str().unwrap_or("?"),
                        l["addr"].as_str().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let admin = &data["admin"];
    let certificate = admin["tlsCertificate"]
        .as_str()
        .unwrap_or("none (plain HTTP)");
    Ok(format!(
        "configuration version: {}\nlisteners: {}\nsubsystems: {} ({} enabled)\n\
         admin: authentication {}, TLS certificate {}, {} active tokens\n\
         maintenance: {} scopes on\nuptime: {}",
        answer["version"],
        listeners.join(", "),
        data["subsystems"]["total"],
        data["subsystems"]["enabled"],
        admin["auth"].as_str().unwrap_or("?"),
        certificate,
        admin["activeTokens"],
        admin["activeMaintenance"],
        duration_text(data["uptimeSecs"].as_u64().unwrap_or(0)),
    ))
}

pub async fn audit(client: &AdminClient, args: &AuditArgs) -> Outcome {
    let mut query = format!("limit={}", args.limit);
    for (name, value) in [
        ("actor", &args.actor),
        ("action", &args.action),
        ("outcome", &args.outcome),
    ] {
        if let Some(value) = value {
            let _ = write!(query, "&{name}={}", percent_encode(value));
        }
    }
    let answer = client.get(&format!("/api/audit?{query}")).await?;
    if args.remote.json {
        return Ok(json_text(&answer));
    }
    let entries = answer["data"].as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        return Ok("no audit entries".to_string());
    }
    // The API is newest first; a tail reads oldest to newest.
    let lines: Vec<String> = entries.iter().rev().map(audit_line).collect();
    Ok(lines.join("\n"))
}

fn audit_line(entry: &Value) -> String {
    let at = UNIX_EPOCH + Duration::from_millis(entry["atMs"].as_u64().unwrap_or(0));
    format!(
        "{} {:<6} {}@{} {} {}",
        humantime::format_rfc3339_seconds(at),
        entry["outcome"].as_str().unwrap_or("?"),
        entry["actor"].as_str().unwrap_or("?"),
        entry["source"].as_str().unwrap_or("?"),
        entry["action"].as_str().unwrap_or("?"),
        entry["target"].as_str().unwrap_or("")
    )
}

/// The query-string pairs for the filters the operator gave, `since` made
/// absolute from `now_ms`.
fn sample_filter_query(filters: &SampleFilters, now_ms: i64) -> String {
    let mut query = String::new();
    let mut add = |name: &str, value: String| {
        let _ = write!(query, "&{name}={}", percent_encode(&value));
    };
    if !filters.triggers.is_empty() {
        add("trigger", filters.triggers.join(","));
    }
    if let Some(class) = filters.status_class {
        add("statusClass", class.to_string());
    }
    for (name, value) in [
        ("subsystem", &filters.subsystem),
        ("route", &filters.route),
        ("instance", &filters.instance),
        ("client", &filters.client),
    ] {
        if let Some(value) = value {
            add(name, value.clone());
        }
    }
    if let Some(min_ms) = filters.min_ms {
        add("minMs", min_ms.to_string());
    }
    if let Some(since) = filters.since {
        let since_ms = now_ms.saturating_sub(i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
        add("since", since_ms.max(0).to_string());
    }
    query
}

pub async fn samples_list(client: &AdminClient, args: &SamplesListArgs, now_ms: i64) -> Outcome {
    let query = format!(
        "limit={}{}",
        args.limit,
        sample_filter_query(&args.filters, now_ms)
    );
    let answer = client.get(&format!("/api/samples?{query}")).await?;
    if args.remote.json {
        return Ok(json_text(&answer));
    }
    let samples = answer["data"].as_array().cloned().unwrap_or_default();
    if samples.is_empty() {
        return Ok("no samples".to_string());
    }
    Ok(samples
        .iter()
        .map(sample_line)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Every matching sample as the proxy sends it: one JSON object per line.
pub async fn samples_export(
    client: &AdminClient,
    args: &SamplesExportArgs,
    now_ms: i64,
) -> Outcome {
    let query = sample_filter_query(&args.filters, now_ms);
    let text = client
        .get_text(&format!(
            "/api/samples/export?{}",
            query.trim_start_matches('&')
        ))
        .await?;
    Ok(text.trim_end_matches('\n').to_string())
}

fn sample_line(sample: &Value) -> String {
    let at = UNIX_EPOCH + Duration::from_millis(sample["atMs"].as_u64().unwrap_or(0));
    format!(
        "{} {} {} {} {} {} {}ms {} instance={} id={}",
        humantime::format_rfc3339_seconds(at),
        sample["status"].as_u64().unwrap_or(0),
        sample["primary"].as_str().unwrap_or("?"),
        sample["subsystem"].as_str().unwrap_or("?"),
        sample["route"].as_str().unwrap_or("?"),
        sample["method"].as_str().unwrap_or("?"),
        sample["totalMs"].as_u64().unwrap_or(0),
        sample["client"].as_str().unwrap_or("?"),
        sample["instance"].as_str().unwrap_or("?"),
        sample["id"].as_i64().unwrap_or(0)
    )
}

/// The request body for turning maintenance on: only what was asked for.
fn request_body(args: &MaintenanceOn) -> Value {
    let mut body = json!({ "message": args.message });
    if let Some(ttl) = args.ttl {
        body["ttlSecs"] = json!(ttl.as_secs());
    }
    if let Some(secs) = args.retry_after {
        body["retryAfterSecs"] = json!(secs);
    }
    let mut bypass = serde_json::Map::new();
    if !args.bypass_cidrs.is_empty() {
        bypass.insert("cidrs".into(), json!(args.bypass_cidrs));
    }
    if let Some(header) = &args.bypass_header {
        bypass.insert("header".into(), json!(header));
    }
    if let Some(secret) = &args.bypass_secret {
        bypass.insert("secret".into(), json!(secret));
    }
    if !bypass.is_empty() {
        body["bypass"] = Value::Object(bypass);
    }
    body
}

/// The API path for a scope, resolving a subsystem name and a route prefix to ids.
async fn scope_path(client: &AdminClient, scope: &ScopeArgs) -> Result<String, Failure> {
    let Some(subsystem) = &scope.subsystem else {
        return Ok("/api/maintenance/global".to_string());
    };
    let subsystems = client.get("/api/subsystems").await?;
    let id = find_id(&subsystems["data"], subsystem, "name")
        .ok_or_else(|| Failure::Refused(format!("no subsystem \"{subsystem}\" (name or id)")))?;
    let Some(route) = &scope.route else {
        return Ok(format!("/api/subsystems/{id}/maintenance"));
    };
    let routes = client.get(&format!("/api/subsystems/{id}/routes")).await?;
    let route_id = find_id(&routes["data"], route, "prefix").ok_or_else(|| {
        Failure::Refused(format!(
            "subsystem \"{subsystem}\" has no route \"{route}\""
        ))
    })?;
    Ok(format!(
        "/api/subsystems/{id}/routes/{route_id}/maintenance"
    ))
}

/// The id of the entry whose `key` equals `wanted` (or, for a number, whose id does).
fn find_id(entries: &Value, wanted: &str, key: &str) -> Option<i64> {
    let numeric = wanted.parse::<i64>().ok();
    entries.as_array()?.iter().find_map(|entry| {
        let id = entry["id"].as_i64()?;
        (entry[key].as_str() == Some(wanted) || (key == "name" && numeric == Some(id)))
            .then_some(id)
    })
}

fn scope_words(scope: &ScopeArgs) -> String {
    match (&scope.subsystem, &scope.route) {
        (Some(subsystem), Some(route)) => format!("route {route} of {subsystem}"),
        (Some(subsystem), None) => format!("subsystem {subsystem}"),
        _ => "the whole proxy".to_string(),
    }
}

/// One scope in a sentence: what it covers, its message, and the countdown.
fn describe_line(scope: &Value) -> String {
    let target = match scope["scope"].as_str() {
        Some("subsystem") => format!("subsystem {}", scope["subsystem"].as_str().unwrap_or("?")),
        Some("route") => format!(
            "route {} of {}",
            scope["route"].as_str().unwrap_or("?"),
            scope["subsystem"].as_str().unwrap_or("?")
        ),
        _ => "the whole proxy".to_string(),
    };
    let mut line = target;
    match scope["secondsLeft"].as_u64() {
        Some(secs) => {
            let _ = write!(line, ", ends in {}", duration_text(secs));
        }
        None => line.push_str(", until turned off"),
    }
    if let Some(message) = scope["message"].as_str().filter(|m| !m.is_empty()) {
        let _ = write!(line, ": \"{message}\"");
    }
    if let Some(by) = scope["setBy"].as_str().filter(|by| !by.is_empty()) {
        let _ = write!(line, " (set by {by})");
    }
    line
}

fn duration_text(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    humantime::format_duration(Duration::from_secs(secs)).to_string()
}

/// Escapes a query value (anything that is not unreserved).
fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{RemoteArgs, SampleFilters};

    fn on() -> MaintenanceOn {
        MaintenanceOn {
            remote: RemoteArgs::default(),
            scope: ScopeArgs::default(),
            ttl: None,
            message: String::new(),
            retry_after: None,
            bypass_cidrs: vec![],
            bypass_header: None,
            bypass_secret: None,
        }
    }

    #[test]
    fn the_body_holds_only_what_was_asked_for() {
        assert_eq!(request_body(&on()), json!({ "message": "" }));
        let full = MaintenanceOn {
            ttl: Some(Duration::from_secs(1800)),
            message: "deploying".into(),
            retry_after: Some(60),
            bypass_cidrs: vec!["10.0.0.0/8".into()],
            bypass_header: Some("X-Pass".into()),
            bypass_secret: Some("s3cret-s3cret-s3cret".into()),
            ..on()
        };
        assert_eq!(
            request_body(&full),
            json!({
                "message": "deploying", "ttlSecs": 1800, "retryAfterSecs": 60,
                "bypass": { "cidrs": ["10.0.0.0/8"], "header": "X-Pass", "secret": "s3cret-s3cret-s3cret" }
            })
        );
    }

    #[test]
    fn a_name_or_an_id_finds_the_subsystem_and_a_prefix_finds_the_route() {
        let subsystems = json!([{ "id": 1, "name": "shop" }, { "id": 2, "name": "blog" }]);
        assert_eq!(find_id(&subsystems, "blog", "name"), Some(2));
        assert_eq!(find_id(&subsystems, "1", "name"), Some(1));
        assert_eq!(find_id(&subsystems, "nope", "name"), None);
        let routes = json!([{ "id": 7, "prefix": "/" }, { "id": 8, "prefix": "/api" }]);
        assert_eq!(find_id(&routes, "/api", "prefix"), Some(8));
        assert_eq!(
            find_id(&routes, "8", "prefix"),
            None,
            "a route is found by prefix only"
        );
    }

    #[test]
    fn a_scope_reads_as_a_sentence_with_its_countdown_and_author() {
        let scope = json!({ "scope": "route", "subsystem": "shop", "route": "/api",
            "secondsLeft": 125, "message": "down", "setBy": "ci" });
        assert_eq!(
            describe_line(&scope),
            "route /api of shop, ends in 2m 5s: \"down\" (set by ci)"
        );
        let open = json!({ "scope": "global", "message": "", "setBy": "" });
        assert_eq!(describe_line(&open), "the whole proxy, until turned off");
    }

    #[test]
    fn audit_lines_read_oldest_first_with_the_actor_and_source() {
        let line = audit_line(&json!({ "atMs": 0, "outcome": "ok", "actor": "ann",
            "source": "10.0.0.5", "action": "route.create", "target": "shop" }));
        assert_eq!(
            line,
            "1970-01-01T00:00:00Z ok     ann@10.0.0.5 route.create shop"
        );
    }

    #[test]
    fn query_values_are_escaped() {
        assert_eq!(percent_encode("ci deploy&x=1"), "ci%20deploy%26x%3D1");
        assert_eq!(percent_encode("a.b-c_d~e"), "a.b-c_d~e");
    }

    #[test]
    fn durations_read_in_words() {
        assert_eq!(duration_text(0), "0s");
        assert_eq!(duration_text(3_661), "1h 1m 1s");
    }

    #[test]
    fn no_filters_give_an_empty_query_and_each_filter_its_own_pair() {
        assert_eq!(sample_filter_query(&SampleFilters::default(), 1_000), "");
        let filters = SampleFilters {
            triggers: vec!["slow".into(), "retried".into()],
            status_class: Some(5),
            subsystem: Some("my shop".into()),
            route: Some("/api".into()),
            instance: Some("a-1".into()),
            client: Some("203.0.113.9:1".into()),
            min_ms: Some(250),
            since: Some(Duration::from_secs(60)),
        };
        assert_eq!(
            sample_filter_query(&filters, 100_000),
            "&trigger=slow%2Cretried&statusClass=5&subsystem=my%20shop&route=%2Fapi\
             &instance=a-1&client=203.0.113.9%3A1&minMs=250&since=40000"
        );
    }

    #[test]
    fn since_never_goes_before_the_epoch() {
        let filters = SampleFilters {
            since: Some(Duration::from_secs(3600)),
            ..SampleFilters::default()
        };
        assert_eq!(sample_filter_query(&filters, 1_000), "&since=0");
    }

    #[test]
    fn a_sample_reads_as_one_line() {
        let line = sample_line(&json!({
            "atMs": 1_700_000_000_000_u64, "status": 504, "primary": "upstream_failure",
            "subsystem": "shop", "route": "/api", "method": "POST", "totalMs": 30012,
            "client": "203.0.113.9:5", "instance": "a1", "id": 42
        }));
        assert_eq!(
            line,
            "2023-11-14T22:13:20Z 504 upstream_failure shop /api POST 30012ms 203.0.113.9:5 instance=a1 id=42"
        );
        assert!(
            sample_line(&json!({})).contains('?'),
            "missing fields do not panic"
        );
    }
}
