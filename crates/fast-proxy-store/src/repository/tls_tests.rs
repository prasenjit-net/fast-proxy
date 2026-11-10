//! Certificates, CA bundles, the vault and the HTTPS and upstream-TLS columns.

use fast_proxy_model::{
    CertificateId, CertificateMetadata, CertificateName, CertificateSource, Hsts, HttpsPolicy,
    RedirectMode, UpstreamTls, MAX_CA_BUNDLES, MAX_CERTIFICATES,
};

use super::ca_bundles::{self, NewCaBundle};
use super::certificates::{self, NewCertificate};
use super::vault::{self, VaultCheck};
use super::{subsystems, upstreams};
use crate::test_support::{commit, seed, subsystem_fields, upstream_fields};
use crate::{Database, StoreError, SubsystemFields};

const SECRET: &[u8] = b"sealed-key-bytes";

fn certificate(name: &str) -> NewCertificate {
    NewCertificate {
        name: CertificateName::parse(name).unwrap(),
        source: CertificateSource::Uploaded,
        chain_pem: "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".into(),
        key_enc: SECRET.to_vec(),
        key_nonce: vec![7; 12],
        metadata: CertificateMetadata {
            names: vec![format!("{name}.example.com"), "*.wild.example.com".into()],
            subject: "CN=test".into(),
            issuer: "CN=issuer".into(),
            not_before_ms: 1_000,
            not_after_ms: 2_000,
            fingerprint: "ab".repeat(32),
        },
    }
}

fn add_certificate(db: &mut Database, name: &str) -> CertificateId {
    commit(db, "certificate uploaded", |conn| {
        let id = certificates::next_id(conn)?;
        certificates::insert(conn, id, &certificate(name))?;
        Ok(id)
    })
    .unwrap()
    .value
}

fn bundle(name: &str) -> NewCaBundle {
    NewCaBundle {
        name: CertificateName::parse(name).unwrap(),
        pem: "-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----\n".into(),
        cert_count: 1,
    }
}

fn https_on(certificate: Option<CertificateId>) -> HttpsPolicy {
    HttpsPolicy {
        enabled: true,
        certificate_id: certificate,
        redirect: RedirectMode::Permanent308,
        hsts: Hsts {
            enabled: true,
            max_age_secs: 600,
            include_subdomains: true,
        },
    }
}

#[test]
fn a_certificate_round_trips_with_its_metadata_and_opaque_key() {
    let mut db = Database::open_in_memory().unwrap();
    let id = add_certificate(&mut db, "shop");
    let row = db.read(|conn| certificates::get(conn, id)).unwrap();
    assert_eq!(row.certificate.name.as_str(), "shop");
    assert_eq!(row.certificate.source, CertificateSource::Uploaded);
    assert_eq!(row.certificate.key_enc, SECRET);
    assert_eq!(row.certificate.key_nonce, vec![7; 12]);
    assert_eq!(row.certificate.metadata, certificate("shop").metadata);
    assert_eq!(db.read(certificates::list).unwrap().len(), 1);
}

#[test]
fn a_duplicate_certificate_name_is_a_conflict() {
    let mut db = Database::open_in_memory().unwrap();
    add_certificate(&mut db, "shop");
    let err = commit(&mut db, "dup", |conn| {
        let id = certificates::next_id(conn)?;
        certificates::insert(conn, id, &certificate("shop"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)), "{err}");
}

#[test]
fn replacing_a_certificate_bumps_updated_at_and_keeps_the_id() {
    let mut db = Database::open_in_memory().unwrap();
    let id = add_certificate(&mut db, "shop");
    let before = db.read(|conn| certificates::get(conn, id)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mut renewed = certificate("shop");
    renewed.metadata.fingerprint = "cd".repeat(32);
    renewed.key_enc = b"another".to_vec();
    commit(&mut db, "replaced", |conn| {
        certificates::update(conn, id, &renewed)
    })
    .unwrap();
    let after = db.read(|conn| certificates::get(conn, id)).unwrap();
    assert_eq!(after.id, id);
    assert!(after.updated_at_ms > before.updated_at_ms);
    assert_eq!(after.created_at_ms, before.created_at_ms);
    assert_eq!(after.certificate.metadata.fingerprint, "cd".repeat(32));
    assert_eq!(after.certificate.key_enc, b"another");
}

#[test]
fn updating_or_deleting_a_missing_certificate_is_not_found() {
    let mut db = Database::open_in_memory().unwrap();
    let err = commit(&mut db, "x", |conn| {
        certificates::update(conn, CertificateId(9), &certificate("a"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
    let err = commit(&mut db, "x", |conn| {
        certificates::delete(conn, CertificateId(9))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn a_certificate_in_use_cannot_be_deleted_until_detached() {
    let mut db = Database::open_in_memory().unwrap();
    let certificate_id = add_certificate(&mut db, "shop");
    let site = seed(&mut db, "shop", "shop.example.com");
    commit(&mut db, "attach", |conn| {
        subsystems::update(
            conn,
            site.subsystem,
            &SubsystemFields {
                https: https_on(Some(certificate_id)),
                ..subsystem_fields("shop")
            },
        )
    })
    .unwrap();

    let err = commit(&mut db, "delete", |conn| {
        certificates::delete(conn, certificate_id)
    })
    .unwrap_err();
    assert!(
        matches!(&err, StoreError::Conflict(m) if m.contains("shop")),
        "{err}"
    );
    let used = db.read(certificates::used_by).unwrap();
    assert_eq!(used[&certificate_id], vec![site.subsystem]);

    commit(&mut db, "detach", |conn| {
        subsystems::update(conn, site.subsystem, &subsystem_fields("shop"))
    })
    .unwrap();
    commit(&mut db, "delete", |conn| {
        certificates::delete(conn, certificate_id)
    })
    .unwrap();
    assert!(db.read(certificates::list).unwrap().is_empty());
}

#[test]
fn a_subsystem_cannot_reference_a_missing_certificate() {
    let mut db = Database::open_in_memory().unwrap();
    let err = commit(&mut db, "x", |conn| {
        subsystems::create(
            conn,
            &SubsystemFields {
                https: https_on(Some(CertificateId(42))),
                ..subsystem_fields("shop")
            },
        )
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)), "{err}");
}

#[test]
fn certificate_limits_are_enforced() {
    let mut db = Database::open_in_memory().unwrap();
    let mut oversized = certificate("big");
    oversized.chain_pem = "x".repeat(256 * 1024 + 1);
    let err = commit(&mut db, "x", |conn| {
        let id = certificates::next_id(conn)?;
        certificates::insert(conn, id, &oversized)
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "certPem"));

    let mut long_chain = certificate("chain");
    long_chain.chain_pem = "-----BEGIN CERTIFICATE-----\n".repeat(11);
    let err = commit(&mut db, "x", |conn| {
        let id = certificates::next_id(conn)?;
        certificates::insert(conn, id, &long_chain)
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "certPem"));

    commit(&mut db, "fill", |conn| {
        for index in 0..MAX_CERTIFICATES {
            let id = certificates::next_id(conn)?;
            certificates::insert(conn, id, &certificate(&format!("c{index}")))?;
        }
        Ok(())
    })
    .unwrap();
    let err = commit(&mut db, "one more", |conn| {
        let id = certificates::next_id(conn)?;
        certificates::insert(conn, id, &certificate("extra"))
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].message.contains("256")));
}

#[test]
fn ca_bundles_round_trip_and_enforce_uniqueness_and_limits() {
    let mut db = Database::open_in_memory().unwrap();
    let id = commit(&mut db, "bundle", |conn| {
        ca_bundles::insert(conn, &bundle("internal"))
    })
    .unwrap()
    .value;
    let row = db.read(|conn| ca_bundles::get(conn, id)).unwrap();
    assert_eq!(row.bundle.name.as_str(), "internal");
    assert_eq!(row.bundle.cert_count, 1);
    let err = commit(&mut db, "dup", |conn| {
        ca_bundles::insert(conn, &bundle("internal"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));

    commit(&mut db, "fill", |conn| {
        for index in 1..MAX_CA_BUNDLES {
            ca_bundles::insert(conn, &bundle(&format!("b{index}")))?;
        }
        Ok(())
    })
    .unwrap();
    let err = commit(&mut db, "more", |conn| {
        ca_bundles::insert(conn, &bundle("extra"))
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].message.contains("16")));
}

#[test]
fn a_ca_bundle_named_by_a_group_cannot_be_deleted_or_renamed() {
    let mut db = Database::open_in_memory().unwrap();
    let id = commit(&mut db, "bundle", |conn| {
        ca_bundles::insert(conn, &bundle("internal"))
    })
    .unwrap()
    .value;
    let site = seed(&mut db, "shop", "shop.example.com");
    let mut fields = upstream_fields("backend");
    fields.policy.tls = UpstreamTls {
        enabled: true,
        ca: "internal".into(),
        ..UpstreamTls::default()
    };
    commit(&mut db, "tls", |conn| {
        upstreams::update(conn, site.subsystem, site.group, &fields)
    })
    .unwrap();

    for attempt in 0..2 {
        let err = commit(&mut db, "x", |conn| {
            if attempt == 0 {
                ca_bundles::delete(conn, id)
            } else {
                ca_bundles::update(conn, id, &bundle("renamed"))
            }
        })
        .unwrap_err();
        assert!(
            matches!(&err, StoreError::Conflict(m) if m.contains("internal")),
            "{err}"
        );
    }
}

#[test]
fn an_unused_ca_bundle_can_be_replaced_and_deleted() {
    let mut db = Database::open_in_memory().unwrap();
    let id = commit(&mut db, "bundle", |conn| {
        ca_bundles::insert(conn, &bundle("internal"))
    })
    .unwrap()
    .value;
    let mut replacement = bundle("internal");
    replacement.cert_count = 3;
    commit(&mut db, "replace", |conn| {
        ca_bundles::update(conn, id, &replacement)
    })
    .unwrap();
    assert_eq!(
        db.read(|conn| ca_bundles::get(conn, id))
            .unwrap()
            .bundle
            .cert_count,
        3
    );
    commit(&mut db, "delete", |conn| ca_bundles::delete(conn, id)).unwrap();
    assert!(db.read(ca_bundles::list).unwrap().is_empty());
}

#[test]
fn the_vault_row_is_stored_and_replaced() {
    let mut db = Database::open_in_memory().unwrap();
    assert!(db.read(vault::get).unwrap().is_none());
    for blob in [vec![1, 2], vec![3]] {
        let check = VaultCheck {
            nonce: vec![9; 12],
            blob,
        };
        commit(&mut db, "vault", |conn| vault::set(conn, &check)).unwrap();
        assert_eq!(db.read(vault::get).unwrap().unwrap(), check);
    }
}

#[test]
fn https_and_tls_settings_are_stored_and_loaded() {
    let mut db = Database::open_in_memory().unwrap();
    let certificate_id = add_certificate(&mut db, "shop");
    let site = seed(&mut db, "shop", "shop.example.com");
    let tls = UpstreamTls {
        enabled: true,
        verify: false,
        server_name: "backend.internal".into(),
        ca: String::new(),
    };
    let mut fields = upstream_fields("backend");
    fields.policy.tls = tls.clone();
    commit(&mut db, "edit", |conn| {
        subsystems::update(
            conn,
            site.subsystem,
            &SubsystemFields {
                https: https_on(Some(certificate_id)),
                ..subsystem_fields("shop")
            },
        )?;
        upstreams::update(conn, site.subsystem, site.group, &fields)
    })
    .unwrap();
    let loaded = db
        .read(|conn| subsystems::get(conn, site.subsystem))
        .unwrap();
    assert_eq!(loaded.https, https_on(Some(certificate_id)));
    assert_eq!(loaded.upstream_groups[0].policy.tls, tls);
}

#[test]
fn invalid_https_or_tls_settings_are_rejected_with_their_field() {
    let mut db = Database::open_in_memory().unwrap();
    let err = commit(&mut db, "x", |conn| {
        subsystems::create(
            conn,
            &SubsystemFields {
                https: HttpsPolicy {
                    redirect: RedirectMode::Permanent301,
                    ..HttpsPolicy::default()
                },
                ..subsystem_fields("shop")
            },
        )
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "https.redirect"));

    let site = seed(&mut db, "ok", "ok.example.com");
    let mut fields = upstream_fields("backend");
    fields.policy.tls.server_name = "bad name".into();
    let err = commit(&mut db, "x", |conn| {
        upstreams::update(conn, site.subsystem, site.group, &fields)
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "tls.serverName"));
}

#[test]
fn a_corrupt_stored_redirect_is_reported_as_internal() {
    let mut db = Database::open_in_memory().unwrap();
    seed(&mut db, "shop", "shop.example.com");
    db.read(|conn| {
        conn.execute("UPDATE subsystem SET https_redirect = 'bogus'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.load_all().unwrap_err(),
        StoreError::Internal(_)
    ));
}

#[test]
fn a_group_may_only_name_an_uploaded_ca_bundle() {
    let mut db = Database::open_in_memory().unwrap();
    let site = seed(&mut db, "shop", "shop.example.com");
    let mut fields = upstream_fields("backend");
    fields.policy.tls.enabled = true;
    fields.policy.tls.ca = "corp".into();
    let err = commit(&mut db, "x", |conn| {
        upstreams::update(conn, site.subsystem, site.group, &fields)
    })
    .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(f) if f[0].field == "tls.ca"),
        "{err}"
    );
    let err = commit(&mut db, "x", |conn| {
        upstreams::create(
            conn,
            site.subsystem,
            &fields,
            &[crate::test_support::endpoint_fields("127.0.0.1:3001")],
        )
    })
    .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(f) if f[0].field == "tls.ca"),
        "{err}"
    );

    commit(&mut db, "bundle", |conn| {
        ca_bundles::insert(conn, &bundle("corp"))
    })
    .unwrap();
    commit(&mut db, "x", |conn| {
        upstreams::update(conn, site.subsystem, site.group, &fields)
    })
    .unwrap();
}
