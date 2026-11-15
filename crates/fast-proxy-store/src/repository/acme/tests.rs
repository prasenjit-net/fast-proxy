use std::collections::HashSet;

use fast_proxy_model::{
    AcmeChallenge, AcmeState, AcmeStatus, AcmeToken, CertificateId, CertificateMetadata,
    CertificateName, CertificateSource, SkipReason, SkippedName, SubsystemId,
};

use super::{account, challenges, lease, status};
use crate::repository::{certificates, subsystems};
use crate::test_support::{commit, seed};
use crate::{Database, StoreError};

fn challenge(token: &str, subsystem: SubsystemId, expires_at_ms: i64) -> AcmeChallenge {
    AcmeChallenge::new(
        AcmeToken::parse(token).unwrap(),
        subsystem,
        vec!["shop.example.com".into()],
        format!("{token}.thumbprint"),
        expires_at_ms,
    )
    .unwrap()
}

fn unrecorded<T>(
    db: &mut Database,
    op: impl FnOnce(&rusqlite::Connection) -> Result<T, StoreError>,
) -> T {
    db.write_unrecorded(op).unwrap()
}

#[test]
fn an_account_is_stored_per_directory_and_replaced_in_place() {
    let mut db = Database::open_in_memory().unwrap();
    let staging = account::AcmeAccountRow {
        directory: "https://staging.example/dir".into(),
        email: "ops@example.com".into(),
        credentials_enc: vec![1, 2, 3],
        credentials_nonce: vec![9; 12],
    };
    unrecorded(&mut db, |conn| account::put(conn, &staging));
    assert_eq!(
        db.read(|conn| account::get(conn, &staging.directory))
            .unwrap(),
        Some(staging.clone())
    );
    assert_eq!(
        db.read(|conn| account::get(conn, "https://other/dir"))
            .unwrap(),
        None
    );

    let replaced = account::AcmeAccountRow {
        email: String::new(),
        credentials_enc: vec![4],
        ..staging.clone()
    };
    unrecorded(&mut db, |conn| account::put(conn, &replaced));
    assert_eq!(
        db.read(|conn| account::get(conn, &staging.directory))
            .unwrap(),
        Some(replaced)
    );
}

#[test]
fn account_debug_never_shows_the_sealed_credentials() {
    let row = account::AcmeAccountRow {
        directory: "d".into(),
        email: "e".into(),
        credentials_enc: vec![0xAB; 4],
        credentials_nonce: vec![0xCD; 4],
    };
    let shown = format!("{row:?}");
    assert!(!shown.contains("171") && !shown.contains("205"), "{shown}");
    assert!(!shown.contains("credentials"), "{shown}");
}

#[test]
fn challenges_round_trip_and_are_cleared_by_subsystem_and_by_expiry() {
    let mut db = Database::open_in_memory().unwrap();
    let shop = seed(&mut db, "shop", "shop.example.com").subsystem;
    let blog = seed(&mut db, "blog", "blog.example.com").subsystem;
    let all = vec![
        challenge("a", shop, 100),
        challenge("b", shop, 300),
        challenge("c", blog, 200),
    ];
    unrecorded(&mut db, |conn| challenges::put(conn, &all));
    assert_eq!(db.read(challenges::list).unwrap(), all);

    assert_eq!(
        unrecorded(&mut db, |conn| challenges::clear_expired(conn, 200)),
        2
    );
    assert_eq!(db.read(challenges::list).unwrap(), vec![all[1].clone()]);
    assert_eq!(
        unrecorded(&mut db, |conn| challenges::clear_subsystem(conn, shop)),
        1
    );
    assert!(db.read(challenges::list).unwrap().is_empty());
}

#[test]
fn a_challenge_for_a_missing_subsystem_is_not_found_and_goes_with_its_subsystem() {
    let mut db = Database::open_in_memory().unwrap();
    let err = db
        .write_unrecorded(|conn| challenges::put(conn, &[challenge("a", SubsystemId(77), 1)]))
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");

    let shop = seed(&mut db, "shop", "shop.example.com").subsystem;
    unrecorded(&mut db, |conn| {
        challenges::put(conn, &[challenge("a", shop, 1)])
    });
    commit(&mut db, "delete", |conn| subsystems::delete(conn, shop)).unwrap();
    assert!(db.read(challenges::list).unwrap().is_empty());
}

#[test]
fn the_lease_is_held_by_one_holder_until_it_expires() {
    let mut db = Database::open_in_memory().unwrap();
    assert!(unrecorded(&mut db, |conn| lease::try_take(
        conn, "a", 0, 100
    )));
    // Another instance cannot take a live lease; the holder can renew it.
    assert!(!unrecorded(&mut db, |conn| lease::try_take(
        conn, "b", 50, 150
    )));
    assert!(unrecorded(&mut db, |conn| lease::try_take(
        conn, "a", 50, 150
    )));
    assert_eq!(
        db.read(lease::get).unwrap().unwrap(),
        fast_proxy_model::AcmeLease {
            holder: "a".into(),
            expires_at_ms: 150
        }
    );
    // Once it expires anyone can take it, and the old holder is out.
    assert!(unrecorded(&mut db, |conn| lease::try_take(
        conn, "b", 150, 250
    )));
    assert!(!unrecorded(&mut db, |conn| lease::try_take(
        conn, "a", 160, 260
    )));
    // Only the holder can release it; then it is free at once.
    assert!(!unrecorded(&mut db, |conn| lease::release(conn, "a")));
    assert!(unrecorded(&mut db, |conn| lease::release(conn, "b")));
    assert_eq!(db.read(lease::get).unwrap(), None);
    assert!(unrecorded(&mut db, |conn| lease::try_take(
        conn, "a", 170, 270
    )));
}

#[test]
fn two_connections_racing_for_the_lease_never_both_win() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lease.db");
    drop(Database::open(&path).unwrap());
    let handles: Vec<_> = ["a", "b", "c", "d"]
        .into_iter()
        .map(|holder| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut db = Database::open(&path).unwrap();
                (0..50)
                    .filter(|_| {
                        db.write_unrecorded(|conn| lease::try_take(conn, holder, 0, i64::MAX))
                            .unwrap()
                    })
                    .map(|_| holder)
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let winners: HashSet<&str> = handles
        .into_iter()
        .flat_map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(winners.len(), 1, "{winners:?}");
}

#[test]
fn lease_writes_create_no_revision() {
    let mut db = Database::open_in_memory().unwrap();
    let before = db.current_version().unwrap();
    unrecorded(&mut db, |conn| lease::try_take(conn, "a", 0, 100));
    assert_eq!(db.current_version().unwrap(), before);
}

#[test]
fn a_status_round_trips_and_is_replaced_and_deleted() {
    let mut db = Database::open_in_memory().unwrap();
    let shop = seed(&mut db, "shop", "shop.example.com").subsystem;
    assert_eq!(db.read(|conn| status::get(conn, shop)).unwrap(), None);

    let mut stored = AcmeStatus::new(shop);
    stored.state = AcmeState::Failed;
    stored.eligible_names = vec!["shop.example.com".into()];
    stored.skipped_names = vec![SkippedName {
        name: "*.shop.example.com".into(),
        reason: SkipReason::WildcardNeedsDns01,
    }];
    stored.next_attempt_at_ms = Some(5_000);
    stored.failures = 2;
    stored.window_failures = 1;
    stored.failure_window_start_ms = Some(1_000);
    stored.last_error = "rateLimited: too many".into();
    stored.last_attempt_at_ms = Some(4_000);
    stored.instance = "i-1".into();
    unrecorded(&mut db, |conn| status::put(conn, &stored));
    assert_eq!(
        db.read(|conn| status::get(conn, shop)).unwrap(),
        Some(stored.clone())
    );

    let mut valid = AcmeStatus::new(shop);
    valid.state = AcmeState::Valid;
    valid.last_success_at_ms = Some(6_000);
    unrecorded(&mut db, |conn| status::put(conn, &valid));
    assert_eq!(db.read(status::list).unwrap(), vec![valid]);

    assert!(unrecorded(&mut db, |conn| status::delete(conn, shop)));
    assert!(!unrecorded(&mut db, |conn| status::delete(conn, shop)));
}

#[test]
fn a_status_for_a_missing_subsystem_is_not_found_and_goes_with_its_subsystem() {
    let mut db = Database::open_in_memory().unwrap();
    let err = db
        .write_unrecorded(|conn| status::put(conn, &AcmeStatus::new(SubsystemId(9))))
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)), "{err:?}");

    let shop = seed(&mut db, "shop", "shop.example.com").subsystem;
    unrecorded(&mut db, |conn| status::put(conn, &AcmeStatus::new(shop)));
    commit(&mut db, "delete", |conn| subsystems::delete(conn, shop)).unwrap();
    assert!(db.read(status::list).unwrap().is_empty());
}

#[test]
fn a_status_refuses_an_unknown_certificate_and_forgets_a_deleted_one() {
    let mut db = Database::open_in_memory().unwrap();
    let shop = seed(&mut db, "shop", "shop.example.com").subsystem;
    let mut with_certificate = AcmeStatus::new(shop);
    with_certificate.certificate_id = Some(CertificateId(12345));
    let err = db
        .write_unrecorded(|conn| status::put(conn, &with_certificate))
        .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)), "{err:?}");

    let id = commit(&mut db, "certificate", |conn| {
        let id = certificates::next_id(conn)?;
        certificates::insert(conn, id, &acme_certificate())?;
        Ok(id)
    })
    .unwrap()
    .value;
    with_certificate.certificate_id = Some(id);
    unrecorded(&mut db, |conn| status::put(conn, &with_certificate));
    commit(&mut db, "delete", |conn| certificates::delete(conn, id)).unwrap();
    let stored = db.read(|conn| status::get(conn, shop)).unwrap().unwrap();
    assert_eq!(stored.certificate_id, None);
}

fn acme_certificate() -> certificates::NewCertificate {
    certificates::NewCertificate {
        name: CertificateName::parse("acme-shop").unwrap(),
        source: CertificateSource::Acme,
        chain_pem: "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".into(),
        key_enc: vec![1; 16],
        key_nonce: vec![2; 12],
        metadata: CertificateMetadata {
            names: vec!["shop.example.com".into()],
            subject: "CN=shop".into(),
            issuer: "CN=ca".into(),
            not_before_ms: 0,
            not_after_ms: 1,
            fingerprint: "ab".repeat(32),
        },
    }
}
