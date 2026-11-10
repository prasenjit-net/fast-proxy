//! The service's TLS state: the process settings, the vault (master key) and
//! the certificate cache. Everything here is touched only under the writer
//! lock or from the store thread inside a write transaction.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use fast_proxy_store::repository::certificates::CertificateRow;
use fast_proxy_store::repository::vault::{self as vault_rows, VaultCheck};
use fast_proxy_store::repository::{ca_bundles, certificates, load};
use fast_proxy_store::{Connection, StoreError};
use fast_proxy_tls::{
    load_master_key, CertCache, RebuildStats, Sealed, StoredCertificate, Vault, VaultError,
};

use crate::dev_certificates;

/// How the service treats TLS, decided at startup.
#[derive(Debug, Clone, Default)]
pub struct TlsSettings {
    /// TLS is on for the process (the HTTPS listener runs).
    pub enabled: bool,
    /// Where the master key file lives; `None` means no file may be created,
    /// so sealing a key fails unless `env_master_key` is set.
    pub master_key_file: Option<PathBuf>,
    /// The value of `FAST_PROXY_MASTER_KEY`, if set.
    pub env_master_key: Option<String>,
}

/// Whether stored private keys can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultStatus {
    /// A master key is loaded and matches this database (or nothing is sealed yet).
    Ok,
    /// A master key is loaded but it is not the one this database was sealed with.
    KeyMismatch,
    /// There is no master key yet: none was needed so far.
    NotCreated,
}

impl VaultStatus {
    /// The API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::KeyMismatch => "key mismatch",
            Self::NotCreated => "not created",
        }
    }
}

pub(crate) struct VaultHolder {
    vault: Option<Arc<Vault>>,
    status: VaultStatus,
}

/// What a write prepared for the compile veto and, once committed, for the
/// service state.
pub(crate) struct Prepared {
    pub cache: CertCache,
    pub stats: RebuildStats,
    pub ca_bundles: HashSet<String>,
}

pub(crate) struct TlsState {
    pub settings: TlsSettings,
    vault: Mutex<VaultHolder>,
    cache: Mutex<CertCache>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn vault_error(err: &VaultError) -> StoreError {
    StoreError::Conflict(format!("cannot use the master key: {err}"))
}

impl TlsState {
    /// Loads the master key when one is needed (TLS on, or certificates stored)
    /// and checks it against the stored check value. A mismatch is reported,
    /// never fatal: the vault stays unavailable and keys read as unreadable.
    pub(crate) fn open(
        settings: TlsSettings,
        has_certificates: bool,
        check: Option<&VaultCheck>,
    ) -> Result<Self, StoreError> {
        let mut holder = VaultHolder {
            vault: None,
            status: VaultStatus::NotCreated,
        };
        if settings.enabled || has_certificates {
            if let Some(loaded) = find_key(&settings, false)? {
                let vault = Vault::new(&loaded.key);
                let matches = check.is_none_or(|check| vault.verify_check(&sealed_of(check)));
                if matches {
                    holder.vault = Some(Arc::new(vault));
                    holder.status = VaultStatus::Ok;
                } else {
                    tracing::error!("the master key does not match this database; stored private keys are unreadable");
                    holder.status = VaultStatus::KeyMismatch;
                }
            }
        }
        Ok(Self {
            settings,
            vault: Mutex::new(holder),
            cache: Mutex::new(CertCache::empty()),
        })
    }

    pub(crate) fn status(&self) -> VaultStatus {
        locked(&self.vault).status
    }

    fn current_vault(&self) -> Option<Arc<Vault>> {
        locked(&self.vault).vault.clone()
    }

    /// The vault for sealing a key, creating the master key (and the stored
    /// check value, in the caller's transaction) the first time one is needed.
    pub(crate) fn vault_for_sealing(&self, conn: &Connection) -> Result<Arc<Vault>, StoreError> {
        let mut holder = locked(&self.vault);
        if holder.status == VaultStatus::KeyMismatch {
            return Err(StoreError::Conflict(
                "the master key does not match this database; restore the right key before storing private keys".into(),
            ));
        }
        let vault = match holder.vault.clone() {
            Some(vault) => vault,
            None => {
                let loaded = find_key(&self.settings, true)?.ok_or_else(|| {
                    StoreError::Conflict(
                        "no master key is configured, so a private key cannot be stored".into(),
                    )
                })?;
                let vault = Arc::new(Vault::new(&loaded.key));
                holder.vault = Some(vault.clone());
                holder.status = VaultStatus::Ok;
                vault
            }
        };
        if vault_rows::get(conn)?.is_none() {
            let sealed = vault.create_check().map_err(|err| vault_error(&err))?;
            vault_rows::set(
                conn,
                &VaultCheck {
                    nonce: sealed.nonce,
                    blob: sealed.ciphertext,
                },
            )?;
        }
        Ok(vault)
    }

    /// Runs inside a write transaction after the caller's mutation: fills
    /// certificate gaps (TLS on only), then builds the would-be cache.
    pub(crate) fn prepare(&self, conn: &Connection, now_ms: i64) -> Result<Prepared, StoreError> {
        if self.settings.enabled {
            dev_certificates::reconcile(conn, self, now_ms)?;
        }
        self.build(conn)
    }

    /// Builds the cache for the rows as they are, reusing unchanged entries.
    pub(crate) fn build(&self, conn: &Connection) -> Result<Prepared, StoreError> {
        let rows: Vec<StoredCertificate> = certificates::list(conn)?
            .into_iter()
            .map(stored_certificate)
            .collect();
        let previous = locked(&self.cache).clone();
        let vault = self.current_vault();
        let (cache, stats) = previous.rebuild(&rows, vault.as_deref());
        let ca_bundles = ca_bundles::list(conn)?
            .into_iter()
            .map(|row| row.bundle.name.as_str().to_string())
            .collect();
        Ok(Prepared {
            cache,
            stats,
            ca_bundles,
        })
    }

    /// Reads the would-be configuration inside the transaction.
    pub(crate) fn config(conn: &Connection) -> Result<fast_proxy_model::Config, StoreError> {
        load::load_all(conn)
    }

    /// Keeps the cache of a committed write for the next rebuild to reuse.
    pub(crate) fn install(&self, cache: CertCache) {
        *locked(&self.cache) = cache;
    }

    /// The cache of the live snapshot.
    pub(crate) fn cache(&self) -> CertCache {
        locked(&self.cache).clone()
    }
}

fn find_key(
    settings: &TlsSettings,
    create: bool,
) -> Result<Option<fast_proxy_tls::LoadedMasterKey>, StoreError> {
    let env = settings.env_master_key.as_deref();
    match &settings.master_key_file {
        Some(file) => load_master_key(env, file, create).map_err(|err| vault_error(&err)),
        None => match env.filter(|text| !text.trim().is_empty()) {
            Some(_) => load_master_key(env, std::path::Path::new(""), false)
                .map_err(|err| vault_error(&err)),
            None => Ok(None),
        },
    }
}

fn sealed_of(check: &VaultCheck) -> Sealed {
    Sealed {
        nonce: check.nonce.clone(),
        ciphertext: check.blob.clone(),
    }
}

/// A store row in the form the cache reads.
pub(crate) fn stored_certificate(row: CertificateRow) -> StoredCertificate {
    let certificate = row.certificate;
    StoredCertificate {
        id: row.id,
        name: certificate.name,
        source: certificate.source,
        chain_pem: certificate.chain_pem,
        sealed_key: Sealed {
            nonce: certificate.key_nonce,
            ciphertext: certificate.key_enc,
        },
        metadata: certificate.metadata,
        updated_at_ms: row.updated_at_ms,
    }
}
