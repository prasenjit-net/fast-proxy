use fast_proxy_model::{Trigger, TriggerSet};

use super::samples::{self, Excess, NewSample, SampleFilter, MAX_PAGE};
use crate::{Database, StoreError};

fn triggers(list: &[Trigger]) -> TriggerSet {
    list.iter().copied().collect()
}

fn sample(n: i64) -> NewSample {
    NewSample {
        at_ms: 1_000 + n,
        instance: "a".into(),
        request_id: format!("req-{n}"),
        triggers: triggers(&[Trigger::UpstreamFailure]),
        subsystem: "shop".into(),
        route: "/api".into(),
        client: "203.0.113.9:5000".into(),
        method: "GET".into(),
        status: 502,
        total_ms: 30,
        detail: format!("{{\"n\":{n}}}"),
    }
}

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

fn insert(db: &Database, rows: &[NewSample]) {
    db.read(|c| samples::insert_batch(c, rows)).unwrap();
}

fn list(db: &Database, filter: &SampleFilter) -> Vec<samples::SampleSummary> {
    db.read(|c| samples::list(c, filter)).unwrap()
}

#[test]
fn a_stored_sample_is_listed_newest_first_and_read_back_whole() {
    let db = db();
    let rows: Vec<NewSample> = (1..=3).map(sample).collect();
    insert(&db, &rows);
    let listed = list(&db, &SampleFilter::default());
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].request_id, "req-3");
    assert_eq!(listed[2].request_id, "req-1");
    assert!(listed[0].id > listed[1].id && listed[1].id > listed[2].id);
    let first = &listed[0];
    assert_eq!(first.primary, Trigger::UpstreamFailure);
    assert_eq!(first.status, 502);
    assert_eq!(first.instance, "a");
    let record = db.read(|c| samples::get(c, first.id)).unwrap();
    assert_eq!(record.summary, *first);
    assert_eq!(record.detail, "{\"n\":3}");
    assert!(first.size_bytes >= record.detail.len() as u64);
}

#[test]
fn the_primary_trigger_is_the_first_in_priority_order() {
    let db = db();
    let mut row = sample(1);
    row.triggers = triggers(&[Trigger::Baseline, Trigger::Slow, Trigger::Retried]);
    insert(&db, &[row]);
    let listed = list(&db, &SampleFilter::default());
    assert_eq!(listed[0].primary, Trigger::Retried);
    assert_eq!(
        listed[0].triggers,
        triggers(&[Trigger::Baseline, Trigger::Slow, Trigger::Retried])
    );
}

#[test]
fn a_missing_sample_is_not_found() {
    let db = db();
    let err = db.read(|c| samples::get(c, 42)).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn every_filter_selects_what_it_names() {
    let db = db();
    let mut rows: Vec<NewSample> = (1..=6).map(sample).collect();
    rows[1].triggers = triggers(&[Trigger::Slow]);
    rows[1].status = 200;
    rows[1].total_ms = 5000;
    rows[2].subsystem = "blog".into();
    rows[3].route = "/static".into();
    rows[4].instance = "b".into();
    rows[5].client = "198.51.100.1:1".into();
    rows[5].status = 404;
    rows[0].at_ms = 10;
    insert(&db, &rows);
    let one = |filter: SampleFilter| -> Vec<String> {
        list(&db, &filter)
            .into_iter()
            .map(|r| r.request_id)
            .collect()
    };
    assert_eq!(
        one(SampleFilter {
            triggers: triggers(&[Trigger::Slow]),
            ..Default::default()
        }),
        ["req-2"]
    );
    assert_eq!(
        one(SampleFilter {
            triggers: triggers(&[Trigger::Slow, Trigger::Baseline]),
            ..Default::default()
        }),
        ["req-2"],
        "any of the listed triggers"
    );
    assert_eq!(
        one(SampleFilter {
            status_class: Some(2),
            ..Default::default()
        }),
        ["req-2"]
    );
    assert_eq!(
        one(SampleFilter {
            status_class: Some(4),
            ..Default::default()
        }),
        ["req-6"]
    );
    assert_eq!(
        one(SampleFilter {
            subsystem: Some("blog".into()),
            ..Default::default()
        }),
        ["req-3"]
    );
    assert_eq!(
        one(SampleFilter {
            route: Some("/static".into()),
            ..Default::default()
        }),
        ["req-4"]
    );
    assert_eq!(
        one(SampleFilter {
            instance: Some("b".into()),
            ..Default::default()
        }),
        ["req-5"]
    );
    assert_eq!(
        one(SampleFilter {
            client: Some("198.51.100.1:1".into()),
            ..Default::default()
        }),
        ["req-6"]
    );
    assert_eq!(
        one(SampleFilter {
            min_ms: Some(1000),
            ..Default::default()
        }),
        ["req-2"]
    );
    assert_eq!(
        one(SampleFilter {
            since_ms: Some(1_000),
            ..Default::default()
        })
        .len(),
        5,
        "the one finished at 10 ms is older"
    );
}

#[test]
fn filters_combine_with_and() {
    let db = db();
    let mut rows: Vec<NewSample> = (1..=4).map(sample).collect();
    rows[0].subsystem = "blog".into();
    rows[1].subsystem = "blog".into();
    rows[1].status = 504;
    rows[2].status = 504;
    insert(&db, &rows);
    let found = list(
        &db,
        &SampleFilter {
            subsystem: Some("blog".into()),
            status_class: Some(5),
            min_ms: Some(30),
            ..Default::default()
        },
    );
    assert_eq!(found.len(), 2);
    let found = list(
        &db,
        &SampleFilter {
            subsystem: Some("blog".into()),
            instance: Some("nobody".into()),
            ..Default::default()
        },
    );
    assert!(found.is_empty());
}

#[test]
fn a_cursor_pages_back_without_repeats_while_new_samples_arrive() {
    let db = db();
    insert(&db, &(1..=5).map(sample).collect::<Vec<_>>());
    let first = list(
        &db,
        &SampleFilter {
            limit: Some(2),
            ..Default::default()
        },
    );
    assert_eq!(first.len(), 2);
    // Another instance writes in between.
    insert(&db, &[sample(6), sample(7)]);
    let second = list(
        &db,
        &SampleFilter {
            limit: Some(2),
            before_id: Some(first[1].id),
            ..Default::default()
        },
    );
    let third = list(
        &db,
        &SampleFilter {
            limit: Some(2),
            before_id: Some(second[1].id),
            ..Default::default()
        },
    );
    let seen: Vec<String> = first
        .iter()
        .chain(&second)
        .chain(&third)
        .map(|r| r.request_id.clone())
        .collect();
    assert_eq!(seen, ["req-5", "req-4", "req-3", "req-2", "req-1"]);
}

#[test]
fn the_page_size_is_bounded() {
    let db = db();
    insert(&db, &(1..=3).map(sample).collect::<Vec<_>>());
    assert_eq!(
        list(
            &db,
            &SampleFilter {
                limit: Some(0),
                ..Default::default()
            }
        )
        .len(),
        1
    );
    assert_eq!(
        list(
            &db,
            &SampleFilter {
                limit: Some(MAX_PAGE * 10),
                ..Default::default()
            }
        )
        .len(),
        3
    );
}

#[test]
fn a_sample_without_a_trigger_is_refused_and_the_whole_batch_with_it() {
    let db = db();
    let mut bad = sample(2);
    bad.triggers = TriggerSet::EMPTY;
    let err = db
        .read(|c| samples::insert_batch(c, &[sample(1), bad]))
        .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "triggers"));
    assert!(list(&db, &SampleFilter::default()).is_empty());
}

#[test]
fn a_batch_that_fails_part_way_leaves_nothing_behind() {
    let db = db();
    db.read(|c| {
        c.execute_batch(
            "CREATE TEMP TRIGGER refuse BEFORE INSERT ON sample WHEN NEW.request_id = 'boom'
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )?;
        Ok(())
    })
    .unwrap();
    let mut third = sample(3);
    third.request_id = "boom".into();
    let err = db
        .read(|c| samples::insert_batch(c, &[sample(1), sample(2), third]))
        .unwrap_err();
    assert!(matches!(err, StoreError::Internal(_)));
    assert!(
        list(&db, &SampleFilter::default()).is_empty(),
        "the first two were rolled back"
    );
    insert(&db, &[sample(4)]);
    assert_eq!(
        list(&db, &SampleFilter::default()).len(),
        1,
        "the connection is usable after"
    );
}

#[test]
fn a_batch_inside_a_failing_transaction_is_rolled_back_with_it() {
    let mut db = db();
    let result: Result<(), StoreError> = db.write_unrecorded(|c| {
        samples::insert_batch(c, &[sample(1), sample(2)])?;
        Err(StoreError::Internal("later step failed".into()))
    });
    assert!(result.is_err());
    assert!(list(&db, &SampleFilter::default()).is_empty());
    db.write_unrecorded(|c| samples::insert_batch(c, &[sample(3)]))
        .unwrap();
    assert_eq!(list(&db, &SampleFilter::default()).len(), 1);
}

#[test]
fn writing_samples_records_no_revision() {
    let mut db = db();
    let before = db.current_version().unwrap();
    db.write_unrecorded(|c| samples::insert_batch(c, &(1..=50).map(sample).collect::<Vec<_>>()))
        .unwrap();
    assert_eq!(
        db.current_version().unwrap(),
        before,
        "no revision, no generation bump"
    );
    let revisions: i64 = db
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM revision", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(revisions, 0);
}

#[test]
fn usage_counts_rows_bytes_and_the_oldest() {
    let db = db();
    let empty = db.read(samples::usage).unwrap();
    assert_eq!((empty.rows, empty.bytes, empty.oldest_at_ms), (0, 0, None));
    let rows: Vec<NewSample> = (1..=3).map(sample).collect();
    insert(&db, &rows);
    let usage = db.read(samples::usage).unwrap();
    assert_eq!(usage.rows, 3);
    assert_eq!(usage.oldest_at_ms, Some(1_001));
    let listed: u64 = list(&db, &SampleFilter::default())
        .iter()
        .map(|r| r.size_bytes)
        .sum();
    assert_eq!(usage.bytes, listed);
}

struct Limits {
    older_than_ms: i64,
    max_rows: u64,
    max_bytes: u64,
}

fn limits(older_than_ms: i64, max_rows: u64, max_bytes: u64) -> Limits {
    Limits {
        older_than_ms,
        max_rows,
        max_bytes,
    }
}

/// What the pruner does: the old ones through the index, then the excess
/// measured once and removed oldest first. Returns how many were deleted.
fn prune_all(db: &Database, limits: &Limits, batch: u32) -> usize {
    let mut total = 0;
    loop {
        let deleted = db
            .read(|c| samples::delete_older_than(c, limits.older_than_ms, batch))
            .unwrap();
        assert!(
            deleted <= batch as usize,
            "never more than one batch per step"
        );
        if deleted == 0 {
            break;
        }
        total += deleted;
    }
    let mut excess = db
        .read(samples::usage)
        .unwrap()
        .excess(limits.max_rows, limits.max_bytes);
    while !excess.is_none() {
        let gone = db
            .read(|c| samples::delete_oldest_covering(c, excess, batch))
            .unwrap();
        assert!(gone.rows <= u64::from(batch));
        if gone.rows == 0 {
            break;
        }
        total += gone.rows as usize;
        excess = Excess {
            rows: excess.rows.saturating_sub(gone.rows),
            bytes: excess.bytes.saturating_sub(gone.bytes),
        };
    }
    total
}

#[test]
fn pruning_by_age_deletes_only_the_old_ones() {
    let db = db();
    insert(&db, &(1..=10).map(sample).collect::<Vec<_>>());
    let deleted = prune_all(&db, &limits(1_006, u64::MAX, u64::MAX), 3);
    assert_eq!(deleted, 5, "samples 1 to 5 finished before 1006");
    let left = list(&db, &SampleFilter::default());
    assert_eq!(left.len(), 5);
    assert_eq!(left.last().unwrap().request_id, "req-6");
}

#[test]
fn pruning_by_rows_keeps_the_newest() {
    let db = db();
    insert(&db, &(1..=10).map(sample).collect::<Vec<_>>());
    let deleted = prune_all(&db, &limits(0, 4, u64::MAX), 3);
    assert_eq!(deleted, 6);
    let ids_left: Vec<String> = list(&db, &SampleFilter::default())
        .into_iter()
        .map(|r| r.request_id)
        .collect();
    assert_eq!(ids_left, ["req-10", "req-9", "req-8", "req-7"]);
}

#[test]
fn pruning_by_bytes_stops_once_under_the_limit() {
    let db = db();
    let mut rows: Vec<NewSample> = (1..=10).map(sample).collect();
    for row in &mut rows {
        row.detail = "x".repeat(1_000);
    }
    insert(&db, &rows);
    let total = db.read(samples::usage).unwrap().bytes;
    let one = total / 10;
    prune_all(&db, &limits(0, u64::MAX, one * 4 + 1), 3);
    let usage = db.read(samples::usage).unwrap();
    assert!(
        usage.bytes <= one * 4 + 1,
        "{} > {}",
        usage.bytes,
        one * 4 + 1
    );
    assert!(
        usage.bytes > one * 3,
        "it did not delete more than it had to"
    );
    assert_eq!(usage.rows, 4);
}

#[test]
fn pruning_with_everything_inside_the_limits_deletes_nothing() {
    let db = db();
    insert(&db, &(1..=5).map(sample).collect::<Vec<_>>());
    assert_eq!(prune_all(&db, &limits(0, 5, u64::MAX), 2), 0);
    assert_eq!(db.read(samples::usage).unwrap().rows, 5);
}

#[test]
fn age_rows_and_bytes_apply_together() {
    let db = db();
    insert(&db, &(1..=20).map(sample).collect::<Vec<_>>());
    prune_all(&db, &limits(1_004, 10, u64::MAX), 4);
    let left = list(&db, &SampleFilter::default());
    assert_eq!(left.len(), 10, "four too old, then six over the row limit");
    assert_eq!(left.last().unwrap().request_id, "req-11");
}

#[test]
fn clearing_deletes_in_steps_until_empty() {
    let db = db();
    insert(&db, &(1..=7).map(sample).collect::<Vec<_>>());
    let mut steps = 0;
    loop {
        let deleted = db.read(|c| samples::clear_step(c, 3)).unwrap();
        if deleted == 0 {
            break;
        }
        steps += 1;
    }
    assert_eq!(steps, 3);
    assert_eq!(db.read(samples::usage).unwrap().rows, 0);
}

#[test]
fn a_stored_row_that_cannot_be_read_back_is_corruption() {
    let db = db();
    insert(&db, &[sample(1)]);
    db.read(|c| {
        c.execute("UPDATE sample SET triggers = 4096", [])?;
        Ok(())
    })
    .unwrap();
    let err = db
        .read(|c| samples::list(c, &SampleFilter::default()))
        .unwrap_err();
    assert!(matches!(err, StoreError::Internal(_)));
}

#[test]
fn a_record_list_carries_the_detail_in_the_same_order_and_filters_as_the_summary_list() {
    let db = db();
    let mut rows: Vec<NewSample> = (1..=4).map(sample).collect();
    rows[1].subsystem = "blog".into();
    insert(&db, &rows);
    let filter = SampleFilter {
        subsystem: Some("shop".into()),
        limit: Some(2),
        ..Default::default()
    };
    let records = db.read(|c| samples::list_records(c, &filter)).unwrap();
    let summaries = list(&db, &filter);
    assert_eq!(
        records
            .iter()
            .map(|r| r.summary.clone())
            .collect::<Vec<_>>(),
        summaries
    );
    assert_eq!(records[0].detail, "{\"n\":4}");
    assert_eq!(records.len(), 2);
}

#[test]
fn a_full_database_reports_the_real_error_and_the_connection_stays_usable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.db");
    let mut db = Database::open(&path).unwrap();
    let pages: i64 = db
        .read(|c| Ok(c.pragma_query_value(None, "page_count", |r| r.get(0))?))
        .unwrap();
    db.read(|c| Ok(c.pragma_update(None, "max_page_count", pages + 6)?))
        .unwrap();
    let big: Vec<NewSample> = (0..100)
        .map(|n| NewSample {
            detail: "x".repeat(4000),
            ..sample(n)
        })
        .collect();
    let err = db
        .write_unrecorded(|c| samples::insert_batch(c, &big))
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("full"),
        "the error says the database is full, not that a savepoint is missing: {err}"
    );
    assert!(list(&db, &SampleFilter::default()).is_empty());
    db.write_unrecorded(|c| samples::insert_batch(c, &[sample(1)]))
        .unwrap();
    assert_eq!(list(&db, &SampleFilter::default()).len(), 1);
}
