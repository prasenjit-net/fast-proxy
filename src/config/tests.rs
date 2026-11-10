use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use super::*;

fn parse(toml: &str) -> Result<AppConfig, toml::de::Error> {
    toml::from_str(toml)
}

#[test]
fn missing_file_falls_back_to_defaults() {
    let config = AppConfig::load(Path::new("/nonexistent/definitely-not-here.toml")).unwrap();
    assert_eq!(
        config.admin.listen,
        SocketAddr::from(([127, 0, 0, 1], 9000))
    );
    assert_eq!(config.data_plane.listener.len(), 1);
    assert_eq!(config.ui.default_theme, "auto");
}

#[test]
fn parses_a_full_config_file() {
    let config = parse(
        r#"
        [data_plane]
        state_dir = "/var/lib/fp"
        database = "/data/fp.db"
        trusted_proxies = ["10.0.0.0/8"]
        shutdown_grace = "45s"

        [[data_plane.listener]]
        name = "public"
        listen = "0.0.0.0:80"

        [[data_plane.listener]]
        name = "alt"
        listen = "0.0.0.0:8080"

        [admin]
        listen = "127.0.0.1:9100"

        [logging]
        level = "debug"
        access_log = "access.log"

        [ui]
        app_name = "Test App"
        "#,
    )
    .unwrap();
    assert_eq!(config.data_plane.state_dir, PathBuf::from("/var/lib/fp"));
    assert_eq!(
        config.data_plane.database_path(),
        PathBuf::from("/data/fp.db")
    );
    assert_eq!(config.data_plane.trusted_proxies, ["10.0.0.0/8"]);
    assert_eq!(config.data_plane.shutdown_grace, Duration::from_secs(45));
    assert_eq!(config.data_plane.listener.len(), 2);
    assert_eq!(config.data_plane.listener[1].name, "alt");
    assert_eq!(config.admin.listen.port(), 9100);
    assert_eq!(config.logging.access_log, Some(PathBuf::from("access.log")));
    assert_eq!(config.ui.app_name, "Test App");
}

#[test]
fn database_defaults_to_a_file_inside_the_state_dir() {
    let config = parse("[data_plane]\nstate_dir = \"/srv/fp\"\n").unwrap();
    assert_eq!(
        config.data_plane.database_path(),
        PathBuf::from("/srv/fp/fast-proxy.db")
    );
}

#[test]
fn missing_sections_fall_back_to_defaults() {
    let config = parse("[admin]\nlisten = \"127.0.0.1:1234\"\n").unwrap();
    assert_eq!(config.admin.listen.port(), 1234);
    assert_eq!(config.logging.level, "info");
    assert_eq!(config.data_plane.shutdown_grace, Duration::from_secs(30));
    assert_eq!(config.data_plane.listener[0].listen.port(), 8000);
    assert_eq!(config.ui.app_name, "Fast Proxy");
}

#[test]
fn unknown_fields_are_rejected() {
    assert!(parse("[admin]\ntypo_field = true\n").is_err());
    assert!(parse("[data_plane]\ntypo_field = true\n").is_err());
}

#[test]
fn the_old_server_section_is_rejected() {
    assert!(parse("[server]\nport = 8080\n").is_err());
}

#[test]
fn a_tls_key_on_a_listener_is_rejected_until_m3() {
    let result =
        parse("[[data_plane.listener]]\nname = \"https\"\nlisten = \"0.0.0.0:443\"\ntls = true\n");
    assert!(result.is_err());
}

#[test]
fn a_listener_without_an_address_is_rejected() {
    assert!(parse("[[data_plane.listener]]\nname = \"http\"\n").is_err());
}

#[test]
fn an_invalid_duration_is_rejected() {
    assert!(parse("[data_plane]\nshutdown_grace = \"soon\"\n").is_err());
}

#[test]
fn an_invalid_listen_address_is_rejected() {
    assert!(parse("[admin]\nlisten = \"not-an-address\"\n").is_err());
}

#[test]
fn load_reports_parse_errors_with_the_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "[server]\nport = 1\n").unwrap();
    let message = AppConfig::load(&path).unwrap_err().to_string();
    assert!(message.contains("bad.toml"), "{message}");
}

#[test]
fn load_reads_a_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ok.toml");
    std::fs::write(&path, "[logging]\nlevel = \"warn\"\n").unwrap();
    assert_eq!(AppConfig::load(&path).unwrap().logging.level, "warn");
}

#[test]
fn tls_and_cluster_settings_default_to_off_and_five_seconds() {
    let config = AppConfig::default();
    let tls = &config.data_plane.tls;
    assert!(!tls.enabled && tls.http2 && !tls.reject_unknown_sni);
    assert_eq!(tls.listen, SocketAddr::from(([0, 0, 0, 0], 8443)));
    assert_eq!(tls.min_version, fast_proxy_tls::MinTlsVersion::V1_2);
    assert_eq!(tls.handshake_timeout, Duration::from_secs(10));
    assert_eq!(tls.max_concurrent_handshakes, 256);
    assert_eq!(tls.http2_max_concurrent_streams, 128);
    assert_eq!(
        config.data_plane.config_poll_interval,
        Duration::from_secs(5)
    );
    assert_eq!(
        config.data_plane.master_key_path(),
        PathBuf::from("./state/master.key")
    );
}

#[test]
fn parses_the_tls_section_and_cluster_settings() {
    let config = parse(
        r#"
        [data_plane]
        master_key_file = "/secrets/master.key"
        config_poll_interval = "0s"

        [data_plane.tls]
        enabled = true
        listen = "0.0.0.0:443"
        public_port = 443
        min_version = "1.3"
        http2 = false
        reject_unknown_sni = true
        handshake_timeout = "3s"
        max_concurrent_handshakes = 64
        http2_max_concurrent_streams = 32
        http2_max_header_list_size = 16384
        http2_keep_alive_interval = "10s"
        http2_keep_alive_timeout = "5s"
        "#,
    )
    .unwrap();
    let tls = &config.data_plane.tls;
    assert!(tls.enabled && !tls.http2 && tls.reject_unknown_sni);
    assert_eq!(tls.public_port, Some(443));
    assert_eq!(tls.min_version, fast_proxy_tls::MinTlsVersion::V1_3);
    assert_eq!(tls.handshake_timeout, Duration::from_secs(3));
    assert_eq!(tls.max_concurrent_handshakes, 64);
    assert_eq!(tls.http2_max_concurrent_streams, 32);
    assert_eq!(tls.http2_max_header_list_size, 16_384);
    assert_eq!(tls.http2_keep_alive_interval, Duration::from_secs(10));
    assert_eq!(tls.http2_keep_alive_timeout, Duration::from_secs(5));
    assert_eq!(config.data_plane.config_poll_interval, Duration::ZERO);
    assert_eq!(
        config.data_plane.master_key_path(),
        PathBuf::from("/secrets/master.key")
    );
}

#[test]
fn unknown_tls_keys_and_certificate_entries_are_rejected() {
    for toml in [
        "[data_plane.tls]\ncert = \"a.pem\"",
        "[data_plane.tls]\nkey = \"a.pem\"",
        "[data_plane.tls]\nmin_version = \"1.1\"",
        "[data_plane.tls]\nhandshake_timeout = \"soon\"",
        "[[data_plane.listener]]\nname = \"a\"\nlisten = \"0.0.0.0:80\"\ntls = true",
    ] {
        assert!(parse(toml).is_err(), "{toml}");
    }
}

#[test]
fn the_repository_config_file_parses_and_is_valid() {
    let config = AppConfig::load(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/config.toml"
    )))
    .unwrap();
    // Whether TLS is switched on in the file is the operator's choice; the
    // built-in default (off) is checked by the defaults test above.
    assert_eq!(config.data_plane.tls.listen.port(), 8443);
    config.validate().unwrap();
}
