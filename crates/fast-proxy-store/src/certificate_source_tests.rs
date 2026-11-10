use fast_proxy_model::{
    CertificateId, CertificateMetadata, CertificateName, CertificateSource as Origin,
};

use crate::repository::ca_bundles::CaBundleRow;
use crate::repository::certificates::{self, CertificateRow, NewCertificate};
use crate::repository::vault::VaultCheck;
use crate::test_support::commit;
use crate::{CertificateSource, Database, StoreError};

/// A source that is not SQLite, proving consumers depend only on the trait.
struct FixedSource {
    rows: Vec<CertificateRow>,
    version: u64,
}

impl CertificateSource for FixedSource {
    fn certificates(&self) -> Result<Vec<CertificateRow>, StoreError> {
        Ok(self.rows.clone())
    }
    fn ca_bundles(&self) -> Result<Vec<CaBundleRow>, StoreError> {
        Ok(Vec::new())
    }
    fn vault_check(&self) -> Result<Option<VaultCheck>, StoreError> {
        Ok(None)
    }
    fn version(&self) -> Result<u64, StoreError> {
        Ok(self.version)
    }
}

/// Reads through the trait only, as the config service will.
fn summarize(source: &dyn CertificateSource) -> (usize, u64) {
    (
        source.certificates().unwrap().len(),
        source.version().unwrap(),
    )
}

fn new_certificate() -> NewCertificate {
    NewCertificate {
        name: CertificateName::parse("shop").unwrap(),
        source: Origin::Uploaded,
        chain_pem: String::new(),
        key_enc: vec![1],
        key_nonce: vec![2],
        metadata: CertificateMetadata {
            names: vec![],
            subject: String::new(),
            issuer: String::new(),
            not_before_ms: 0,
            not_after_ms: 1,
            fingerprint: String::new(),
        },
    }
}

#[test]
fn the_sqlite_database_and_a_test_double_both_serve_the_trait() {
    let mut db = Database::open_in_memory().unwrap();
    assert_eq!(summarize(&db), (0, 0));
    commit(&mut db, "upload", |conn| {
        let id = certificates::next_id(conn)?;
        certificates::insert(conn, id, &new_certificate())
    })
    .unwrap();
    assert_eq!(
        summarize(&db),
        (1, 1),
        "the version is the revision version"
    );

    let row = db.certificates().unwrap().remove(0);
    assert_eq!(row.id, CertificateId(1));
    let double = FixedSource {
        rows: vec![row],
        version: 41,
    };
    assert_eq!(summarize(&double), (1, 41));
}

#[test]
fn the_store_has_no_cryptography_dependency_so_it_cannot_decrypt() {
    let manifest = include_str!("../Cargo.toml");
    for crypto in ["ring", "aes", "rustls", "openssl", "chacha", "sodium"] {
        assert!(
            !manifest.contains(crypto),
            "{crypto} must not be a dependency"
        );
    }
}
