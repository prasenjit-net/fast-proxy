//! CA bundle writes: public PEM that upstream groups name to verify TLS
//! backends (ADR 0013). They go through [`ConfigService::apply`] like every
//! other change; a bundle an upstream group names cannot be deleted.

use fast_proxy_model::CertificateName;
use fast_proxy_store::repository::ca_bundles::{self, NewCaBundle};
use fast_proxy_store::{Mutation, StoreError};
use fast_proxy_tls::{parse_ca_bundle, CaCertInfo};

use crate::{Applied, ConfigService};

/// A stored CA bundle as the admin plane lists it.
#[derive(Debug, Clone)]
pub struct CaBundleSummary {
    pub id: i64,
    pub name: CertificateName,
    pub certificates: Vec<CaCertInfo>,
    pub updated_at_ms: i64,
}

impl ConfigService {
    /// Stores a CA bundle. Fails with `pem` when it holds no usable certificate.
    pub async fn upload_ca_bundle(
        &self,
        name: &str,
        pem: &str,
    ) -> Result<Applied<i64>, StoreError> {
        let name = CertificateName::parse(name)?;
        let infos = parse_ca_bundle(pem)?;
        let bundle = NewCaBundle {
            name: name.clone(),
            pem: pem.to_string(),
            cert_count: u32::try_from(infos.len()).unwrap_or(u32::MAX),
        };
        self.apply(move |conn| {
            Ok(Mutation {
                value: ca_bundles::insert(conn, &bundle)?,
                summary: format!("CA bundle uploaded: {}", name.as_str()),
            })
        })
        .await
    }

    /// Deletes a CA bundle; a `Conflict` while an upstream group names it.
    pub async fn delete_ca_bundle(&self, id: i64) -> Result<Applied<()>, StoreError> {
        self.apply(move |conn| {
            let name = ca_bundles::get(conn, id)?.bundle.name;
            ca_bundles::delete(conn, id)?;
            Ok(Mutation {
                value: (),
                summary: format!("CA bundle deleted: {}", name.as_str()),
            })
        })
        .await
    }

    /// Every CA bundle with the subjects and expiry of its certificates.
    pub async fn list_ca_bundles(&self) -> Result<Vec<CaBundleSummary>, StoreError> {
        let rows = self.read(ca_bundles::list).await?.value;
        rows.into_iter()
            .map(|row| {
                Ok(CaBundleSummary {
                    id: row.id,
                    certificates: parse_ca_bundle(&row.bundle.pem)?,
                    name: row.bundle.name,
                    updated_at_ms: row.updated_at_ms,
                })
            })
            .collect()
    }
}
