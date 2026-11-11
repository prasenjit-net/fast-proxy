//! Where a command gets its admin token and which admin URL it talks to.

use std::path::Path;

use crate::commands::failure::Failure;
use crate::config::AppConfig;

/// The environment variable that carries the admin token.
pub const TOKEN_VARIABLE: &str = "FAST_PROXY_TOKEN";

/// The token to use. A flag wins, then a token file, then the environment; none
/// at all is fine (an admin plane with authentication off wants none).
pub fn resolve_token(
    flag: Option<&str>,
    file: Option<&Path>,
    environment: Option<&str>,
) -> Result<Option<String>, Failure> {
    if let Some(flag) = flag {
        return Ok(non_empty(flag));
    }
    if let Some(file) = file {
        return read_token_file(file).map(Some);
    }
    Ok(environment.and_then(non_empty))
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn read_token_file(path: &Path) -> Result<String, Failure> {
    refuse_shared_file(path)?;
    let text = std::fs::read_to_string(path).map_err(|err| {
        Failure::Usage(format!(
            "cannot read the token file {}: {err}",
            path.display()
        ))
    })?;
    non_empty(&text)
        .ok_or_else(|| Failure::Usage(format!("the token file {} is empty", path.display())))
}

/// A token file anyone else can read is not a secret: refuse it (Unix; other
/// platforms have no mode bits to check).
#[cfg(unix)]
fn refuse_shared_file(path: &Path) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|err| {
            Failure::Usage(format!(
                "cannot read the token file {}: {err}",
                path.display()
            ))
        })?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(Failure::Usage(format!(
            "the token file {} is readable by other users (mode {:o}); run chmod 600 on it",
            path.display(),
            mode & 0o777
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn refuse_shared_file(_: &Path) -> Result<(), Failure> {
    Ok(())
}

/// The admin plane's URL from the config file's `[admin]` section: its scheme
/// follows `[admin.tls]`, and an unspecified listen address means this machine.
pub fn default_url(config: &AppConfig) -> String {
    let scheme = if config.admin.tls.enabled {
        "https"
    } else {
        "http"
    };
    let mut address = config.admin.listen;
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    format!("{scheme}://{address}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_beats_a_file_beats_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        write_private(&file, "from-file\n");
        let pick = |flag, file: Option<&Path>, env| resolve_token(flag, file, env).unwrap();
        assert_eq!(
            pick(Some("flag"), Some(&file), Some("env")),
            Some("flag".into())
        );
        assert_eq!(
            pick(None, Some(&file), Some("env")),
            Some("from-file".into())
        );
        assert_eq!(pick(None, None, Some("env")), Some("env".into()));
        assert_eq!(pick(None, None, None), None);
    }

    #[test]
    fn blank_values_count_as_absent() {
        assert_eq!(resolve_token(Some("  "), None, Some("env")).unwrap(), None);
        assert_eq!(resolve_token(None, None, Some("\n")).unwrap(), None);
    }

    #[test]
    fn a_missing_or_empty_token_file_is_a_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = resolve_token(None, Some(&dir.path().join("none")), None).unwrap_err();
        assert!(matches!(missing, Failure::Usage(_)));
        let empty = dir.path().join("empty");
        write_private(&empty, "  \n");
        let err = resolve_token(None, Some(&empty), None).unwrap_err();
        assert!(err.message().contains("empty"), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_token_file_other_users_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("token");
        write_private(&file, "secret");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = resolve_token(None, Some(&file), None).unwrap_err();
        assert!(err.message().contains("chmod 600"), "{err:?}");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(resolve_token(None, Some(&file), None).is_ok());
    }

    fn write_private(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn the_default_url_follows_the_listen_address_and_admin_tls() {
        let mut config = AppConfig::default();
        assert_eq!(default_url(&config), "http://127.0.0.1:9000");
        config.admin.listen = "0.0.0.0:9443".parse().unwrap();
        config.admin.tls.enabled = true;
        assert_eq!(default_url(&config), "https://127.0.0.1:9443");
        config.admin.listen = "[::]:9000".parse().unwrap();
        assert_eq!(default_url(&config), "https://[::1]:9000");
        config.admin.listen = "192.0.2.10:9000".parse().unwrap();
        assert_eq!(default_url(&config), "https://192.0.2.10:9000");
    }
}
