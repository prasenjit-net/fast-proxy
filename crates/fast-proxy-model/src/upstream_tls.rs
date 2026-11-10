use crate::certificate::validate_material_name;
use crate::host_pattern::validate_dns_name;
use crate::FieldError;

/// How the proxy talks TLS to the endpoints of one upstream group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamTls {
    pub enabled: bool,
    /// Verify the endpoint's certificate (on by default; off is allowed but
    /// surfaced everywhere).
    pub verify: bool,
    /// Overrides the name used for SNI and the certificate check; empty means
    /// the endpoint's host.
    pub server_name: String,
    /// Name of an uploaded CA bundle; empty means the OS trust store.
    pub ca: String,
}

impl Default for UpstreamTls {
    fn default() -> Self {
        Self {
            enabled: false,
            verify: true,
            server_name: String::new(),
            ca: String::new(),
        }
    }
}

impl UpstreamTls {
    /// Checks the server name and CA name; errors name `tls.serverName` or `tls.ca`.
    pub fn validate(&self) -> Result<(), FieldError> {
        if !self.server_name.is_empty() {
            validate_dns_name(&self.server_name.to_ascii_lowercase())
                .map_err(|err| FieldError::new("tls.serverName", err.message))?;
        }
        if !self.ca.is_empty() {
            validate_material_name("tls.ca", &self.ca)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_verify_and_use_the_endpoint_host_and_os_store() {
        let tls = UpstreamTls::default();
        assert!(!tls.enabled && tls.verify);
        assert!(tls.server_name.is_empty() && tls.ca.is_empty());
        assert!(tls.validate().is_ok());
    }

    #[test]
    fn server_name_must_be_empty_or_a_dns_name() {
        for good in ["backend.internal", "Backend", "a-b.c"] {
            let tls = UpstreamTls {
                server_name: good.into(),
                ..UpstreamTls::default()
            };
            assert!(tls.validate().is_ok(), "{good}");
        }
        for bad in ["a b", "a:443", "10.0.0.1", "*.internal", "a..b"] {
            let tls = UpstreamTls {
                server_name: bad.into(),
                ..UpstreamTls::default()
            };
            assert_eq!(tls.validate().unwrap_err().field, "tls.serverName", "{bad}");
        }
    }

    #[test]
    fn ca_must_be_empty_or_a_valid_bundle_name() {
        let ok = UpstreamTls {
            ca: "internal-ca".into(),
            ..UpstreamTls::default()
        };
        assert!(ok.validate().is_ok());
        for bad in ["-x", "a b", &"a".repeat(64)] {
            let tls = UpstreamTls {
                ca: bad.into(),
                ..UpstreamTls::default()
            };
            assert_eq!(tls.validate().unwrap_err().field, "tls.ca", "{bad}");
        }
    }
}
