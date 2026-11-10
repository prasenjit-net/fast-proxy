//! The certificate guarantee (ADR 0010): while TLS is on, a subsystem with
//! HTTPS enabled always has a certificate. A subsystem without one gets a
//! self-signed development certificate `dev-<subsystem>`, generated and attached
//! in the same write; a dev certificate that stops covering the subsystem's
//! hosts, or has under 30 days left, is regenerated in place. An uploaded
//! certificate is never replaced.

use fast_proxy_model::{
    CertificateId, CertificateName, CertificateSource, Config, HostPattern, HttpsPolicy,
    SubsystemConfig,
};
use fast_proxy_store::repository::certificates::{self, CertificateRow, NewCertificate};
use fast_proxy_store::repository::subsystems;
use fast_proxy_store::{Connection, StoreError, SubsystemFields};
use fast_proxy_tls::{generate_self_signed, parse_chain, read_metadata};

use crate::tls_state::TlsState;

const REGENERATE_BELOW_MS: i64 = 30 * 86_400_000;

/// One development certificate to create or refresh.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Generate {
    pub subsystem: SubsystemConfig,
    /// A stored dev certificate to replace in place, if there is one.
    pub existing: Option<CertificateId>,
}

/// The name of a subsystem's development certificate.
pub(crate) fn dev_name(subsystem: &SubsystemConfig) -> String {
    format!("dev-{}", subsystem.name.as_str())
}

/// Decides which development certificates are needed. Pure: the clock is passed in.
pub(crate) fn plan(config: &Config, rows: &[CertificateRow], now_ms: i64) -> Vec<Generate> {
    let mut actions = Vec::new();
    for subsystem in &config.subsystems {
        if !subsystem.https.enabled || subsystem.hosts.is_empty() {
            continue;
        }
        let dev = dev_name(subsystem);
        let own_dev = rows.iter().find(|row| {
            row.certificate.name.as_str() == dev
                && row.certificate.source == CertificateSource::SelfSigned
        });
        match subsystem.https.certificate_id {
            None => actions.push(Generate {
                subsystem: subsystem.clone(),
                existing: own_dev.map(|row| row.id),
            }),
            Some(id) => {
                let Some(row) = own_dev.filter(|row| row.id == id) else {
                    continue;
                };
                if needs_refresh(row, &subsystem.hosts, now_ms) {
                    actions.push(Generate {
                        subsystem: subsystem.clone(),
                        existing: Some(id),
                    });
                }
            }
        }
    }
    actions
}

fn needs_refresh(row: &CertificateRow, hosts: &[HostPattern], now_ms: i64) -> bool {
    let metadata = &row.certificate.metadata;
    metadata.not_after_ms - now_ms < REGENERATE_BELOW_MS
        || !hosts.iter().all(|host| metadata.covers(host))
}

/// Executes the plan for the would-be configuration inside the transaction.
pub(crate) fn reconcile(conn: &Connection, tls: &TlsState, now_ms: i64) -> Result<(), StoreError> {
    let config = TlsState::config(conn)?;
    let rows = certificates::list(conn)?;
    for action in plan(&config, &rows, now_ms) {
        generate(conn, tls, &action, &rows, now_ms)?;
    }
    Ok(())
}

fn generate(
    conn: &Connection,
    tls: &TlsState,
    action: &Generate,
    rows: &[CertificateRow],
    now_ms: i64,
) -> Result<(), StoreError> {
    let subsystem = &action.subsystem;
    let names: Vec<String> = subsystem
        .hosts
        .iter()
        .map(HostPattern::to_pattern_string)
        .collect();
    let generated = generate_self_signed(&names, now_ms).map_err(|err| {
        StoreError::Internal(format!("cannot generate a development certificate: {err}"))
    })?;
    let metadata = read_metadata(&parse_chain(&generated.chain_pem)?[0])?;
    let vault = tls.vault_for_sealing(conn)?;
    let (id, name) = match action.existing {
        Some(id) => (id, existing_name(rows, id)),
        None => (
            certificates::next_id(conn)?,
            free_name(rows, &dev_name(subsystem)),
        ),
    };
    let sealed = vault
        .seal_key(id, &name, &generated.key.to_sealed_bytes())
        .map_err(|err| StoreError::Internal(err.to_string()))?;
    let certificate = NewCertificate {
        name: CertificateName::parse(&name)?,
        source: CertificateSource::SelfSigned,
        chain_pem: generated.chain_pem,
        key_enc: sealed.ciphertext,
        key_nonce: sealed.nonce,
        metadata,
    };
    if action.existing.is_some() {
        certificates::update(conn, id, &certificate)?;
    } else {
        certificates::insert(conn, id, &certificate)?;
    }
    attach(conn, subsystem, id)
}

fn existing_name(rows: &[CertificateRow], id: CertificateId) -> String {
    rows.iter()
        .find(|row| row.id == id)
        .map(|row| row.certificate.name.as_str().to_string())
        .unwrap_or_default()
}

/// `base`, or `base-2`, `base-3`... when an admin-made certificate holds the name.
fn free_name(rows: &[CertificateRow], base: &str) -> String {
    let taken = |name: &str| rows.iter().any(|row| row.certificate.name.as_str() == name);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_string())
}

fn attach(
    conn: &Connection,
    subsystem: &SubsystemConfig,
    id: CertificateId,
) -> Result<(), StoreError> {
    subsystems::update(
        conn,
        subsystem.id,
        &SubsystemFields {
            name: subsystem.name.clone(),
            enabled: subsystem.enabled,
            description: subsystem.description.clone(),
            https: HttpsPolicy {
                certificate_id: Some(id),
                ..subsystem.https
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{CertificateMetadata, HostPattern, SubsystemId, SubsystemName};

    use super::*;

    const DAY: i64 = 86_400_000;
    const NOW: i64 = 1_800_000_000_000;

    fn subsystem(https: HttpsPolicy, hosts: &[&str]) -> SubsystemConfig {
        SubsystemConfig {
            id: SubsystemId(1),
            name: SubsystemName::parse("shop").unwrap(),
            enabled: true,
            description: String::new(),
            https,
            hosts: hosts
                .iter()
                .map(|h| HostPattern::parse(h).unwrap())
                .collect(),
            upstream_groups: Vec::new(),
            routes: Vec::new(),
        }
    }

    fn row(
        id: i64,
        name: &str,
        source: CertificateSource,
        names: &[&str],
        left_days: i64,
    ) -> CertificateRow {
        CertificateRow {
            id: CertificateId(id),
            certificate: NewCertificate {
                name: CertificateName::parse(name).unwrap(),
                source,
                chain_pem: String::new(),
                key_enc: Vec::new(),
                key_nonce: Vec::new(),
                metadata: CertificateMetadata {
                    names: names.iter().map(ToString::to_string).collect(),
                    subject: String::new(),
                    issuer: String::new(),
                    not_before_ms: NOW - DAY,
                    not_after_ms: NOW + left_days * DAY,
                    fingerprint: String::new(),
                },
            },
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn https(certificate: Option<i64>) -> HttpsPolicy {
        HttpsPolicy {
            enabled: true,
            certificate_id: certificate.map(CertificateId),
            ..HttpsPolicy::default()
        }
    }

    fn plan_for(subsystem: SubsystemConfig, rows: &[CertificateRow]) -> Vec<Generate> {
        plan(
            &Config {
                subsystems: vec![subsystem],
            },
            rows,
            NOW,
        )
    }

    #[test]
    fn a_subsystem_with_https_and_no_certificate_gets_one() {
        let actions = plan_for(subsystem(https(None), &["shop.example.com"]), &[]);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].existing, None);
    }

    #[test]
    fn nothing_is_planned_without_https_or_without_hosts() {
        assert!(plan_for(subsystem(HttpsPolicy::default(), &["a.example.com"]), &[]).is_empty());
        assert!(plan_for(subsystem(https(None), &[]), &[]).is_empty());
    }

    #[test]
    fn a_healthy_dev_certificate_is_left_alone_until_hosts_or_expiry_change() {
        let dev = row(
            1,
            "dev-shop",
            CertificateSource::SelfSigned,
            &["shop.example.com"],
            300,
        );
        let rows = [dev];
        assert!(plan_for(subsystem(https(Some(1)), &["shop.example.com"]), &rows).is_empty());
        let wider = plan_for(
            subsystem(https(Some(1)), &["shop.example.com", "new.example.com"]),
            &rows,
        );
        assert_eq!(wider[0].existing, Some(CertificateId(1)));
    }

    #[test]
    fn a_dev_certificate_with_under_thirty_days_left_is_regenerated() {
        let rows = [row(
            1,
            "dev-shop",
            CertificateSource::SelfSigned,
            &["shop.example.com"],
            29,
        )];
        let actions = plan_for(subsystem(https(Some(1)), &["shop.example.com"]), &rows);
        assert_eq!(actions[0].existing, Some(CertificateId(1)));
        let rows = [row(
            1,
            "dev-shop",
            CertificateSource::SelfSigned,
            &["shop.example.com"],
            31,
        )];
        assert!(plan_for(subsystem(https(Some(1)), &["shop.example.com"]), &rows).is_empty());
    }

    #[test]
    fn an_uploaded_or_foreign_certificate_is_never_replaced() {
        let uploaded = [row(
            1,
            "dev-shop",
            CertificateSource::Uploaded,
            &["other.example.com"],
            1,
        )];
        assert!(plan_for(subsystem(https(Some(1)), &["shop.example.com"]), &uploaded).is_empty());
        let generated_by_hand = [row(
            1,
            "scratch",
            CertificateSource::SelfSigned,
            &["other.example.com"],
            1,
        )];
        assert!(plan_for(
            subsystem(https(Some(1)), &["shop.example.com"]),
            &generated_by_hand
        )
        .is_empty());
    }

    #[test]
    fn detaching_replaces_the_stored_dev_certificate_in_place() {
        let rows = [row(
            4,
            "dev-shop",
            CertificateSource::SelfSigned,
            &["shop.example.com"],
            300,
        )];
        let actions = plan_for(subsystem(https(None), &["shop.example.com"]), &rows);
        assert_eq!(actions[0].existing, Some(CertificateId(4)));
    }

    #[test]
    fn a_taken_name_gets_a_numbered_variant() {
        let rows = [
            row(
                1,
                "dev-shop",
                CertificateSource::Uploaded,
                &["a.example.com"],
                10,
            ),
            row(
                2,
                "dev-shop-2",
                CertificateSource::Uploaded,
                &["a.example.com"],
                10,
            ),
        ];
        assert_eq!(free_name(&rows, "dev-shop"), "dev-shop-3");
        assert_eq!(free_name(&rows, "dev-blog"), "dev-blog");
        assert_eq!(existing_name(&rows, CertificateId(2)), "dev-shop-2");
    }
}
