use fast_proxy_model::{Role, SecretHash, TokenId, TokenName};

use super::tokens::{self, NewToken};
use crate::{Database, StoreError};

const NOW: i64 = 1_000_000;

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

fn token(name: &str, role: Role) -> NewToken {
    NewToken {
        id: None,
        name: TokenName::parse(name).unwrap(),
        role,
        secret_hash: SecretHash::new([name.len() as u8; 32]),
        created_by: "cli".into(),
        expires_at_ms: None,
    }
}

fn create(db: &Database, name: &str, role: Role) -> TokenId {
    db.read(|c| tokens::create(c, &token(name, role), NOW))
        .unwrap()
}

#[test]
fn a_created_token_is_listed_and_found_without_its_hash() {
    let db = db();
    let id = create(&db, "ci", Role::Viewer);
    let listed = db.read(tokens::list).unwrap();
    assert_eq!(listed.len(), 1);
    let found = db.read(|c| tokens::find(c, id)).unwrap();
    assert_eq!(listed[0], found);
    assert_eq!(found.name.as_str(), "ci");
    assert_eq!(found.role, Role::Viewer);
    assert_eq!(found.created_at_ms, NOW);
    assert_eq!(found.created_by, "cli");
    assert_eq!(found.revoked_at_ms, None);
}

#[test]
fn a_duplicate_name_is_a_conflict_naming_it() {
    let db = db();
    create(&db, "ci", Role::Admin);
    let err = db
        .read(|c| tokens::create(c, &token("ci", Role::Viewer), NOW))
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Conflict(m) if m.contains("\"ci\"")),
        "{err}"
    );
}

#[test]
fn finding_a_missing_token_is_not_found() {
    let db = db();
    let err = db.read(|c| tokens::find(c, TokenId(9))).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn credentials_carry_the_hash_and_the_revoked_flag() {
    let db = db();
    create(&db, "a", Role::Admin);
    let second = create(&db, "bb", Role::Viewer);
    db.read(|c| tokens::revoke(c, second, NOW)).unwrap();
    let credentials = db.read(tokens::credentials).unwrap();
    assert_eq!(credentials.len(), 2);
    assert_eq!(credentials[0].secret_hash, SecretHash::new([1; 32]));
    assert!(!credentials[0].revoked);
    assert!(credentials[1].revoked);
}

#[test]
fn the_last_usable_admin_cannot_be_revoked() {
    let db = db();
    let only = create(&db, "root", Role::Admin);
    let err = db.read(|c| tokens::revoke(c, only, NOW)).unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
    assert_eq!(db.read(|c| tokens::usable_admin_count(c, NOW)).unwrap(), 1);
}

#[test]
fn an_admin_can_be_revoked_while_another_is_usable() {
    let db = db();
    let first = create(&db, "one", Role::Admin);
    create(&db, "two", Role::Admin);
    db.read(|c| tokens::revoke(c, first, NOW + 5)).unwrap();
    let revoked = db.read(|c| tokens::find(c, first)).unwrap();
    assert_eq!(revoked.revoked_at_ms, Some(NOW + 5));
    assert_eq!(db.read(|c| tokens::usable_admin_count(c, NOW)).unwrap(), 1);
}

#[test]
fn viewers_never_count_toward_the_last_admin_rule() {
    let db = db();
    create(&db, "root", Role::Admin);
    let viewer = create(&db, "eyes", Role::Viewer);
    db.read(|c| tokens::revoke(c, viewer, NOW)).unwrap();
}

#[test]
fn an_expired_admin_does_not_count_as_usable() {
    let db = db();
    let mut expiring = token("old", Role::Admin);
    expiring.expires_at_ms = Some(NOW + 10);
    db.read(|c| tokens::create(c, &expiring, NOW)).unwrap();
    let fresh = create(&db, "new", Role::Admin);
    assert_eq!(db.read(|c| tokens::usable_admin_count(c, NOW)).unwrap(), 2);
    assert_eq!(
        db.read(|c| tokens::usable_admin_count(c, NOW + 10))
            .unwrap(),
        1
    );
    let err = db.read(|c| tokens::revoke(c, fresh, NOW + 10)).unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
}

#[test]
fn revoking_twice_keeps_the_first_time() {
    let db = db();
    create(&db, "root", Role::Admin);
    let other = create(&db, "other", Role::Admin);
    db.read(|c| tokens::revoke(c, other, NOW + 1)).unwrap();
    db.read(|c| tokens::revoke(c, other, NOW + 99)).unwrap();
    assert_eq!(
        db.read(|c| tokens::find(c, other)).unwrap().revoked_at_ms,
        Some(NOW + 1)
    );
}

#[test]
fn marking_a_token_that_does_not_exist_is_not_found() {
    let db = db();
    let err = db
        .read(|c| tokens::mark_used(c, TokenId(9), NOW))
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn last_used_is_refreshed_at_most_once_a_minute() {
    let db = db();
    let id = create(&db, "ci", Role::Admin);
    let used = |db: &Database| db.read(|c| tokens::find(c, id)).unwrap().last_used_at_ms;
    assert_eq!(used(&db), None);
    assert!(
        db.read(|c| tokens::mark_used(c, id, NOW)).unwrap(),
        "the first use"
    );
    assert_eq!(used(&db), Some(NOW));
    assert!(!db.read(|c| tokens::mark_used(c, id, NOW + 59_999)).unwrap());
    assert_eq!(used(&db), Some(NOW), "too soon");
    assert!(!db.read(|c| tokens::mark_used(c, id, NOW + 60_000)).unwrap());
    assert_eq!(used(&db), Some(NOW + 60_000));
}
