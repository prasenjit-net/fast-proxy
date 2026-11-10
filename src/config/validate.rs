use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use super::{AppConfig, ConfigError};

/// Checks cross-field rules and makes sure the state directory exists.
pub fn validate(config: &AppConfig) -> Result<(), ConfigError> {
    let listeners = &config.data_plane.listener;
    if listeners.is_empty() {
        return invalid("at least one [[data_plane.listener]] is required");
    }

    let mut names = HashSet::new();
    for listener in listeners {
        if listener.name.trim().is_empty() {
            return invalid("listener names must not be empty");
        }
        if !names.insert(listener.name.as_str()) {
            return invalid(format!("duplicate listener name \"{}\"", listener.name));
        }
    }

    for (index, first) in listeners.iter().enumerate() {
        for second in &listeners[index + 1..] {
            if addresses_collide(first.listen, second.listen) {
                return invalid(format!(
                    "listeners \"{}\" and \"{}\" collide on {} / {}",
                    first.name, second.name, first.listen, second.listen
                ));
            }
        }
        if addresses_collide(first.listen, config.admin.listen) {
            return invalid(format!(
                "admin.listen {} collides with listener \"{}\" on {}",
                config.admin.listen, first.name, first.listen
            ));
        }
    }

    validate_tls(config)?;

    let state_dir = &config.data_plane.state_dir;
    std::fs::create_dir_all(state_dir).map_err(|err| {
        ConfigError::Invalid(format!(
            "state_dir {} cannot be created: {err}",
            state_dir.display()
        ))
    })
}

/// Checks `[data_plane.tls]` and the master key file. The listener clash is only
/// checked while TLS is on, since a disabled listener binds nothing.
fn validate_tls(config: &AppConfig) -> Result<(), ConfigError> {
    let tls = &config.data_plane.tls;
    if tls.public_port == Some(0) {
        return invalid("data_plane.tls.public_port must be between 1 and 65535");
    }
    if tls.handshake_timeout.is_zero() || tls.handshake_timeout > Duration::from_secs(300) {
        return invalid("data_plane.tls.handshake_timeout must be above 0s and at most 5m");
    }
    if !(1..=65_536).contains(&tls.max_concurrent_handshakes) {
        return invalid("data_plane.tls.max_concurrent_handshakes must be between 1 and 65536");
    }
    if !(1..=1000).contains(&tls.http2_max_concurrent_streams) {
        return invalid("data_plane.tls.http2_max_concurrent_streams must be between 1 and 1000");
    }
    if !(1024..=1_048_576).contains(&tls.http2_max_header_list_size) {
        return invalid(
            "data_plane.tls.http2_max_header_list_size must be between 1024 and 1048576 bytes",
        );
    }
    for (key, value) in [
        ("http2_keep_alive_interval", tls.http2_keep_alive_interval),
        ("http2_keep_alive_timeout", tls.http2_keep_alive_timeout),
    ] {
        if value.is_zero() || value > Duration::from_secs(3600) {
            return invalid(format!(
                "data_plane.tls.{key} must be above 0s and at most 1h"
            ));
        }
    }
    check_master_key_file(&config.data_plane.master_key_path())?;
    if !tls.enabled {
        return Ok(());
    }
    if addresses_collide(tls.listen, config.admin.listen) {
        return invalid(format!(
            "data_plane.tls.listen {} collides with admin.listen {}",
            tls.listen, config.admin.listen
        ));
    }
    for listener in &config.data_plane.listener {
        if addresses_collide(tls.listen, listener.listen) {
            return invalid(format!(
                "data_plane.tls.listen {} collides with listener \"{}\" on {}",
                tls.listen, listener.name, listener.listen
            ));
        }
    }
    Ok(())
}

/// The master key file may be absent (it is created when a key must be sealed)
/// but, when present, must be a readable file. Its content is never printed.
fn check_master_key_file(path: &Path) -> Result<(), ConfigError> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() || std::fs::File::open(path).is_err() {
        return invalid(format!(
            "data_plane.master_key_file {} is not a readable file",
            path.display()
        ));
    }
    Ok(())
}

/// Two bind addresses collide when they share a port and either the IPs
/// are equal or one of them is the wildcard address (which covers the other).
fn addresses_collide(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() == b.port() && (a.ip() == b.ip() || a.ip().is_unspecified() || b.ip().is_unspecified())
}

fn invalid(message: impl Into<String>) -> Result<(), ConfigError> {
    Err(ConfigError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_in(dir: &tempfile::TempDir) -> AppConfig {
        let mut config = AppConfig::default();
        config.data_plane.state_dir = dir.path().join("state");
        config
    }

    fn message(result: Result<(), ConfigError>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn defaults_are_valid_and_create_the_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        validate(&config).unwrap();
        assert!(config.data_plane.state_dir.is_dir());
    }

    #[test]
    fn rejects_no_listeners() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        config.data_plane.listener.clear();
        assert!(message(validate(&config)).contains("at least one"));
    }

    #[test]
    fn rejects_duplicate_listener_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        let mut second = config.data_plane.listener[0].clone();
        second.listen = SocketAddr::from(([0, 0, 0, 0], 8001));
        config.data_plane.listener.push(second);
        assert!(message(validate(&config)).contains("duplicate listener name"));
    }

    #[test]
    fn rejects_duplicate_listener_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        let mut second = config.data_plane.listener[0].clone();
        second.name = "other".into();
        config.data_plane.listener.push(second);
        assert!(message(validate(&config)).contains("collide"));
    }

    #[test]
    fn rejects_admin_address_equal_to_a_listener() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        config.admin.listen = config.data_plane.listener[0].listen;
        assert!(message(validate(&config)).contains("admin.listen"));
    }

    #[test]
    fn wildcard_listener_collides_with_admin_on_the_same_port() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        config.admin.listen = SocketAddr::from(([127, 0, 0, 1], 8000));
        assert!(message(validate(&config)).contains("admin.listen"));
    }

    #[test]
    fn same_port_on_distinct_specific_ips_is_allowed() {
        assert!(!addresses_collide(
            SocketAddr::from(([127, 0, 0, 1], 80)),
            SocketAddr::from(([127, 0, 0, 2], 80))
        ));
    }

    #[test]
    fn rejects_a_state_dir_that_cannot_be_created() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("a-file");
        std::fs::write(&blocker, b"x").unwrap();
        let mut config = config_in(&dir);
        config.data_plane.state_dir = blocker.join("state");
        assert!(message(validate(&config)).contains("state_dir"));
    }

    #[test]
    fn rejects_an_empty_listener_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        config.data_plane.listener[0].name = " ".into();
        assert!(message(validate(&config)).contains("must not be empty"));
    }

    fn tls_on(dir: &tempfile::TempDir) -> AppConfig {
        let mut config = config_in(dir);
        config.data_plane.tls.enabled = true;
        config
    }

    #[test]
    fn tls_defaults_are_valid_and_off() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        assert!(!config.data_plane.tls.enabled);
        validate(&config).unwrap();
        validate(&tls_on(&dir)).unwrap();
    }

    #[test]
    fn the_https_listener_must_not_collide_with_a_plain_listener_or_the_admin_address() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = tls_on(&dir);
        config.data_plane.tls.listen = config.data_plane.listener[0].listen;
        assert!(message(validate(&config)).contains("data_plane.tls.listen"));
        let mut config = tls_on(&dir);
        config.data_plane.tls.listen = config.admin.listen;
        assert!(message(validate(&config)).contains("admin.listen"));
    }

    #[test]
    fn a_clashing_https_listener_is_ignored_while_tls_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        config.data_plane.tls.listen = config.data_plane.listener[0].listen;
        validate(&config).unwrap();
    }

    #[test]
    fn tls_limits_are_range_checked() {
        let dir = tempfile::tempdir().unwrap();
        type Change = fn(&mut AppConfig);
        let cases: [(Change, &str); 9] = [
            (|c| c.data_plane.tls.public_port = Some(0), "public_port"),
            (
                |c| c.data_plane.tls.handshake_timeout = Duration::ZERO,
                "handshake_timeout",
            ),
            (
                |c| c.data_plane.tls.handshake_timeout = Duration::from_secs(301),
                "handshake_timeout",
            ),
            (
                |c| c.data_plane.tls.max_concurrent_handshakes = 0,
                "max_concurrent_handshakes",
            ),
            (
                |c| c.data_plane.tls.max_concurrent_handshakes = 70_000,
                "max_concurrent_handshakes",
            ),
            (
                |c| c.data_plane.tls.http2_max_concurrent_streams = 0,
                "http2_max_concurrent_streams",
            ),
            (
                |c| c.data_plane.tls.http2_max_header_list_size = 100,
                "http2_max_header_list_size",
            ),
            (
                |c| c.data_plane.tls.http2_keep_alive_interval = Duration::ZERO,
                "http2_keep_alive_interval",
            ),
            (
                |c| c.data_plane.tls.http2_keep_alive_timeout = Duration::from_secs(7200),
                "http2_keep_alive_timeout",
            ),
        ];
        for (change, key) in cases {
            let mut config = config_in(&dir);
            change(&mut config);
            assert!(message(validate(&config)).contains(key), "{key}");
        }
        let mut config = config_in(&dir);
        config.data_plane.tls.public_port = Some(443);
        config.data_plane.tls.max_concurrent_handshakes = 65_536;
        validate(&config).unwrap();
    }

    #[test]
    fn the_master_key_file_may_be_absent_but_must_be_readable_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_in(&dir);
        config.data_plane.master_key_file = Some(dir.path().join("absent.key"));
        validate(&config).unwrap();

        let existing = dir.path().join("master.key");
        std::fs::write(&existing, "x").unwrap();
        config.data_plane.master_key_file = Some(existing);
        validate(&config).unwrap();

        config.data_plane.master_key_file = Some(dir.path().to_path_buf());
        let text = message(validate(&config));
        assert!(text.contains("master_key_file") && !text.contains("x\n"));
    }
}
