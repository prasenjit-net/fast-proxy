use fast_proxy_config_service::CertificateSummary;
use serde::Serialize;

/// A certificate as the API shows it: metadata only. Private keys, their
/// ciphertext and the PEM never appear here.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateDto {
    pub id: i64,
    pub name: String,
    pub source: &'static str,
    pub names: Vec<String>,
    pub subject: String,
    pub issuer: String,
    pub not_before_ms: i64,
    pub not_after_ms: i64,
    pub seconds_left: i64,
    /// `ok`, `expiring`, `expired` or `unreadable`.
    pub state: &'static str,
    pub fingerprint: String,
    pub used_by: Vec<i64>,
    pub updated_at_ms: i64,
    /// Non-fatal findings of a write, such as a certificate expiring soon.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl CertificateDto {
    pub fn new(summary: &CertificateSummary, now_ms: i64, warnings: Vec<String>) -> Self {
        let metadata = &summary.metadata;
        Self {
            id: summary.id.0,
            name: summary.name.as_str().to_string(),
            source: summary.source.as_str(),
            names: metadata.names.clone(),
            subject: metadata.subject.clone(),
            issuer: metadata.issuer.clone(),
            not_before_ms: metadata.not_before_ms,
            not_after_ms: metadata.not_after_ms,
            seconds_left: (metadata.not_after_ms - now_ms).div_euclid(1000),
            state: summary.state.as_str(),
            fingerprint: metadata.fingerprint.clone(),
            used_by: summary.used_by.iter().map(|id| id.0).collect(),
            updated_at_ms: summary.updated_at_ms,
            warnings,
        }
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
