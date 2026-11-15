//! ACME coordination state (ADR 0026): the renewal lease, the per-subsystem
//! status and the account. None of it is configuration and none of it is
//! compiled, so it is written **without a revision** (a revision per lease
//! renewal would make every instance reload constantly), yet still only by
//! this service. Failures and a new account are audited.

use std::time::Duration;

use fast_proxy_model::{AcmeLease, AcmeStatus, SubsystemId};
use fast_proxy_store::repository::acme::account::{self, AcmeAccountRow};
use fast_proxy_store::repository::acme::{lease, status};
use fast_proxy_store::repository::audit::{self, AuditOutcome, NewAuditEntry};
use fast_proxy_store::{Connection, StoreError};
use fast_proxy_telemetry::AuditResult;
use fast_proxy_tls::Sealed;
use zeroize::Zeroizing;

use crate::service::now_ms;
use crate::ConfigService;

/// An ACME account's credentials (account URL and private key, as the
/// protocol client serialises them). Zeroed on drop; never printed.
pub struct AcmeAccountCredentials(Zeroizing<Vec<u8>>);

impl AcmeAccountCredentials {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The bytes, for the protocol client only.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for AcmeAccountCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcmeAccountCredentials(..)")
    }
}

/// A stored account, opened.
#[derive(Debug)]
pub struct AcmeAccount {
    pub directory: String,
    pub email: String,
    pub credentials: AcmeAccountCredentials,
}

/// The vault label an account's credentials are sealed under.
fn account_label(directory: &str) -> String {
    format!("acme-account:{directory}")
}

impl ConfigService {
    /// Takes or renews the renewal lease for this instance until `ttl` from
    /// now. Returns whether this instance holds it.
    pub async fn try_acme_lease(&self, ttl: Duration) -> Result<bool, StoreError> {
        let holder = self.instance.clone();
        let now = now_ms();
        let expires = now.saturating_add(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
        self.store
            .run(move |db| db.write_unrecorded(|conn| lease::try_take(conn, &holder, now, expires)))
            .await
    }

    /// Gives the lease up, if this instance holds it, so another can take it
    /// at once (on shutdown).
    pub async fn release_acme_lease(&self) -> Result<bool, StoreError> {
        let holder = self.instance.clone();
        self.store
            .run(move |db| db.write_unrecorded(|conn| lease::release(conn, &holder)))
            .await
    }

    /// The lease as stored, expired or not.
    pub async fn acme_lease(&self) -> Result<Option<AcmeLease>, StoreError> {
        Ok(self.read(lease::get).await?.value)
    }

    /// Stores a subsystem's status as written by this instance.
    pub async fn record_acme_status(&self, mut written: AcmeStatus) -> Result<(), StoreError> {
        written.instance = self.instance.clone();
        self.store
            .run(move |db| db.write_unrecorded(|conn| status::put(conn, &written)))
            .await
    }

    /// Stores a status after a failed order together with its audit entry
    /// (`certificate.acme.fail`, the status's sanitized error as the detail),
    /// in one transaction and without a revision.
    pub async fn record_acme_failure(&self, mut written: AcmeStatus) -> Result<(), StoreError> {
        written.instance = self.instance.clone();
        let entry = failure_entry(&written, &self.instance);
        self.store
            .run(move |db| {
                db.write_unrecorded(|conn| {
                    status::put(conn, &written)?;
                    audit::append(conn, &entry).map(drop)
                })
            })
            .await?;
        self.metrics.audit_entry(AuditResult::Failed);
        Ok(())
    }

    /// Every stored status.
    pub async fn acme_statuses(&self) -> Result<Vec<AcmeStatus>, StoreError> {
        Ok(self.read(status::list).await?.value)
    }

    /// The status of one subsystem, if one was written.
    pub async fn acme_status(
        &self,
        subsystem: SubsystemId,
    ) -> Result<Option<AcmeStatus>, StoreError> {
        Ok(self
            .read(move |conn| status::get(conn, subsystem))
            .await?
            .value)
    }

    /// Removes the status of a subsystem that stopped using ACME.
    pub async fn forget_acme_status(&self, subsystem: SubsystemId) -> Result<bool, StoreError> {
        self.store
            .run(move |db| db.write_unrecorded(|conn| status::delete(conn, subsystem)))
            .await
    }

    /// The account registered with `directory`, opened with the vault.
    pub async fn acme_account(&self, directory: &str) -> Result<Option<AcmeAccount>, StoreError> {
        let directory = directory.to_string();
        let tls = self.tls.clone();
        self.store
            .run(move |db| {
                db.read(|conn| {
                    let Some(row) = account::get(conn, &directory)? else {
                        return Ok(None);
                    };
                    let credentials = tls
                        .vault_for_sealing(conn)?
                        .open_secret(&account_label(&directory), &sealed_of(&row))
                        .map_err(|_| {
                            StoreError::Conflict("the stored ACME account cannot be read".into())
                        })?;
                    Ok(Some(AcmeAccount {
                        directory: row.directory,
                        email: row.email,
                        credentials: AcmeAccountCredentials(credentials),
                    }))
                })
            })
            .await
    }

    /// Seals and stores the account for `directory`, replacing an earlier one,
    /// with an audit entry (`acme.account.save`); no revision.
    pub async fn save_acme_account(
        &self,
        directory: &str,
        email: &str,
        credentials: &AcmeAccountCredentials,
    ) -> Result<(), StoreError> {
        let directory = directory.to_string();
        let email = email.to_string();
        let plaintext = Zeroizing::new(credentials.expose().to_vec());
        let tls = self.tls.clone();
        let instance = self.instance.clone();
        self.store
            .run(move |db| {
                db.write_unrecorded(|conn| {
                    let sealed = tls
                        .vault_for_sealing(conn)?
                        .seal_secret(&account_label(&directory), &plaintext)
                        .map_err(|err| StoreError::Internal(err.to_string()))?;
                    account::put(
                        conn,
                        &AcmeAccountRow {
                            directory: directory.clone(),
                            email,
                            credentials_enc: sealed.ciphertext,
                            credentials_nonce: sealed.nonce,
                        },
                    )?;
                    append_system_entry(
                        conn,
                        &instance,
                        "acme.account.save",
                        &directory,
                        AuditOutcome::Ok,
                        "{}",
                    )
                })
            })
            .await?;
        self.metrics.audit_entry(AuditResult::Ok);
        Ok(())
    }
}

fn sealed_of(row: &AcmeAccountRow) -> Sealed {
    Sealed {
        nonce: row.credentials_nonce.clone(),
        ciphertext: row.credentials_enc.clone(),
    }
}

fn failure_entry(written: &AcmeStatus, instance: &str) -> NewAuditEntry {
    system_entry(
        instance,
        "certificate.acme.fail",
        &format!("subsystem:{}", written.subsystem_id.0),
        AuditOutcome::Failed,
        &serde_json::json!({ "error": written.last_error, "state": written.state.as_str() })
            .to_string(),
    )
}

fn append_system_entry(
    conn: &Connection,
    instance: &str,
    action: &str,
    target: &str,
    outcome: AuditOutcome,
    detail: &str,
) -> Result<(), StoreError> {
    audit::append(
        conn,
        &system_entry(instance, action, target, outcome, detail),
    )
    .map(drop)
}

/// An audit entry by the `system` actor that is not a committed change.
fn system_entry(
    instance: &str,
    action: &str,
    target: &str,
    outcome: AuditOutcome,
    detail: &str,
) -> NewAuditEntry {
    NewAuditEntry {
        at_ms: now_ms(),
        instance: instance.to_string(),
        actor: "system".into(),
        role: "system".into(),
        source: "local".into(),
        request_id: String::new(),
        action: action.to_string(),
        target: target.to_string(),
        outcome,
        detail: detail.to_string(),
        revision: None,
    }
}
