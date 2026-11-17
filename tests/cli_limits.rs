//! The limits commands against the real binary (M7, A3): `rate-limit
//! show|set|clear` change what the data plane does at once, `limits` reports
//! the refusals, and `status --local` checks liveness with no token.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};

use common::console::{bootstrap_token, start_ok_with_args};
use common::*;
use serde_json::json;

struct Ran {
    code: i32,
    out: String,
    err: String,
}

/// Runs `fast-proxy --config <dir>/config.toml <args>` with no token in the environment.
fn cli(dir: &Path, args: &[&str]) -> Ran {
    let output = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(dir.join("config.toml"))
        .args(args)
        .env_remove("FAST_PROXY_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Ran {
        code: output.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        err: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

const HOST: &str = "shop.example.com";

#[test]
fn a_rate_limit_set_from_the_cli_refuses_at_once_and_limits_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    create_site(&proxy, "shop", HOST, backend);
    let url = format!("http://{}", proxy.admin);

    let none = cli(dir.path(), &["rate-limit", "show", "shop", "--url", &url]);
    assert_eq!(none.code, 0, "{}", none.err);
    assert_eq!(none.out, "subsystem shop: no rate limit");

    let set = cli(
        dir.path(),
        &[
            "rate-limit",
            "set",
            "shop",
            "--url",
            &url,
            "--rate",
            "2",
            "--per",
            "1m",
        ],
    );
    assert_eq!(set.code, 0, "{}", set.err);
    assert!(
        set.out
            .starts_with("subsystem shop: 2 per 1m, burst 2, by client (IPv6 /64), per instance"),
        "{}",
        set.out
    );
    let statuses: Vec<u16> = (0..3).map(|_| through_proxy(&proxy, HOST, "/").0).collect();
    assert_eq!(statuses, [200, 200, 429], "live without a restart");

    let shown = cli(
        dir.path(),
        &["rate-limit", "show", "shop", "--url", &url, "--json"],
    );
    let parsed: serde_json::Value = serde_json::from_str(&shown.out).unwrap();
    assert_eq!(parsed["data"]["rate"], 2);

    let limits = cli(dir.path(), &["limits", "--url", &url]);
    assert_eq!(limits.code, 0, "{}", limits.err);
    assert!(
        limits.out.contains("requests refused: rate_limited 1"),
        "{}",
        limits.out
    );
    assert!(
        limits.out.contains("rate-limit keys: 1 of "),
        "{}",
        limits.out
    );

    let cleared = cli(dir.path(), &["rate-limit", "clear", "shop", "--url", &url]);
    assert_eq!(cleared.code, 0, "{}", cleared.err);
    assert!(
        cleared.out.contains("rate limit cleared"),
        "{}",
        cleared.out
    );
    assert_eq!(through_proxy(&proxy, HOST, "/").0, 200);
    let again = cli(dir.path(), &["rate-limit", "clear", "shop", "--url", &url]);
    assert_eq!(again.code, 1, "nothing to clear is a refusal");
}

#[test]
fn a_route_policy_is_addressed_by_prefix_and_bad_values_exit_with_their_code() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    let (shop, upstream) = create_site(&proxy, "shop", HOST, backend);
    let (status, _) = admin(
        &proxy,
        "POST",
        &format!("/api/subsystems/{shop}/routes"),
        Some(json!({ "prefix": "/api", "upstreamGroupId": upstream })),
    );
    assert_eq!(status, 201);
    let url = format!("http://{}", proxy.admin);

    let set = cli(
        dir.path(),
        &[
            "rate-limit",
            "set",
            "shop",
            "--route",
            "/api",
            "--url",
            &url,
            "--rate",
            "1",
            "--per",
            "1h",
            "--header",
            "X-Customer",
        ],
    );
    assert_eq!(set.code, 0, "{}", set.err);
    assert!(
        set.out
            .starts_with("route /api of shop: 1 per 1h, burst 1, by header x-customer"),
        "{}",
        set.out
    );

    let refused = cli(
        dir.path(),
        &["rate-limit", "set", "shop", "--url", &url, "--rate", "0"],
    );
    assert_eq!(refused.code, 1);
    assert!(refused.err.contains("rateLimit.rate"), "{}", refused.err);
    let no_route = cli(
        dir.path(),
        &[
            "rate-limit",
            "show",
            "shop",
            "--route",
            "/nope",
            "--url",
            &url,
        ],
    );
    assert_eq!(no_route.code, 1);
    assert!(
        no_route.err.contains("no route \"/nope\""),
        "{}",
        no_route.err
    );
    let fraction = cli(
        dir.path(),
        &[
            "rate-limit",
            "set",
            "shop",
            "--url",
            &url,
            "--rate",
            "1",
            "--per",
            "1500ms",
        ],
    );
    assert_eq!(fraction.code, 2, "a usage error: {}", fraction.err);
}

#[test]
fn status_local_needs_no_token_and_fails_when_nothing_answers() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_ok_with_args(dir.path(), &[], &[]);
    assert!(bootstrap_token(&proxy).is_some(), "authentication is on");
    let url = format!("http://{}", proxy.admin);

    let local = cli(dir.path(), &["status", "--local", "--url", &url]);
    assert_eq!(local.code, 0, "{}", local.err);
    assert!(local.out.starts_with("ok "), "{}", local.out);
    let full = cli(dir.path(), &["status", "--url", &url]);
    assert_eq!(full.code, 1, "the full status still needs a token");

    drop(proxy);
    let gone = cli(dir.path(), &["status", "--local", "--url", &url]);
    assert_eq!(gone.code, 2, "{}", gone.err);
}
