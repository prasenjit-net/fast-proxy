use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    pub access_log: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            access_log: None,
        }
    }
}

/// The `[ui]` section is exposed verbatim to the SPA through
/// `GET /api/config` (serialized in camelCase for the frontend).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "camelCase"))]
pub struct UiConfig {
    pub app_name: String,
    pub tagline: String,
    pub default_theme: String,
    pub repo_url: Option<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            app_name: "Rusty Template".into(),
            tagline: "Single-binary fullstack starter".into(),
            default_theme: "auto".into(),
            repo_url: None,
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        if !path.exists() {
            eprintln!(
                "warning: config file {} not found, using built-in defaults",
                path.display()
            );
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|err| AppError::Config(format!("failed to read {}: {err}", path.display())))?;
        toml::from_str(&raw)
            .map_err(|err| AppError::Config(format!("invalid config {}: {err}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let config = AppConfig::load(Path::new("/nonexistent/definitely-not-here.toml")).unwrap();
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.ui.default_theme, "auto");
    }

    #[test]
    fn parses_a_full_config_file() {
        let toml = r#"
            [server]
            host = "0.0.0.0"
            port = 9000

            [logging]
            level = "debug"
            access_log = "access.log"

            [ui]
            app_name = "Test App"
            tagline = "Testing"
            default_theme = "dark"
            repo_url = "https://example.com/repo"
        "#;
        let config: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.access_log, Some(PathBuf::from("access.log")));
        assert_eq!(config.ui.app_name, "Test App");
        assert_eq!(
            config.ui.repo_url.as_deref(),
            Some("https://example.com/repo")
        );
    }

    #[test]
    fn missing_sections_fall_back_to_defaults() {
        let config: AppConfig = toml::from_str("[server]\nport = 1234\n").unwrap();
        assert_eq!(config.server.port, 1234);
        assert_eq!(config.server.host, "127.0.0.1"); // untouched field defaults
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.ui.app_name, "Rusty Template");
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let toml = "[server]\nport = 1234\ntypo_field = true\n";
        let result: Result<AppConfig, _> = toml::from_str(toml);
        assert!(
            result.is_err(),
            "deny_unknown_fields should reject typo_field"
        );
    }

    #[test]
    fn ui_config_serializes_camel_case_for_the_frontend() {
        let ui = UiConfig {
            app_name: "X".into(),
            tagline: "Y".into(),
            default_theme: "auto".into(),
            repo_url: None,
        };
        let value = serde_json::to_value(&ui).unwrap();
        assert!(value.get("appName").is_some());
        assert!(value.get("defaultTheme").is_some());
        assert!(value.get("app_name").is_none());
    }
}
