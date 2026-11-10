//! The `https` object of a subsystem: what the admin edits, and what it sees.

use fast_proxy_model::{
    CertificateId, CertificateMetadata, CertificateSource, HostPattern, Hsts, HttpsPolicy,
    RedirectMode,
};
use serde::{Deserialize, Serialize};

use super::input::FieldErrors;

/// The `https` request object. Omitted keys take their defaults; omitting the
/// whole object on an update keeps the stored settings.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpsRequest {
    #[serde(default)]
    enabled: bool,
    certificate_id: Option<i64>,
    redirect: Option<String>,
    hsts: Option<HstsRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HstsRequest {
    #[serde(default)]
    enabled: bool,
    max_age_secs: Option<u32>,
    #[serde(default)]
    include_subdomains: bool,
}

impl HttpsRequest {
    /// Validates the object, collecting every failure under its nested field
    /// name (`https.redirect`, `https.hsts.maxAgeSecs`, ...).
    pub fn into_policy(self, errors: &mut FieldErrors) -> Option<HttpsPolicy> {
        let defaults = Hsts::default();
        let redirect = match self.redirect.as_deref() {
            None => Some(RedirectMode::Off),
            Some(text) => errors.check(RedirectMode::parse(text)),
        };
        let hsts = self.hsts.map_or(defaults, |hsts| Hsts {
            enabled: hsts.enabled,
            max_age_secs: hsts.max_age_secs.unwrap_or(defaults.max_age_secs),
            include_subdomains: hsts.include_subdomains,
        });
        let policy = HttpsPolicy {
            enabled: self.enabled,
            certificate_id: self.certificate_id.map(CertificateId),
            redirect: redirect?,
            hsts,
        };
        errors.check(policy.validate().map(|()| policy))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HstsDto {
    enabled: bool,
    max_age_secs: u32,
    include_subdomains: bool,
}

/// The attached certificate as shown beside the settings.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachedCertificateDto {
    name: String,
    source: &'static str,
    not_after_ms: i64,
    /// Host patterns of the subsystem that the certificate does not cover.
    uncovered_hosts: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpsDto {
    enabled: bool,
    certificate_id: Option<i64>,
    redirect: &'static str,
    hsts: HstsDto,
    /// Whether the settings take effect: HTTPS is enabled here and TLS is on
    /// for the process. Stored settings are inert while TLS is off.
    effective: bool,
    certificate: Option<AttachedCertificateDto>,
}

/// The certificate facts the response needs.
pub struct AttachedCertificate {
    pub name: String,
    pub source: CertificateSource,
    pub metadata: CertificateMetadata,
}

impl HttpsDto {
    pub fn new(
        policy: &HttpsPolicy,
        tls_enabled: bool,
        hosts: &[HostPattern],
        certificate: Option<AttachedCertificate>,
    ) -> Self {
        Self {
            enabled: policy.enabled,
            certificate_id: policy.certificate_id.map(|id| id.0),
            redirect: policy.redirect.as_str(),
            hsts: HstsDto {
                enabled: policy.hsts.enabled,
                max_age_secs: policy.hsts.max_age_secs,
                include_subdomains: policy.hsts.include_subdomains,
            },
            effective: tls_enabled && policy.enabled,
            certificate: certificate.map(|cert| AttachedCertificateDto {
                uncovered_hosts: hosts
                    .iter()
                    .filter(|host| !cert.metadata.covers(host))
                    .map(HostPattern::to_pattern_string)
                    .collect(),
                name: cert.name,
                source: cert.source.as_str(),
                not_after_ms: cert.metadata.not_after_ms,
            }),
        }
    }
}
