use super::audit::{self, AuditOutcome, AuditQuery, NewAuditEntry, MAX_DETAIL_BYTES};
use crate::{Database, StoreError};

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

fn entry(at_ms: i64, actor: &str, action: &str, outcome: AuditOutcome) -> NewAuditEntry {
    NewAuditEntry {
        at_ms,
        instance: "i-1".into(),
        actor: actor.into(),
        role: "admin".into(),
        source: "127.0.0.1".into(),
        request_id: "req".into(),
        action: action.into(),
        target: "shop".into(),
        outcome,
        detail: "{}".into(),
        revision: Some(4),
    }
}

fn append_all(db: &Database, entries: &[NewAuditEntry]) {
    for e in entries {
        db.read(|c| audit::append(c, e)).unwrap();
    }
}

fn list(db: &Database, query: &AuditQuery, before: Option<i64>, limit: u32) -> Vec<i64> {
    db.read(|c| audit::list(c, query, before, limit))
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect()
}

#[test]
fn entries_round_trip_newest_first() {
    let db = db();
    let first = entry(1, "ann", "subsystem.create", AuditOutcome::Ok);
    let mut second = entry(2, "bob", "route.delete", AuditOutcome::Denied);
    second.revision = None;
    append_all(&db, &[first.clone(), second.clone()]);
    let listed = db
        .read(|c| audit::list(c, &AuditQuery::default(), None, 10))
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].entry, second);
    assert_eq!(listed[1].entry, first);
}

#[test]
fn paging_by_id_is_stable_while_new_entries_arrive() {
    let db = db();
    let entries: Vec<_> = (0..5)
        .map(|n| entry(n, "ann", "x.y", AuditOutcome::Ok))
        .collect();
    append_all(&db, &entries);
    let page_one = list(&db, &AuditQuery::default(), None, 2);
    assert_eq!(page_one, vec![5, 4]);
    append_all(&db, &[entry(9, "ann", "x.y", AuditOutcome::Ok)]);
    let page_two = list(&db, &AuditQuery::default(), Some(4), 2);
    assert_eq!(page_two, vec![3, 2]);
    assert_eq!(list(&db, &AuditQuery::default(), Some(2), 10), vec![1]);
}

#[test]
fn each_filter_narrows_the_result() {
    let db = db();
    append_all(
        &db,
        &[
            entry(10, "ann", "route.create", AuditOutcome::Ok),
            entry(20, "bob", "route.create", AuditOutcome::Failed),
            entry(30, "ann", "route.delete", AuditOutcome::Denied),
        ],
    );
    let by = |query: AuditQuery| list(&db, &query, None, 10);
    assert_eq!(
        by(AuditQuery {
            actor: Some("ann".into()),
            ..Default::default()
        }),
        vec![3, 1]
    );
    assert_eq!(
        by(AuditQuery {
            action: Some("route.create".into()),
            ..Default::default()
        }),
        vec![2, 1]
    );
    assert_eq!(
        by(AuditQuery {
            outcome: Some(AuditOutcome::Denied),
            ..Default::default()
        }),
        vec![3]
    );
    assert_eq!(
        by(AuditQuery {
            since_ms: Some(20),
            ..Default::default()
        }),
        vec![3, 2]
    );
    assert_eq!(
        by(AuditQuery {
            actor: Some("ann".into()),
            since_ms: Some(20),
            ..Default::default()
        }),
        vec![3]
    );
}

#[test]
fn export_is_oldest_first_and_resumable() {
    let db = db();
    let entries: Vec<_> = (0..5)
        .map(|n| entry(n * 10, "ann", "x.y", AuditOutcome::Ok))
        .collect();
    append_all(&db, &entries);
    let ids = |after, since| -> Vec<i64> {
        db.read(|c| audit::export(c, since, after, 2))
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect()
    };
    assert_eq!(ids(0, None), vec![1, 2]);
    assert_eq!(ids(2, None), vec![3, 4]);
    assert_eq!(ids(4, None), vec![5]);
    assert_eq!(ids(0, Some(20)), vec![3, 4]);
}

#[test]
fn an_oversized_detail_is_refused_and_the_limit_itself_accepted() {
    let db = db();
    let mut e = entry(1, "ann", "x.y", AuditOutcome::Ok);
    e.detail = "a".repeat(MAX_DETAIL_BYTES);
    db.read(|c| audit::append(c, &e)).unwrap();
    e.detail.push('a');
    let err = db.read(|c| audit::append(c, &e)).unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "detail"));
}

#[test]
fn pruning_by_age_removes_only_older_entries_in_batches() {
    let db = db();
    let entries: Vec<_> = (0..6)
        .map(|n| entry(n * 10, "ann", "x.y", AuditOutcome::Ok))
        .collect();
    append_all(&db, &entries);
    let prune = |batch| db.read(|c| audit::prune(c, 30, 1000, batch)).unwrap();
    assert_eq!(prune(2), 2, "one batch at a time");
    assert_eq!(prune(2), 1);
    assert_eq!(prune(2), 0);
    assert_eq!(list(&db, &AuditQuery::default(), None, 10), vec![6, 5, 4]);
}

#[test]
fn pruning_by_count_keeps_the_newest_rows() {
    let db = db();
    let entries: Vec<_> = (0..6)
        .map(|n| entry(1_000 + n, "ann", "x.y", AuditOutcome::Ok))
        .collect();
    append_all(&db, &entries);
    let mut removed = 0;
    loop {
        let batch = db.read(|c| audit::prune(c, 0, 3, 2)).unwrap();
        if batch == 0 {
            break;
        }
        removed += batch;
    }
    assert_eq!(removed, 3);
    assert_eq!(list(&db, &AuditQuery::default(), None, 10), vec![6, 5, 4]);
}

#[test]
fn pruning_an_empty_or_small_log_removes_nothing() {
    let db = db();
    assert_eq!(db.read(|c| audit::prune(c, i64::MAX, 3, 10)).unwrap(), 0);
    append_all(&db, &[entry(5, "ann", "x.y", AuditOutcome::Ok)]);
    assert_eq!(db.read(|c| audit::prune(c, 0, 3, 10)).unwrap(), 0);
}
