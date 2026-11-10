//! The certificate cache: parsed, decrypted rustls keys keyed by certificate id
//! and version. A rebuild reuses unchanged entries, parses only new or changed
//! rows, drops removed ones and marks unreadable ones, so a renewal costs one
//! parse, not N (ADR 0011).

use std::collections::BTreeMap;
use std::sync::Arc;

use fast_proxy_model::{CertificateId, CertificateMetadata, CertificateName, CertificateSource};
use rustls::sign::CertifiedKey;

use crate::key_material::KeyMaterial;
use crate::validate::parse_chain;
use crate::vault::{Sealed, Vault};

/// One stored certificate as the cache reads it. The config service builds
/// these from store rows; this crate never depends on the store.
#[derive(Debug, Clone)]
pub struct StoredCertificate {
    pub id: CertificateId,
    pub name: CertificateName,
    pub source: CertificateSource,
    pub chain_pem: String,
    pub sealed_key: Sealed,
    pub metadata: CertificateMetadata,
    /// When the row last changed, milliseconds since the epoch.
    pub updated_at_ms: i64,
}

/// What identifies a cached version of a certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertVersion {
    pub updated_at_ms: i64,
    pub fingerprint: String,
}

/// A usable certificate: the rustls key plus what the admin plane shows.
pub struct CertHandle {
    pub id: CertificateId,
    pub name: CertificateName,
    pub source: CertificateSource,
    pub version: CertVersion,
    pub metadata: CertificateMetadata,
    pub key: Arc<CertifiedKey>,
}

impl std::fmt::Debug for CertHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertHandle")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// Why a stored certificate could not be turned into a handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadableReason {
    /// There is no master key to open the sealed private key with.
    NoMasterKey,
    /// The sealed key did not open: a wrong master key or changed data.
    Undecryptable,
    /// The key or chain did not parse, or the key does not match.
    Corrupt,
}

impl UnreadableReason {
    /// A short, secret-free description for status and logs.
    pub fn describe(self) -> &'static str {
        match self {
            Self::NoMasterKey => "no master key is available",
            Self::Undecryptable => "the private key cannot be decrypted (master key mismatch?)",
            Self::Corrupt => "the stored certificate or key is corrupt",
        }
    }
}

/// A stored certificate that could not be read, with the public facts that are
/// still known so the admin plane can show it.
#[derive(Debug, Clone)]
pub struct UnreadableCert {
    pub id: CertificateId,
    pub name: CertificateName,
    pub source: CertificateSource,
    pub metadata: CertificateMetadata,
    pub reason: UnreadableReason,
}

/// One cache entry.
#[derive(Debug, Clone)]
pub enum CertEntry {
    Ready(Arc<CertHandle>),
    Unreadable(UnreadableCert),
}

/// What a rebuild did, for the cache metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildStats {
    /// Unchanged entries reused as they were.
    pub hits: u64,
    /// New or changed rows that were parsed.
    pub misses: u64,
    /// Rows that could not be read.
    pub unreadable: u64,
}

/// The parsed certificates of one configuration generation. Immutable: a
/// rebuild returns a new cache and leaves this one (and handles shared with
/// in-flight handshakes) untouched.
#[derive(Debug, Clone, Default)]
pub struct CertCache {
    entries: BTreeMap<CertificateId, CertEntry>,
}

impl CertCache {
    pub fn empty() -> Self {
        Self::default()
    }

    /// The usable handle of a certificate, if it exists and is readable.
    pub fn handle(&self, id: CertificateId) -> Option<&Arc<CertHandle>> {
        match self.entries.get(&id)? {
            CertEntry::Ready(handle) => Some(handle),
            CertEntry::Unreadable(_) => None,
        }
    }

    pub fn entry(&self, id: CertificateId) -> Option<&CertEntry> {
        self.entries.get(&id)
    }

    /// Every entry, ordered by id.
    pub fn entries(&self) -> impl Iterator<Item = &CertEntry> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Builds the cache for `rows`, reusing this cache's unchanged handles.
    /// Without a `vault` (no master key) every row is unreadable.
    pub fn rebuild(
        &self,
        rows: &[StoredCertificate],
        vault: Option<&Vault>,
    ) -> (Self, RebuildStats) {
        let mut stats = RebuildStats::default();
        let mut entries = BTreeMap::new();
        for row in rows {
            let entry = match self.reusable(row) {
                Some(handle) => {
                    stats.hits += 1;
                    CertEntry::Ready(handle)
                }
                None => match parse(row, vault) {
                    Ok(handle) => {
                        stats.misses += 1;
                        CertEntry::Ready(Arc::new(handle))
                    }
                    Err(reason) => {
                        stats.unreadable += 1;
                        CertEntry::Unreadable(UnreadableCert {
                            id: row.id,
                            name: row.name.clone(),
                            source: row.source,
                            metadata: row.metadata.clone(),
                            reason,
                        })
                    }
                },
            };
            entries.insert(row.id, entry);
        }
        (Self { entries }, stats)
    }

    fn reusable(&self, row: &StoredCertificate) -> Option<Arc<CertHandle>> {
        let CertEntry::Ready(handle) = self.entries.get(&row.id)? else {
            return None;
        };
        let same = handle.version == version_of(row) && handle.name == row.name;
        same.then(|| Arc::clone(handle))
    }
}

fn version_of(row: &StoredCertificate) -> CertVersion {
    CertVersion {
        updated_at_ms: row.updated_at_ms,
        fingerprint: row.metadata.fingerprint.clone(),
    }
}

fn parse(row: &StoredCertificate, vault: Option<&Vault>) -> Result<CertHandle, UnreadableReason> {
    let vault = vault.ok_or(UnreadableReason::NoMasterKey)?;
    let plain = vault
        .open_key(row.id, row.name.as_str(), &row.sealed_key)
        .map_err(|_| UnreadableReason::Undecryptable)?;
    let key = KeyMaterial::from_sealed_bytes(&plain).ok_or(UnreadableReason::Corrupt)?;
    let chain = parse_chain(&row.chain_pem).map_err(|_| UnreadableReason::Corrupt)?;
    let signing_key = key.signing_key().ok_or(UnreadableReason::Corrupt)?;
    let certified = CertifiedKey::new(chain, signing_key);
    certified
        .keys_match()
        .map_err(|_| UnreadableReason::Corrupt)?;
    Ok(CertHandle {
        id: row.id,
        name: row.name.clone(),
        source: row.source,
        version: version_of(row),
        metadata: row.metadata.clone(),
        key: Arc::new(certified),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::self_signed::generate_self_signed;
    use crate::validate::read_metadata;
    use crate::vault::MasterKey;

    const NOW: i64 = 1_800_000_000_000;

    fn vault() -> Vault {
        Vault::new(&MasterKey::from_base64(&base64_key(1)).unwrap())
    }

    fn other_vault() -> Vault {
        Vault::new(&MasterKey::from_base64(&base64_key(2)).unwrap())
    }

    fn base64_key(fill: u8) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode([fill; 32])
    }

    fn row(vault: &Vault, id: i64, name: &str, updated: i64) -> StoredCertificate {
        let generated = generate_self_signed(&[format!("{name}.example.com")], NOW).unwrap();
        let chain = parse_chain(&generated.chain_pem).unwrap();
        let metadata = read_metadata(&chain[0]).unwrap();
        let id = CertificateId(id);
        let sealed_key = vault
            .seal_key(id, name, &generated.key.to_sealed_bytes())
            .unwrap();
        StoredCertificate {
            id,
            name: CertificateName::parse(name).unwrap(),
            source: CertificateSource::SelfSigned,
            chain_pem: generated.chain_pem,
            sealed_key,
            metadata,
            updated_at_ms: updated,
        }
    }

    #[test]
    fn every_row_is_parsed_once_and_then_reused_by_pointer() {
        let vault = vault();
        let rows = [row(&vault, 1, "a", 10), row(&vault, 2, "b", 10)];
        let (first, stats) = CertCache::empty().rebuild(&rows, Some(&vault));
        assert_eq!(
            stats,
            RebuildStats {
                hits: 0,
                misses: 2,
                unreadable: 0
            }
        );
        let (second, stats) = first.rebuild(&rows, Some(&vault));
        assert_eq!(
            stats,
            RebuildStats {
                hits: 2,
                misses: 0,
                unreadable: 0
            }
        );
        for id in [1, 2] {
            assert!(Arc::ptr_eq(
                first.handle(CertificateId(id)).unwrap(),
                second.handle(CertificateId(id)).unwrap()
            ));
        }
    }

    #[test]
    fn a_changed_row_gets_a_new_handle_while_the_old_one_stays_usable() {
        let vault = vault();
        let before = [row(&vault, 1, "a", 10), row(&vault, 2, "b", 10)];
        let (first, _) = CertCache::empty().rebuild(&before, Some(&vault));
        let old = Arc::clone(first.handle(CertificateId(1)).unwrap());

        let renewed = row(&vault, 1, "a", 20);
        let after = [renewed, before[1].clone()];
        let (second, stats) = first.rebuild(&after, Some(&vault));
        assert_eq!(
            stats,
            RebuildStats {
                hits: 1,
                misses: 1,
                unreadable: 0
            }
        );
        let new = second.handle(CertificateId(1)).unwrap();
        assert!(!Arc::ptr_eq(&old, new));
        assert_ne!(old.version, new.version);
        assert!(Arc::ptr_eq(
            first.handle(CertificateId(2)).unwrap(),
            second.handle(CertificateId(2)).unwrap()
        ));
        assert!(
            old.key.keys_match().is_ok(),
            "the replaced handle still works"
        );
    }

    #[test]
    fn a_removed_row_is_dropped() {
        let vault = vault();
        let rows = [row(&vault, 1, "a", 1), row(&vault, 2, "b", 1)];
        let (first, _) = CertCache::empty().rebuild(&rows, Some(&vault));
        let (second, _) = first.rebuild(&rows[..1], Some(&vault));
        assert_eq!(second.len(), 1);
        assert!(second.entry(CertificateId(2)).is_none());
        assert_eq!(first.len(), 2, "the old cache is unchanged");
    }

    #[test]
    fn an_unreadable_row_does_not_poison_the_others() {
        let vault = vault();
        let mut broken = row(&vault, 2, "b", 1);
        broken.sealed_key.ciphertext[0] ^= 1;
        let mut corrupt_chain = row(&vault, 3, "c", 1);
        corrupt_chain.chain_pem = "garbage".into();
        let rows = [row(&vault, 1, "a", 1), broken, corrupt_chain];
        let (cache, stats) = CertCache::empty().rebuild(&rows, Some(&vault));
        assert_eq!(
            stats,
            RebuildStats {
                hits: 0,
                misses: 1,
                unreadable: 2
            }
        );
        assert!(cache.handle(CertificateId(1)).is_some());
        let reasons: Vec<UnreadableReason> = cache
            .entries()
            .filter_map(|entry| match entry {
                CertEntry::Unreadable(cert) => Some(cert.reason),
                CertEntry::Ready(_) => None,
            })
            .collect();
        assert_eq!(
            reasons,
            [UnreadableReason::Undecryptable, UnreadableReason::Corrupt]
        );
        assert!(cache.handle(CertificateId(2)).is_none());
    }

    #[test]
    fn a_wrong_master_key_marks_every_certificate_unreadable() {
        let rows = [row(&vault(), 1, "a", 1), row(&vault(), 2, "b", 1)];
        let (cache, stats) = CertCache::empty().rebuild(&rows, Some(&other_vault()));
        assert_eq!(
            stats,
            RebuildStats {
                hits: 0,
                misses: 0,
                unreadable: 2
            }
        );
        assert!(cache.entries().all(|entry| matches!(
            entry,
            CertEntry::Unreadable(cert) if cert.reason == UnreadableReason::Undecryptable
        )));
    }

    #[test]
    fn no_vault_means_no_master_key_and_unreadable_rows_keep_their_metadata() {
        let rows = [row(&vault(), 1, "a", 1)];
        let (cache, _) = CertCache::empty().rebuild(&rows, None);
        let Some(CertEntry::Unreadable(cert)) = cache.entry(CertificateId(1)) else {
            panic!("expected an unreadable entry");
        };
        assert_eq!(cert.reason, UnreadableReason::NoMasterKey);
        assert_eq!(cert.metadata.names, ["a.example.com"]);
        assert!(!cert.reason.describe().is_empty());
    }

    #[test]
    fn a_row_that_was_unreadable_is_retried_on_the_next_rebuild() {
        let vault = vault();
        let rows = [row(&vault, 1, "a", 1)];
        let (broken, _) = CertCache::empty().rebuild(&rows, None);
        let (fixed, stats) = broken.rebuild(&rows, Some(&vault));
        assert_eq!(stats.misses, 1);
        assert!(fixed.handle(CertificateId(1)).is_some());
    }

    #[test]
    fn a_rename_replaces_the_entry_even_if_the_timestamp_is_equal() {
        let vault = vault();
        let (first, _) = CertCache::empty().rebuild(&[row(&vault, 1, "a", 5)], Some(&vault));
        let (_, stats) = first.rebuild(&[row(&vault, 1, "b", 5)], Some(&vault));
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn rebuilding_256_certificates_stays_within_budget() {
        let vault = vault();
        let rows: Vec<StoredCertificate> = (1..=256)
            .map(|id| row(&vault, id, &format!("c{id}"), 1))
            .collect();
        let started = Instant::now();
        let (cache, stats) = CertCache::empty().rebuild(&rows, Some(&vault));
        let full = started.elapsed();
        assert_eq!(stats.misses, 256);
        let started = Instant::now();
        let (_, stats) = cache.rebuild(&rows, Some(&vault));
        let reused = started.elapsed();
        assert_eq!(stats.hits, 256);
        // Printed for docs/benchmarks.md: run with --release -- --nocapture.
        eprintln!("cache rebuild, 256 certificates: full parse {full:?}, all reused {reused:?}");
        assert!(full.as_millis() < 5_000, "full parse took {full:?}");
        assert!(reused.as_millis() < 250, "reuse took {reused:?}");
        assert!(!cache.is_empty());
    }

    #[test]
    fn handles_have_no_secret_debug_output() {
        let vault = vault();
        let (cache, _) = CertCache::empty().rebuild(&[row(&vault, 1, "a", 1)], Some(&vault));
        let text = format!("{:?}", cache.handle(CertificateId(1)).unwrap());
        assert!(text.contains("CertHandle") && !text.contains("PRIVATE"));
    }
}
