//! The `[ui]` config section served to the SPA.

use serde::{Deserialize, Serialize};

/// The `[ui]` section of the config file, exposed verbatim to the SPA
/// through `GET /api/config` (serialized in camelCase for the frontend).
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
            app_name: "Fast Proxy".into(),
            tagline: "Realtime-configurable ingress and reverse proxy".into(),
            default_theme: "auto".into(),
            repo_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_camel_case_for_the_frontend() {
        let value = serde_json::to_value(UiConfig::default()).unwrap();
        assert!(value.get("appName").is_some());
        assert!(value.get("defaultTheme").is_some());
        assert!(value.get("app_name").is_none());
    }

    #[test]
    fn rejects_unknown_fields() {
        let parsed: Result<UiConfig, _> = serde_json::from_str(r#"{"typo_field": true}"#);
        assert!(parsed.is_err());
    }
}
