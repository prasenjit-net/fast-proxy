use std::collections::HashSet;
use std::net::SocketAddr;

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

    let state_dir = &config.data_plane.state_dir;
    std::fs::create_dir_all(state_dir).map_err(|err| {
        ConfigError::Invalid(format!(
            "state_dir {} cannot be created: {err}",
            state_dir.display()
        ))
    })
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
}
