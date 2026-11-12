use fast_proxy_model::{Config, EndpointState, LbAlgorithm, RetryPolicy};

use super::{get, list};
use crate::repository::{endpoints, routes, subsystems, upstreams};
use crate::test_support::{
    commit, endpoint_fields, route_fields, seed, subsystem_fields, upstream_fields,
};
use crate::{Database, Mutation, StoreError};

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

fn restore_as_new_revision(db: &mut Database, version: u64) -> u64 {
    db.write(
        |conn| {
            super::restore(conn, version)?;
            Ok(Mutation {
                value: (),
                summary: format!("rollback to v{version}"),
            })
        },
        |_| Ok(()),
    )
    .unwrap()
    .version
}

#[test]
fn a_fresh_database_has_no_revisions() {
    let db = db();
    assert_eq!(db.current_version().unwrap(), 0);
    assert!(db.read(|c| list(c, 10, None)).unwrap().is_empty());
}

#[test]
fn every_write_appends_a_revision_with_its_summary_and_config() {
    let mut db = db();
    let written = commit(&mut db, "subsystem created: shop", |c| {
        subsystems::create(c, &subsystem_fields("shop"))
    })
    .unwrap();
    assert_eq!(written.version, 1);
    assert_eq!(written.config.subsystems.len(), 1);

    commit(&mut db, "subsystem deleted: shop", |c| {
        subsystems::delete(c, written.value)
    })
    .unwrap();
    assert_eq!(db.current_version().unwrap(), 2);

    let first = db.read(|c| get(c, 1)).unwrap();
    assert_eq!(first.summary, "subsystem created: shop");
    assert_eq!(first.config.subsystems[0].name.as_str(), "shop");
    let second = db.read(|c| get(c, 2)).unwrap();
    assert!(second.config.subsystems.is_empty());
}

#[test]
fn failed_writes_leave_no_revision() {
    let mut db = db();
    seed(&mut db, "shop", "s.example.com");
    let before = db.current_version().unwrap();
    commit(&mut db, "dup", |c| {
        subsystems::create(c, &subsystem_fields("shop"))
    })
    .unwrap_err();
    assert_eq!(db.current_version().unwrap(), before);
}

#[test]
fn lists_newest_first_and_pages_with_before() {
    let mut db = db();
    for name in ["a", "b", "c", "d"] {
        commit(&mut db, &format!("create {name}"), |c| {
            subsystems::create(c, &subsystem_fields(name))
        })
        .unwrap();
    }
    let page = db.read(|c| list(c, 2, None)).unwrap();
    let versions: Vec<u64> = page.iter().map(|r| r.version).collect();
    assert_eq!(versions, [4, 3]);
    assert_eq!(page[0].summary, "create d");

    let next = db.read(|c| list(c, 2, Some(3))).unwrap();
    let versions: Vec<u64> = next.iter().map(|r| r.version).collect();
    assert_eq!(versions, [2, 1]);
}

#[test]
fn a_missing_revision_is_not_found() {
    let db = db();
    let err = db.read(|c| get(c, 5)).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
    let err = db.read(|c| super::restore(c, 5)).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn only_the_last_100_revisions_are_kept() {
    let mut db = db();
    let id = commit(&mut db, "create", |c| {
        subsystems::create(c, &subsystem_fields("shop"))
    })
    .unwrap()
    .value;
    for n in 0..109 {
        commit(&mut db, &format!("edit {n}"), |c| {
            let mut fields = subsystem_fields("shop");
            fields.description = n.to_string();
            subsystems::update(c, id, &fields)
        })
        .unwrap();
    }
    assert_eq!(db.current_version().unwrap(), 110);
    let kept = db.read(|c| list(c, 500, None)).unwrap();
    assert_eq!(kept.len(), 100);
    assert_eq!(kept.last().unwrap().version, 11);
    assert!(db.read(|c| get(c, 10)).is_err());
}

#[test]
fn rollback_restores_each_prior_version_exactly() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com"); // v1
    let route = commit(&mut db, "route", |c| {
        routes::create(c, a.subsystem, &route_fields("/api", a.group))
    })
    .unwrap(); // v2
    commit(&mut db, "edit", |c| {
        let mut fields = subsystem_fields("alpha");
        fields.enabled = false;
        subsystems::update(c, a.subsystem, &fields)
    })
    .unwrap(); // v3
    commit(&mut db, "route removed", |c| {
        routes::delete(c, a.subsystem, route.value)
    })
    .unwrap(); // v4
    seed(&mut db, "beta", "b.example.com"); // v5
    commit(&mut db, "delete alpha", |c| {
        subsystems::delete(c, a.subsystem)
    })
    .unwrap(); // v6

    let stored: Vec<Config> = (1..=6)
        .map(|v| db.read(|c| get(c, v)).unwrap().config)
        .collect();
    for version in (1..=6u64).rev() {
        let new_version = restore_as_new_revision(&mut db, version);
        assert_eq!(
            db.load_all().unwrap(),
            stored[(version - 1) as usize],
            "after rolling back to v{version}"
        );
        assert_eq!(db.current_version().unwrap(), new_version);
        let latest = db.read(|c| get(c, new_version)).unwrap();
        assert_eq!(latest.summary, format!("rollback to v{version}"));
        assert_eq!(latest.config, stored[(version - 1) as usize]);
    }
}

#[test]
fn rollback_keeps_ids_so_new_rows_never_reuse_them() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    commit(&mut db, "delete", |c| subsystems::delete(c, a.subsystem)).unwrap();
    restore_as_new_revision(&mut db, 1);
    let restored = db.load_all().unwrap();
    assert_eq!(restored.subsystems[0].id, a.subsystem);
    assert_eq!(restored.subsystems[0].routes.len(), 1);
}

#[test]
fn an_unreadable_stored_revision_is_an_internal_error() {
    let mut db = db();
    seed(&mut db, "alpha", "a.example.com");
    db.read(|c| {
        c.execute("UPDATE revision SET config = 'not json'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.read(|c| get(c, 1)).unwrap_err(),
        StoreError::Internal(_)
    ));
}

#[test]
fn an_invalid_value_inside_a_stored_revision_is_an_internal_error() {
    let mut db = db();
    seed(&mut db, "alpha", "a.example.com");
    db.read(|c| {
        c.execute(
            "UPDATE revision SET config = replace(config, '\"alpha\"', '\"Bad Name\"')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.read(|c| get(c, 1)).unwrap_err(),
        StoreError::Internal(_)
    ));
}

#[test]
fn rollback_restores_endpoint_ids_states_policies_and_route_retries() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com"); // v1
    let second = commit(&mut db, "add", |c| {
        endpoints::add(c, a.subsystem, a.group, &endpoint_fields("b:2"))
    })
    .unwrap()
    .value; // v2
    commit(&mut db, "drain", |c| {
        endpoints::set_state(c, a.subsystem, a.group, second, EndpointState::Draining)
    })
    .unwrap(); // v3
    commit(&mut db, "policy", |c| {
        let mut fields = upstream_fields("backend");
        fields.policy.lb.algorithm = LbAlgorithm::ConsistentHash;
        fields.policy.health.rise = 7;
        upstreams::update(c, a.subsystem, a.group, &fields)
    })
    .unwrap(); // v4
    commit(&mut db, "retry", |c| {
        let route = db_route(c, a.subsystem);
        let mut fields = route_fields("/", a.group);
        fields.retry = RetryPolicy {
            max_attempts: 4,
            idempotent_on_5xx: false,
            on_timeout: true,
        };
        routes::update(c, a.subsystem, route, &fields)
    })
    .unwrap(); // v5
    commit(&mut db, "remove", |c| {
        endpoints::delete(c, a.subsystem, a.group, second)
    })
    .unwrap(); // v6

    let stored: Vec<Config> = (1..=6)
        .map(|v| db.read(|c| get(c, v)).unwrap().config)
        .collect();
    for version in (1..=6u64).rev() {
        restore_as_new_revision(&mut db, version);
        assert_eq!(
            db.load_all().unwrap(),
            stored[(version - 1) as usize],
            "after rolling back to v{version}"
        );
    }
}

fn db_route(
    conn: &rusqlite::Connection,
    subsystem: fast_proxy_model::SubsystemId,
) -> fast_proxy_model::RouteId {
    subsystems::get(conn, subsystem).unwrap().routes[0].id
}

mod tls_revisions {
    use fast_proxy_model::{
        CertificateId, CertificateMetadata, CertificateName, CertificateSource, HttpsPolicy,
        RedirectMode, UpstreamTls,
    };

    use super::{db, restore_as_new_revision};
    use crate::repository::ca_bundles::{self, NewCaBundle};
    use crate::repository::certificates::{self, NewCertificate};
    use crate::repository::{subsystems, upstreams};
    use crate::revision::{get, list};
    use crate::test_support::{commit, seed, subsystem_fields, upstream_fields};
    use crate::{CertificateSource as _, Database, StoreError, SubsystemFields};

    const PRIVATE_MARKER: &str = "PRIVATE-KEY-MARKER";

    fn upload(db: &mut Database, name: &str) -> CertificateId {
        commit(db, &format!("certificate uploaded: {name}"), |conn| {
            let id = certificates::next_id(conn)?;
            certificates::insert(
                conn,
                id,
                &NewCertificate {
                    name: CertificateName::parse(name).unwrap(),
                    source: CertificateSource::Uploaded,
                    chain_pem: "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"
                        .into(),
                    key_enc: PRIVATE_MARKER.as_bytes().to_vec(),
                    key_nonce: vec![1; 12],
                    metadata: CertificateMetadata {
                        names: vec!["a.example.com".into()],
                        subject: "CN=a".into(),
                        issuer: "CN=a".into(),
                        not_before_ms: 0,
                        not_after_ms: 1,
                        fingerprint: "00".into(),
                    },
                },
            )?;
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

    fn attach(db: &mut Database, site: &crate::test_support::Fixture, id: Option<CertificateId>) {
        commit(db, "https edited", |conn| {
            subsystems::update(
                conn,
                site.subsystem,
                &SubsystemFields {
                    https: HttpsPolicy {
                        enabled: true,
                        certificate_id: id,
                        redirect: RedirectMode::Permanent308,
                        ..HttpsPolicy::default()
                    },
                    ..subsystem_fields("shop")
                },
            )
        })
        .unwrap();
    }

    #[test]
    fn rollback_restores_https_and_tls_settings_at_every_version() {
        let mut db = db();
        let certificate = upload(&mut db, "shop");
        let site = seed(&mut db, "shop", "shop.example.com");
        let mut stored = Vec::new();
        stored.push((db.current_version().unwrap(), db.load_all().unwrap()));

        attach(&mut db, &site, Some(certificate));
        stored.push((db.current_version().unwrap(), db.load_all().unwrap()));

        let mut fields = upstream_fields("backend");
        fields.policy.tls = UpstreamTls {
            enabled: true,
            verify: false,
            server_name: "backend.internal".into(),
            ca: String::new(),
        };
        commit(&mut db, "tls", |conn| {
            upstreams::update(conn, site.subsystem, site.group, &fields)
        })
        .unwrap();
        stored.push((db.current_version().unwrap(), db.load_all().unwrap()));

        for (version, expected) in stored.iter().rev() {
            restore_as_new_revision(&mut db, *version);
            assert_eq!(&db.load_all().unwrap(), expected, "v{version}");
        }
    }

    #[test]
    fn rollback_to_a_deleted_certificate_is_refused_naming_it() {
        let mut db = db();
        let certificate = upload(&mut db, "shop-cert");
        let site = seed(&mut db, "shop", "shop.example.com");
        attach(&mut db, &site, Some(certificate));
        let with_certificate = db.current_version().unwrap();
        commit(&mut db, "detach", |conn| {
            subsystems::update(conn, site.subsystem, &subsystem_fields("shop"))
        })
        .unwrap();
        commit(&mut db, "delete", |conn| {
            certificates::delete(conn, certificate)
        })
        .unwrap();
        let before = db.load_all().unwrap();

        let err = commit(&mut db, "rollback", |conn| {
            super::super::restore(conn, with_certificate)
        })
        .unwrap_err();
        assert!(
            matches!(&err, StoreError::Conflict(m) if m.contains("shop-cert")),
            "{err}"
        );
        assert_eq!(db.load_all().unwrap(), before, "nothing changed");
    }

    #[test]
    fn a_reused_certificate_id_with_another_name_does_not_satisfy_a_rollback() {
        let mut db = db();
        let original = upload(&mut db, "first");
        let site = seed(&mut db, "shop", "shop.example.com");
        attach(&mut db, &site, Some(original));
        let with_certificate = db.current_version().unwrap();
        commit(&mut db, "detach", |conn| {
            subsystems::update(conn, site.subsystem, &subsystem_fields("shop"))
        })
        .unwrap();
        commit(&mut db, "delete", |conn| {
            certificates::delete(conn, original)
        })
        .unwrap();
        let reused = upload(&mut db, "second");
        assert_eq!(reused, original, "SQLite reuses the highest id");

        let err = commit(&mut db, "rollback", |conn| {
            super::super::restore(conn, with_certificate)
        })
        .unwrap_err();
        assert!(
            matches!(&err, StoreError::Conflict(m) if m.contains("first")),
            "{err}"
        );
    }

    #[test]
    fn rollback_to_a_deleted_ca_bundle_is_refused_naming_it() {
        let mut db = db();
        let bundle_id = commit(&mut db, "bundle", |conn| {
            ca_bundles::insert(conn, &bundle("corp"))
        })
        .unwrap()
        .value;
        let site = seed(&mut db, "shop", "shop.example.com");
        let mut fields = upstream_fields("backend");
        fields.policy.tls.enabled = true;
        fields.policy.tls.ca = "corp".into();
        commit(&mut db, "tls", |conn| {
            upstreams::update(conn, site.subsystem, site.group, &fields)
        })
        .unwrap();
        let with_bundle = db.current_version().unwrap();
        commit(&mut db, "plain", |conn| {
            upstreams::update(
                conn,
                site.subsystem,
                site.group,
                &upstream_fields("backend"),
            )
        })
        .unwrap();
        commit(&mut db, "delete", |conn| {
            ca_bundles::delete(conn, bundle_id)
        })
        .unwrap();

        let err = commit(&mut db, "rollback", |conn| {
            super::super::restore(conn, with_bundle)
        })
        .unwrap_err();
        assert!(
            matches!(&err, StoreError::Conflict(m) if m.contains("corp")),
            "{err}"
        );
    }

    #[test]
    fn certificate_changes_are_summary_only_revisions_and_rollback_leaves_material_alone() {
        let mut db = db();
        let site = seed(&mut db, "shop", "shop.example.com");
        let before = db.current_version().unwrap();
        let certificate = upload(&mut db, "shop");
        let uploaded = db.current_version().unwrap();
        assert_eq!(uploaded, before + 1);
        assert_eq!(
            db.read(|c| get(c, uploaded)).unwrap().config,
            db.read(|c| get(c, before)).unwrap().config,
            "a material change leaves the configuration as it was"
        );

        let material = db.read(certificates::list).unwrap();
        restore_as_new_revision(&mut db, before);
        let after = db.read(certificates::list).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].certificate.key_enc,
            material[0].certificate.key_enc
        );
        assert_eq!(after[0].updated_at_ms, material[0].updated_at_ms);

        commit(&mut db, "certificate deleted: shop", |conn| {
            certificates::delete(conn, certificate)
        })
        .unwrap();
        let summaries: Vec<String> = db
            .read(|c| list(c, 10, None))
            .unwrap()
            .into_iter()
            .map(|r| r.summary)
            .collect();
        assert!(summaries.iter().any(|s| s == "certificate uploaded: shop"));
        assert!(summaries.iter().any(|s| s == "certificate deleted: shop"));
        assert_eq!(
            db.read(|c| subsystems::get(c, site.subsystem))
                .unwrap()
                .hosts
                .len(),
            1
        );
    }

    #[test]
    fn no_revision_row_contains_pem_or_key_bytes() {
        let mut db = db();
        let certificate = upload(&mut db, "shop");
        let site = seed(&mut db, "shop", "shop.example.com");
        attach(&mut db, &site, Some(certificate));
        let rows: Vec<String> = db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT summary || config FROM revision")?;
                let rows = statement.query_map([], |row| row.get(0))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap();
        assert!(!rows.is_empty());
        for row in rows {
            assert!(!row.contains("BEGIN"), "no PEM in a revision");
            assert!(!row.contains(PRIVATE_MARKER), "no key bytes in a revision");
        }
        let last = db.read(|c| get(c, db.current_version().unwrap())).unwrap();
        assert!(last.config.subsystems[0].https.certificate_id.is_some());
        let json_has_name = db
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT config FROM revision ORDER BY version DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .unwrap()
            .contains("\"certificate_name\":\"shop\"");
        assert!(
            json_has_name,
            "the certificate's name is recorded for rollback messages"
        );
        let _ = db.version();
    }
}

mod outside_config {
    use fast_proxy_model::{
        BypassRules, MaintenanceSettings, MaintenanceTarget, Role, SecretHash, TokenName,
    };

    use super::*;
    use crate::repository::maintenance;
    use crate::repository::tokens::{self, NewToken};

    /// A change to data outside the configuration: the configuration copy in
    /// the revision stays as it was.
    fn write_outside<T>(
        db: &mut Database,
        actor: &str,
        mutate: impl FnOnce(&rusqlite::Connection) -> Result<Mutation<T>, StoreError>,
    ) -> Result<crate::Written<T>, StoreError> {
        db.write_as(actor, mutate, |_| Ok(()))
    }

    fn maintenance_settings() -> MaintenanceSettings {
        MaintenanceSettings {
            message: "down".into(),
            body: None,
            retry_after_secs: None,
            bypass: BypassRules::default(),
            expires_at_ms: None,
            set_by: "ops".into(),
            set_at_ms: 1,
        }
    }

    fn new_token(name: &str) -> NewToken {
        NewToken {
            id: None,
            name: TokenName::parse(name).unwrap(),
            role: Role::Admin,
            secret_hash: SecretHash::new([0xAB; 32]),
            created_by: "cli".into(),
            expires_at_ms: None,
        }
    }

    #[test]
    fn a_write_records_the_actor_and_plain_writes_record_system() {
        let mut db = db();
        db.write_as(
            "ann",
            |c| {
                subsystems::create(c, &subsystem_fields("shop"))?;
                Ok(Mutation {
                    value: (),
                    summary: "created".into(),
                })
            },
            |_| Ok(()),
        )
        .unwrap();
        commit(&mut db, "plain", |c| {
            subsystems::create(c, &subsystem_fields("blog"))
        })
        .unwrap();
        let listed = db.read(|c| list(c, 10, None)).unwrap();
        assert_eq!(listed[0].actor, "system");
        assert_eq!(listed[1].actor, "ann");
        assert_eq!(db.read(|c| get(c, 1)).unwrap().actor, "ann");
    }

    #[test]
    fn a_change_outside_config_moves_the_version_without_changing_the_config() {
        let mut db = db();
        seed(&mut db, "shop", "s.example.com");
        let before = db.load_all().unwrap();
        let written = write_outside(&mut db, "ann", |c| {
            tokens::create(c, &new_token("ci"), 5)?;
            Ok(Mutation {
                value: (),
                summary: "token created: ci".into(),
            })
        })
        .unwrap();
        assert_eq!(written.version, 2);
        assert_eq!(written.config, before);
        let revision = db.read(|c| get(c, 2)).unwrap();
        assert_eq!(revision.actor, "ann");
        assert_eq!(revision.summary, "token created: ci");
        assert_eq!(revision.config, before);
    }

    #[test]
    fn a_failed_change_outside_config_leaves_nothing_behind() {
        let mut db = db();
        let result: Result<_, _> = write_outside(&mut db, "ann", |c| {
            tokens::create(c, &new_token("ci"), 5)?;
            tokens::create(c, &new_token("ci"), 5)?;
            Ok(Mutation {
                value: (),
                summary: "never".into(),
            })
        });
        assert!(matches!(result.unwrap_err(), StoreError::Conflict(_)));
        assert!(db.read(tokens::list).unwrap().is_empty());
        assert_eq!(db.current_version().unwrap(), 0);
    }

    #[test]
    fn a_rollback_leaves_tokens_and_maintenance_untouched() {
        let mut db = db();
        let fixture = seed(&mut db, "shop", "s.example.com");
        write_outside(&mut db, "ann", |c| {
            tokens::create(c, &new_token("ci"), 5)?;
            maintenance::set(c, MaintenanceTarget::Global, &maintenance_settings())?;
            maintenance::set(
                c,
                MaintenanceTarget::Subsystem(fixture.subsystem),
                &maintenance_settings(),
            )?;
            Ok(Mutation {
                value: (),
                summary: "ops".into(),
            })
        })
        .unwrap();
        let tokens_before = db.read(tokens::list).unwrap();
        let maintenance_before = db.read(maintenance::list).unwrap();

        restore_as_new_revision(&mut db, 1);

        assert_eq!(db.read(tokens::list).unwrap(), tokens_before);
        assert_eq!(db.read(maintenance::list).unwrap(), maintenance_before);
    }

    #[test]
    fn a_rollback_drops_maintenance_only_for_targets_that_are_gone() {
        let mut db = db();
        let early = seed(&mut db, "shop", "s.example.com");
        let later = seed(&mut db, "blog", "b.example.com");
        write_outside(&mut db, "ann", |c| {
            maintenance::set(
                c,
                MaintenanceTarget::Subsystem(early.subsystem),
                &maintenance_settings(),
            )?;
            maintenance::set(
                c,
                MaintenanceTarget::Subsystem(later.subsystem),
                &maintenance_settings(),
            )?;
            Ok(Mutation {
                value: (),
                summary: "ops".into(),
            })
        })
        .unwrap();

        // Version 1 had only "shop": "blog" disappears and takes its scope along.
        restore_as_new_revision(&mut db, 1);

        let rows = db.read(maintenance::list).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].target,
            MaintenanceTarget::Subsystem(early.subsystem)
        );
    }

    fn sampling_override(baseline: u32) -> fast_proxy_model::SamplingOverride {
        fast_proxy_model::SamplingOverride {
            enabled: Some(true),
            capture_bodies: Some(true),
            baseline_per_mille: Some(baseline),
            slow_threshold_ms: None,
            slow_p99_factor_x10: None,
            set_by: "ops".into(),
            set_at_ms: 1,
        }
    }

    #[test]
    fn a_rollback_leaves_the_sampling_settings_overrides_and_samples_untouched() {
        use crate::repository::{samples, sampling};
        use fast_proxy_model::{SamplingSettings, SamplingTarget, Trigger};

        let mut db = db();
        let fixture = seed(&mut db, "shop", "s.example.com");
        let settings = SamplingSettings {
            retention_days: 3,
            redact_body_fields: vec!["iban".into()],
            updated_by: "ann".into(),
            ..SamplingSettings::default()
        };
        write_outside(&mut db, "ann", |c| {
            sampling::set_settings(c, &settings)?;
            sampling::set_override(
                c,
                SamplingTarget::Subsystem(fixture.subsystem),
                &sampling_override(10),
            )?;
            Ok(Mutation {
                value: (),
                summary: "sampling".into(),
            })
        })
        .unwrap();
        db.write_unrecorded(|c| {
            samples::insert_batch(
                c,
                &[samples::NewSample {
                    at_ms: 5,
                    instance: "a".into(),
                    request_id: "r".into(),
                    triggers: [Trigger::Slow].into_iter().collect(),
                    subsystem: "shop".into(),
                    route: "/".into(),
                    client: "c".into(),
                    method: "GET".into(),
                    status: 200,
                    total_ms: 9,
                    detail: "{}".into(),
                }],
            )
        })
        .unwrap();
        let overrides_before = db.read(sampling::list_overrides).unwrap();

        restore_as_new_revision(&mut db, 1);

        assert_eq!(db.read(sampling::get_settings).unwrap(), settings);
        assert_eq!(db.read(sampling::list_overrides).unwrap(), overrides_before);
        assert_eq!(db.read(samples::usage).unwrap().rows, 1);
    }

    #[test]
    fn a_rollback_drops_a_sampling_override_only_for_targets_that_are_gone() {
        use crate::repository::sampling;
        use fast_proxy_model::SamplingTarget;

        let mut db = db();
        let early = seed(&mut db, "shop", "s.example.com");
        let later = seed(&mut db, "blog", "b.example.com");
        write_outside(&mut db, "ann", |c| {
            for subsystem in [early.subsystem, later.subsystem] {
                sampling::set_override(
                    c,
                    SamplingTarget::Subsystem(subsystem),
                    &sampling_override(5),
                )?;
            }
            Ok(Mutation {
                value: (),
                summary: "ops".into(),
            })
        })
        .unwrap();

        restore_as_new_revision(&mut db, 1);

        let rows = db.read(sampling::list_overrides).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, SamplingTarget::Subsystem(early.subsystem));
    }

    #[test]
    fn no_revision_contains_a_secret_hash() {
        let mut db = db();
        write_outside(&mut db, "ann", |c| {
            tokens::create(c, &new_token("ci"), 5)?;
            Ok(Mutation {
                value: (),
                summary: "token created: ci".into(),
            })
        })
        .unwrap();
        let stored: Vec<String> = db
            .read(|c| {
                let mut statement = c.prepare("SELECT summary || config || actor FROM revision")?;
                let rows = statement.query_map([], |row| row.get(0))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap();
        let hash_hex = "ab".repeat(32);
        for row in stored {
            assert!(!row.to_lowercase().contains(&hash_hex));
            assert!(!row.contains("171, 171"), "no byte-array form either");
        }
    }
}
