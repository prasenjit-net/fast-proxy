//! The `subsystem` HTTPS columns and their mapping to [`HttpsPolicy`], shared
//! by create, update, load and revision restore.

use fast_proxy_model::{CertificateId, Hsts, HttpsPolicy, RedirectMode};
use rusqlite::Row;

use crate::StoreError;

/// Column names in the order [`read`] expects them.
pub const SELECT_LIST: &str = "https_enabled, https_certificate_id, https_redirect, \
     hsts_enabled, hsts_max_age_secs, hsts_include_subdomains";

/// A policy flattened into the values bound to the HTTPS columns.
pub struct Bound {
    pub enabled: bool,
    pub certificate_id: Option<i64>,
    pub redirect: &'static str,
    pub hsts_enabled: bool,
    pub hsts_max_age_secs: u32,
    pub hsts_include_subdomains: bool,
}

impl Bound {
    /// Validates `policy` and flattens it.
    pub fn new(policy: &HttpsPolicy) -> Result<Self, StoreError> {
        policy.validate()?;
        Ok(Self {
            enabled: policy.enabled,
            certificate_id: policy.certificate_id.map(|id| id.0),
            redirect: policy.redirect.as_str(),
            hsts_enabled: policy.hsts.enabled,
            hsts_max_age_secs: policy.hsts.max_age_secs,
            hsts_include_subdomains: policy.hsts.include_subdomains,
        })
    }
}

/// Reads a policy from `row` starting at column `first` (the order of
/// [`SELECT_LIST`]). Stored values that no longer validate are `Internal`.
pub fn read(row: &Row<'_>, first: usize) -> rusqlite::Result<Result<HttpsPolicy, StoreError>> {
    let enabled: bool = row.get(first)?;
    let certificate_id: Option<i64> = row.get(first + 1)?;
    let redirect: String = row.get(first + 2)?;
    let hsts_enabled: bool = row.get(first + 3)?;
    let hsts_max_age_secs: i64 = row.get(first + 4)?;
    let include_subdomains: bool = row.get(first + 5)?;
    let build = || -> Result<HttpsPolicy, fast_proxy_model::FieldError> {
        let policy = HttpsPolicy {
            enabled,
            certificate_id: certificate_id.map(CertificateId),
            redirect: RedirectMode::parse(&redirect)?,
            hsts: Hsts {
                enabled: hsts_enabled,
                max_age_secs: u32::try_from(hsts_max_age_secs).unwrap_or(u32::MAX),
                include_subdomains,
            },
        };
        policy.validate()?;
        Ok(policy)
    };
    Ok(build()
        .map_err(|err| StoreError::Internal(format!("stored https policy is invalid: {err}"))))
}
