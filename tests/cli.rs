//! The command line against the real binary: operating a running proxy
//! (maintenance, reload, status, audit), the exit codes, the token sources and
//! TLS to the admin plane, `config validate`, and offline token recovery.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::console::{printed, start_ok_with_args, start_ok_with_config, token_in, Running};
use common::*;
use serde_json::json;

/// What a command printed and how it ended.
struct Ran {
    code: i32,
    out: String,
    err: String,
}

impl Ran {
    fn of(output: Output) -> Self {
        Self {
            code: output.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            err: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }
    }
}

/// Runs `fast-proxy --config <dir>/config.toml <args>`; the token comes from
/// the environment variable the command documents, when given.
fn cli(dir: &Path, args: &[&str], token: Option<&str>) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fast-proxy"));
    command
        .arg("--config")
        .arg(dir.join("config.toml"))
        .args(args)
        .env_remove("FAST_PROXY_TOKEN")
        .stdin(Stdio::null());
    if let Some(token) = token {
        command.env("FAST_PROXY_TOKEN", token);
    }
    Ran::of(command.output().unwrap())
}

fn database(dir: &Path) -> PathBuf {
    dir.join("state").join("fast-proxy.db")
}

// ---- operating a running proxy ----

#[test]
fn maintenance_on_with_a_ttl_answers_503_status_counts_down_and_off_restores_service() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    create_site(&proxy, "shop", "shop.example.com", backend);
    let url = format!("http://{}", proxy.admin);

    let on = cli(
        dir.path(),
        &[
            "maintenance",
            "on",
            "--url",
            &url,
            "--subsystem",
            "shop",
            "--ttl",
            "30m",
            "--message",
            "Deploying v2",
            "--bypass-cidr",
            "10.0.0.0/8",
            "--bypass-header",
            "X-Pass",
        ],
        None,
    );
    assert_eq!(on.code, 0, "{}", on.err);
    assert!(
        on.out
            .contains("maintenance on: subsystem shop, ends in 29m"),
        "{}",
        on.out
    );
    assert!(
        on.out.contains("bypass secret for X-Pass (shown once"),
        "{}",
        on.out
    );
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 503);

    let status = cli(dir.path(), &["maintenance", "status", "--url", &url], None);
    assert_eq!(status.code, 0, "{}", status.err);
    assert!(
        status.out.contains("subsystem shop, ends in"),
        "{}",
        status.out
    );
    assert!(status.out.contains("\"Deploying v2\""), "{}", status.out);

    let off = cli(
        dir.path(),
        &["maintenance", "off", "--url", &url, "--subsystem", "shop"],
        None,
    );
    assert_eq!(off.code, 0, "{}", off.err);
    assert_eq!(off.out, "maintenance off: subsystem shop");
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 200);
    assert_eq!(
        cli(dir.path(), &["maintenance", "status", "--url", &url], None).out,
        "maintenance is off everywhere"
    );
}

#[test]
fn a_route_and_the_whole_proxy_are_addressed_by_prefix_and_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    create_site(&proxy, "shop", "shop.example.com", backend);
    let url = format!("http://{}", proxy.admin);

    let route = cli(
        dir.path(),
        &[
            "maintenance",
            "on",
            "--url",
            &url,
            "--subsystem",
            "1",
            "--route",
            "/",
            "--message",
            "x",
        ],
        None,
    );
    assert_eq!(route.code, 0, "{}", route.err);
    assert!(route.out.contains("route / of shop"), "{}", route.out);
    let whole = cli(dir.path(), &["maintenance", "on", "--url", &url], None);
    assert!(
        whole
            .out
            .contains("maintenance on: the whole proxy, until turned off"),
        "{}",
        whole.out
    );
    let both = cli(dir.path(), &["maintenance", "status", "--url", &url], None);
    assert_eq!(both.out.lines().count(), 2, "{}", both.out);

    let unknown = cli(
        dir.path(),
        &["maintenance", "on", "--url", &url, "--subsystem", "nope"],
        None,
    );
    assert_eq!(unknown.code, 1);
    assert!(
        unknown.err.contains("no subsystem \"nope\""),
        "{}",
        unknown.err
    );
    let no_route = cli(
        dir.path(),
        &[
            "maintenance",
            "off",
            "--url",
            &url,
            "--subsystem",
            "shop",
            "--route",
            "/nope",
        ],
        None,
    );
    assert_eq!(no_route.code, 1);
    assert!(
        no_route.err.contains("no route \"/nope\""),
        "{}",
        no_route.err
    );
}

#[test]
fn the_url_defaults_to_the_one_in_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let _ = &proxy;
    let status = cli(dir.path(), &["status"], None);
    assert_eq!(status.code, 0, "{}", status.err);
    assert!(
        status.out.contains("configuration version: 0"),
        "{}",
        status.out
    );
    assert!(status.out.contains("authentication off"), "{}", status.out);
}

#[test]
fn reload_answers_with_the_live_version_and_json_is_available() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    create_site(&proxy, "shop", "shop.example.com", backend);
    let reload = cli(dir.path(), &["reload"], None);
    assert_eq!(reload.code, 0, "{}", reload.err);
    assert!(
        reload.out.starts_with("reloaded: configuration version "),
        "{}",
        reload.out
    );
    let json = cli(dir.path(), &["reload", "--json"], None);
    let parsed: serde_json::Value = serde_json::from_str(&json.out).unwrap();
    assert!(parsed["version"].as_u64().unwrap() >= 4);
}

#[test]
fn the_audit_tail_shows_the_change_with_its_actor_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    create_site(&proxy, "shop", "shop.example.com", backend);
    cli(dir.path(), &["maintenance", "on", "--message", "x"], None);
    cli(dir.path(), &["maintenance", "off"], None);

    let all = cli(dir.path(), &["audit", "--limit", "50"], None);
    assert_eq!(all.code, 0, "{}", all.err);
    let lines: Vec<&str> = all.out.lines().collect();
    let on = lines
        .iter()
        .position(|l| l.contains("maintenance.set"))
        .unwrap();
    let off = lines
        .iter()
        .position(|l| l.contains("maintenance.clear"))
        .unwrap();
    assert!(on < off, "oldest first:\n{}", all.out);
    assert!(lines[on].contains("anonymous@127.0.0.1"), "{}", lines[on]);

    let only = cli(dir.path(), &["audit", "--action", "maintenance.set"], None);
    assert_eq!(only.out.lines().count(), 1, "{}", only.out);
    let none = cli(dir.path(), &["audit", "--actor", "nobody"], None);
    assert_eq!(none.out, "no audit entries");
}

// ---- sampled requests ----

/// A proxy whose only backend is down, one failed request made, and the sample
/// of it waited for in the database.
fn proxy_with_a_stored_sample(dir: &Path) -> Proxy {
    let proxy = start_proxy(dir);
    create_site(&proxy, "shop", "shop.example.com", free_port());
    assert_eq!(
        http_with_headers(
            proxy.data,
            "GET",
            "/orders?token=ZQSECRET&page=2",
            "shop.example.com",
            &[("Authorization", "Bearer ZQBEARER")],
            None
        )
        .0,
        502
    );
    let stored = || {
        fast_proxy_store::Database::open_read_only(&database(dir))
            .and_then(|db| db.read(fast_proxy_store::repository::samples::usage))
            .map(|usage| usage.rows)
            .unwrap_or(0)
    };
    assert!(wait_until(|| stored() >= 1), "the sample is written");
    proxy
}

#[test]
fn samples_list_shows_the_failure_and_the_filters_narrow_it() {
    let dir = tempfile::tempdir().unwrap();
    let _proxy = proxy_with_a_stored_sample(dir.path());

    let listed = cli(dir.path(), &["samples", "list"], None);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(listed.out.lines().count(), 1, "{}", listed.out);
    for expected in [
        "502",
        "upstream_failure",
        "shop",
        " / ",
        "GET",
        "instance=",
        "id=",
    ] {
        assert!(
            listed.out.contains(expected),
            "{expected} in {}",
            listed.out
        );
    }

    let matching = |args: &[&str]| {
        let mut all = vec!["samples", "list"];
        all.extend_from_slice(args);
        cli(dir.path(), &all, None).out
    };
    assert!(matching(&["--trigger", "upstream_failure"]).contains("upstream_failure"));
    assert!(matching(&["--status-class", "5", "--subsystem", "shop"]).contains("502"));
    // Not `slow`: a refused connection takes about 2 s on Windows, the default threshold.
    assert_eq!(matching(&["--trigger", "oversized_request"]), "no samples");
    assert_eq!(matching(&["--status-class", "4"]), "no samples");
    assert_eq!(matching(&["--subsystem", "blog"]), "no samples");
    assert_eq!(matching(&["--min-ms", "3600000"]), "no samples");
    assert!(
        matching(&["--since", "1h"]).contains("502"),
        "within the last hour"
    );
    assert_eq!(matching(&["--instance", "elsewhere"]), "no samples");

    let json = cli(dir.path(), &["samples", "list", "--json"], None);
    let parsed: serde_json::Value = serde_json::from_str(&json.out).unwrap();
    assert_eq!(parsed["data"][0]["primary"], "upstream_failure");
}

#[test]
fn samples_export_prints_whole_samples_as_json_lines_with_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let _proxy = proxy_with_a_stored_sample(dir.path());
    let exported = cli(dir.path(), &["samples", "export"], None);
    assert_eq!(exported.code, 0, "{}", exported.err);
    let lines: Vec<serde_json::Value> = exported
        .out
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["request"]["path"], "/orders");
    assert_eq!(lines[0]["request"]["query"], "token=[redacted]&page=2");
    assert_eq!(
        lines[0]["upstream"]["attempts"][0]["failure"],
        "connect_refused"
    );
    assert!(!exported.out.contains("ZQ"), "{}", exported.out);

    let none = cli(
        dir.path(),
        &["samples", "export", "--trigger", "oversized_request"],
        None,
    );
    assert_eq!(none.code, 0);
    assert_eq!(none.out, "", "nothing matches, nothing is printed");
}

#[test]
fn a_viewer_token_is_refused_the_samples_and_an_admin_token_is_not() {
    let dir = tempfile::tempdir().unwrap();
    // A short sync interval, so the proxy sees the token created offline soon.
    let proxy = start_ok_with_config(dir.path(), "config_poll_interval = \"200ms\"", &[], &[]);
    let bootstrap = token_in(&printed(&proxy)).unwrap();
    let url = format!("http://{}", proxy.admin);
    let created = cli(
        dir.path(),
        &["token", "create", "--name", "eyes", "--role", "viewer"],
        None,
    );
    let viewer = created.out.lines().last().unwrap().to_string();
    assert!(wait_until(|| cli(
        dir.path(),
        &["status", "--url", &url],
        Some(&viewer)
    )
    .code
        == 0));

    for action in ["list", "export"] {
        let refused = cli(
            dir.path(),
            &["samples", action, "--url", &url],
            Some(&viewer),
        );
        assert_eq!(refused.code, 1, "{action}");
        assert!(refused.err.contains("(403)"), "{}", refused.err);
        let allowed = cli(
            dir.path(),
            &["samples", action, "--url", &url],
            Some(&bootstrap),
        );
        assert_eq!(allowed.code, 0, "{}", allowed.err);
    }
    let empty = cli(
        dir.path(),
        &["samples", "list", "--url", &url],
        Some(&bootstrap),
    );
    assert_eq!(empty.out, "no samples");
}

// ---- exit codes and tokens ----

fn running_with_token(dir: &Path) -> (Running, String) {
    let proxy = start_ok_with_args(dir, &[], &[]);
    let token = token_in(&printed(&proxy)).expect("a bootstrap token");
    (proxy, token)
}

#[test]
fn a_refused_request_exits_1_with_the_reason_and_an_unreachable_proxy_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let (proxy, token) = running_with_token(dir.path());
    let url = format!("http://{}", proxy.admin);

    let none = cli(dir.path(), &["status", "--url", &url], None);
    assert_eq!(none.code, 1);
    assert!(
        none.err.contains("(401)") && none.err.contains("FAST_PROXY_TOKEN"),
        "{}",
        none.err
    );
    let wrong = cli(dir.path(), &["status", "--url", &url], Some("fp_1.wrong"));
    assert_eq!(wrong.code, 1);
    assert!(
        !wrong.err.contains("pass a token"),
        "a token was given: {}",
        wrong.err
    );
    let right = cli(dir.path(), &["status", "--url", &url], Some(&token));
    assert_eq!(right.code, 0, "{}", right.err);
    assert!(
        right.out.contains("authentication required"),
        "{}",
        right.out
    );

    let bad_field = cli(
        dir.path(),
        &["maintenance", "on", "--url", &url, "--bypass-cidr", "nope"],
        Some(&token),
    );
    assert_eq!(bad_field.code, 1);
    assert!(bad_field.err.contains("bypass.cidrs"), "{}", bad_field.err);

    drop(proxy);
    let gone = cli(dir.path(), &["status", "--url", &url], Some(&token));
    assert_eq!(gone.code, 2);
    assert!(gone.err.contains("cannot reach the proxy"), "{}", gone.err);
}

#[test]
fn usage_errors_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let nothing = cli(dir.path(), &["maintenance", "on", "--route", "/api"], None);
    assert_eq!(nothing.code, 2);
    let bad_ttl = cli(dir.path(), &["maintenance", "on", "--ttl", "soon"], None);
    assert_eq!(bad_ttl.code, 2);
    let no_file = cli(
        dir.path(),
        &[
            "status",
            "--token-file",
            "/nonexistent/token",
            "--url",
            "http://127.0.0.1:1",
        ],
        None,
    );
    assert_eq!(no_file.code, 2);
    assert!(no_file.err.contains("token file"), "{}", no_file.err);
}

#[cfg(unix)]
#[test]
fn the_token_flag_beats_the_file_and_the_file_beats_the_environment() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, token) = running_with_token(dir.path());
    let url = format!("http://{}", proxy.admin);
    let file = dir.path().join("token");
    std::fs::write(&file, format!("{token}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let file = file.to_str().unwrap();

    // The right token only in the file: the wrong one in the environment loses.
    let from_file = cli(
        dir.path(),
        &["status", "--url", &url, "--token-file", file],
        Some("fp_1.wrong"),
    );
    assert_eq!(from_file.code, 0, "{}", from_file.err);
    // A wrong flag beats a right file.
    let from_flag = cli(
        dir.path(),
        &[
            "status",
            "--url",
            &url,
            "--token",
            "fp_1.wrong",
            "--token-file",
            file,
        ],
        None,
    );
    assert_eq!(from_flag.code, 1);
    // A file other users can read is refused.
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let shared = cli(
        dir.path(),
        &["status", "--url", &url, "--token-file", file],
        None,
    );
    assert_eq!(shared.code, 2);
    assert!(shared.err.contains("chmod 600"), "{}", shared.err);
}

// ---- TLS to the admin plane ----

#[tokio::test]
async fn an_admin_plane_over_tls_needs_a_ca_file_or_insecure() {
    use fast_proxy_test_pki::{request, ClientHello, LeafSpec, TestCa};
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_ok_with_args(dir.path(), &[], &["--admin-tls"]);
    let token = token_in(&printed(&proxy)).unwrap();
    let url = format!("https://localhost:{}", proxy.admin.port());

    // The generated certificate is self-signed: the system roots refuse it.
    let default = cli(dir.path(), &["status", "--url", &url], Some(&token));
    assert_eq!(default.code, 2, "{}", default.err);
    // --insecure accepts it.
    let insecure = cli(
        dir.path(),
        &["status", "--url", &url, "--insecure"],
        Some(&token),
    );
    assert_eq!(insecure.code, 0, "{}", insecure.err);
    assert!(
        insecure.out.contains("TLS certificate admin"),
        "{}",
        insecure.out
    );

    // Replace it with one a CA issued, and trust that CA with --ca-file.
    let ca = TestCa::new("ops-ca");
    let issued = ca.issue(&LeafSpec::new(&["localhost"]));
    let hello = ClientHello {
        sni: Some("localhost".into()),
        insecure: true,
        alpn: vec!["http/1.1".into()],
        ..ClientHello::default()
    };
    let list = request(
        proxy.admin,
        &hello,
        format!(
            "GET /api/certificates HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let id = list
        .split("\"id\":")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .unwrap()
        .to_string();
    let body = json!({ "certPem": issued.chain_pem, "keyPem": issued.key_pem }).to_string();
    let put = request(
        proxy.admin,
        &hello,
        format!(
            "PUT /api/certificates/{id} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    assert!(put.starts_with("HTTP/1.1 200"), "{put}");

    let ca_file = dir.path().join("ca.pem");
    std::fs::write(&ca_file, &issued.ca_pem).unwrap();
    let trusted = cli(
        dir.path(),
        &[
            "status",
            "--url",
            &url,
            "--ca-file",
            ca_file.to_str().unwrap(),
        ],
        Some(&token),
    );
    assert_eq!(trusted.code, 0, "{}", trusted.err);
    let wrong_ca = TestCa::new("another-ca");
    let other = dir.path().join("other.pem");
    std::fs::write(
        &other,
        wrong_ca.issue(&LeafSpec::new(&["localhost"])).ca_pem,
    )
    .unwrap();
    let mismatched = cli(
        dir.path(),
        &[
            "status",
            "--url",
            &url,
            "--ca-file",
            other.to_str().unwrap(),
        ],
        Some(&token),
    );
    assert_eq!(mismatched.code, 2, "a different CA does not verify it");
}

// ---- config validate ----

#[test]
fn a_valid_configuration_passes_and_nothing_is_created() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), 18_000, 19_000, "");
    let before: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    let ok = cli(dir.path(), &["config", "validate"], None);
    assert_eq!(ok.code, 0, "{}", ok.err);
    assert!(ok.out.contains("is valid"), "{}", ok.out);
    assert!(
        !dir.path().join("state").exists(),
        "no state directory was created"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), before.len());
}

#[test]
fn each_kind_of_invalid_configuration_exits_with_its_code_and_names_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let case = |toml: &str, args: &[&str]| {
        std::fs::write(&path, toml).unwrap();
        cli(dir.path(), args, None)
    };
    let missing = cli(&dir.path().join("elsewhere"), &["config", "validate"], None);
    assert_eq!(missing.code, 2, "{}", missing.err);

    let typo = case(
        "[admin]\nlistn = \"127.0.0.1:9000\"\n",
        &["config", "validate"],
    );
    assert_eq!(typo.code, 1);
    assert!(typo.err.contains("listn"), "{}", typo.err);

    let exposed = case(
        "[admin]\nlisten = \"0.0.0.0:9000\"\n",
        &["config", "validate"],
    );
    assert_eq!(exposed.code, 1);
    assert!(exposed.err.contains("admin.tls.enabled"), "{}", exposed.err);

    let off_remote = case(
        "[admin]\nlisten = \"0.0.0.0:9000\"\nauth = \"off\"\nallow_insecure_remote = true\n",
        &["config", "validate"],
    );
    assert_eq!(off_remote.code, 1);
    assert!(off_remote.err.contains("admin.auth"), "{}", off_remote.err);

    // The command-line overrides count too, because the next start would use them.
    let overridden = case(
        "",
        &[
            "--admin-auth",
            "off",
            "--admin-listen",
            "0.0.0.0:9000",
            "config",
            "validate",
        ],
    );
    assert_eq!(overridden.code, 1, "{}", overridden.err);
    assert!(overridden.err.contains("admin.auth"), "{}", overridden.err);
}

#[test]
fn the_stored_configuration_is_compiled_read_only_and_a_broken_one_is_reported() {
    use fast_proxy_store::Connection;
    let dir = tempfile::tempdir().unwrap();
    let backend = start_backend("shop-backend");
    {
        let proxy = start_proxy(dir.path());
        create_site(&proxy, "shop", "shop.example.com", backend);
    }
    let good = cli(
        dir.path(),
        &["config", "validate", "--check-database"],
        None,
    );
    assert_eq!(good.code, 0, "{}", good.err);
    assert!(
        good.out.contains("compiles: 1 subsystems, 1 routes"),
        "{}",
        good.out
    );
    assert!(
        good.out.contains("certificates are not decrypted"),
        "{}",
        good.out
    );

    Connection::open(database(dir.path()))
        .unwrap()
        .execute("DELETE FROM upstream_endpoint", [])
        .unwrap();
    let broken = cli(
        dir.path(),
        &["config", "validate", "--check-database"],
        None,
    );
    assert_eq!(broken.code, 1);
    assert!(broken.err.contains("does not compile"), "{}", broken.err);

    let elsewhere = cli(
        dir.path(),
        &[
            "config",
            "validate",
            "--check-database",
            "/nonexistent/x.db",
        ],
        None,
    );
    assert_eq!(elsewhere.code, 2);
    let empty = dir.path().join("empty.db");
    Connection::open(&empty).unwrap();
    let not_ours = cli(
        dir.path(),
        &[
            "config",
            "validate",
            "--check-database",
            empty.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(not_ours.code, 1);
    assert!(
        not_ours.err.contains("not a fast-proxy database"),
        "{}",
        not_ours.err
    );
}

// ---- offline token recovery ----

#[test]
fn a_token_created_offline_works_on_the_running_proxy_within_the_sync_interval() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_ok_with_config(dir.path(), "config_poll_interval = \"200ms\"", &[], &[]);
    let bootstrap = token_in(&printed(&proxy)).unwrap();
    let url = format!("http://{}", proxy.admin);

    let created = cli(
        dir.path(),
        &["token", "create", "--name", "rescue", "--role", "admin"],
        None,
    );
    assert_eq!(created.code, 0, "{}", created.err);
    let rescue = created.out.lines().last().unwrap().to_string();
    assert!(rescue.starts_with("fp_"), "{}", created.out);

    assert!(
        wait_until(|| cli(dir.path(), &["status", "--url", &url], Some(&rescue)).code == 0),
        "the running proxy accepts it after its next sync"
    );
    let listed = cli(dir.path(), &["token", "list"], None);
    assert!(
        listed.out.contains("rescue") && listed.out.contains("bootstrap"),
        "{}",
        listed.out
    );
    assert!(!listed.out.contains(rescue.rsplit('.').next().unwrap()));

    // Revoke the bootstrap token offline: the running proxy locks it out, and the
    // new one keeps working.
    let revoked = cli(dir.path(), &["token", "revoke", "bootstrap"], None);
    assert_eq!(revoked.code, 0, "{}", revoked.err);
    assert!(wait_until(|| cli(
        dir.path(),
        &["status", "--url", &url],
        Some(&bootstrap)
    )
    .code
        == 1));
    assert_eq!(
        cli(dir.path(), &["status", "--url", &url], Some(&rescue)).code,
        0
    );

    // The last admin token cannot be revoked, even from the machine.
    let last = cli(dir.path(), &["token", "revoke", "rescue"], None);
    assert_eq!(last.code, 1);
    assert!(last.err.contains("last usable admin token"), "{}", last.err);

    // Both offline changes are in the audit trail, as `cli`.
    let audit = cli(
        dir.path(),
        &["audit", "--url", &url, "--actor", "cli"],
        Some(&rescue),
    );
    assert!(
        audit.out.contains("token.create") && audit.out.contains("token.revoke"),
        "{}",
        audit.out
    );
}

#[test]
fn offline_token_commands_refuse_a_database_that_does_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), 18_001, 19_001, "");
    let missing = cli(dir.path(), &["token", "list"], None);
    assert_eq!(missing.code, 2);
    assert!(missing.err.contains("no database at"), "{}", missing.err);
    assert!(!database(dir.path()).exists());
}
