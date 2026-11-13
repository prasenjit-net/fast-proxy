use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fast_proxy_model::{SamplingSettings, SubsystemName, Trigger};
use fast_proxy_sampling::{
    RawSample, Redactor, Sample, SampleQuery, SampleSink, SamplerConfig, SamplerRuntime,
};
use fast_proxy_store::repository::samples::{self, SampleFilter};
use fast_proxy_store::repository::{sampling, subsystems};
use fast_proxy_store::{Database, StoreError, SubsystemFields};

use crate::test_support::{sample, Shared, TIMEOUT};
use crate::{prune_once, Pruner, PrunerConfig, SampleReader, SampleWriter};

fn rows(db: &Database) -> u64 {
    db.read(samples::usage).unwrap().rows
}

fn write(writer: &mut SampleWriter, batch: Vec<Sample>) {
    writer.write_batch(batch).unwrap();
}

fn set_retention(db: &Database, days: u32, max_rows: u64, max_bytes: u64) {
    let settings = SamplingSettings {
        retention_days: days,
        max_rows,
        max_bytes,
        ..SamplingSettings::default()
    };
    db.read(|conn| sampling::set_settings(conn, &settings))
        .unwrap();
}

fn insert_many(db: &mut Database, instance: &str, count: i64, detail_bytes: usize) {
    insert_many_from(db, instance, 0, count, detail_bytes);
}

/// Samples that finished at `base_ms + n`; the pruner thread uses the real clock,
/// so its tests write samples from just now.
fn insert_many_from(
    db: &mut Database,
    instance: &str,
    base_ms: i64,
    count: i64,
    detail_bytes: usize,
) {
    for start in (0..count).step_by(1000) {
        let batch: Vec<Sample> = (start..(start + 1000).min(count))
            .map(|n| {
                let mut s = sample(base_ms + n, instance, "shop");
                s.request.path = "x".repeat(detail_bytes);
                s
            })
            .collect();
        let rows: Vec<_> = batch.iter().map(crate::convert::to_row).collect();
        db.write_unrecorded(|c| samples::insert_batch(c, &rows))
            .unwrap();
    }
}

// ---- the writer ----

#[test]
fn a_batch_lands_in_the_table_with_its_columns_and_records_no_revision() {
    let shared = Shared::new();
    let mut writer = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
    let mut slow = sample(2, "a", "blog");
    slow.triggers = vec!["slow".into(), "baseline".into()];
    slow.primary = "slow".into();
    slow.response.status = 200;
    slow.total_ms = 4000;
    write(&mut writer, vec![sample(1, "a", "shop"), slow]);

    let listed = shared
        .primary
        .read(|c| samples::list(c, &SampleFilter::default()))
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(
        (
            listed[0].subsystem.as_str(),
            listed[0].status,
            listed[0].total_ms
        ),
        ("blog", 200, 4000)
    );
    assert_eq!(listed[0].primary, Trigger::Slow);
    assert_eq!(
        listed[0].triggers.bits(),
        Trigger::Slow.bit() | Trigger::Baseline.bit()
    );
    assert_eq!(listed[1].primary, Trigger::UpstreamFailure);
    assert_eq!(
        shared.primary.current_version().unwrap(),
        0,
        "no revision, no generation bump"
    );
}

#[test]
fn a_locked_database_is_a_sink_error_and_the_writer_recovers() {
    let shared = Shared::new();
    let mut writer = SampleWriter::open(&shared.path, Duration::from_millis(100)).unwrap();
    let holder = rusqlite::Connection::open(&shared.path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    let err = writer
        .write_batch(vec![sample(1, "a", "shop")])
        .unwrap_err();
    assert!(
        started.elapsed() < TIMEOUT,
        "it gave up at the busy timeout"
    );
    assert!(!err.to_string().is_empty());
    assert_eq!(rows(&shared.primary), 0, "nothing was half-written");
    holder.execute_batch("ROLLBACK").unwrap();
    write(&mut writer, vec![sample(2, "a", "shop")]);
    assert_eq!(rows(&shared.primary), 1);
}

#[test]
fn a_full_disk_is_an_error_not_a_panic_and_the_writer_recovers_when_there_is_room() {
    let shared = Shared::new();
    let mut writer = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
    let pages: i64 = shared
        .primary
        .read(|c| Ok(c.pragma_query_value(None, "page_count", |r| r.get(0))?))
        .unwrap();
    // The writer's own connection may not grow the file past a few more pages.
    let limited = Database::open_secondary(&shared.path, TIMEOUT).unwrap();
    limited
        .read(|c| Ok(c.pragma_update(None, "max_page_count", pages + 8)?))
        .unwrap();
    let mut small = SampleWriter::from_database(limited);
    let big: Vec<Sample> = (0..200)
        .map(|n| {
            let mut s = sample(n, "a", "shop");
            s.request.path = "p".repeat(4000);
            s
        })
        .collect();
    let err = small.write_batch(big).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("full"), "{err}");
    assert_eq!(
        rows(&shared.primary),
        0,
        "the failed batch left nothing behind"
    );
    write(&mut writer, vec![sample(1, "a", "shop")]);
    assert_eq!(rows(&shared.primary), 1, "a writer with room is unaffected");
}

#[test]
fn two_writers_produce_one_increasing_sequence_of_ids() {
    let shared = Shared::new();
    let threads: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|instance| {
            let mut writer = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
            std::thread::spawn(move || {
                for round in 0..20 {
                    let batch = (0..25)
                        .map(|n| sample(round * 100 + n, instance, "shop"))
                        .collect();
                    writer.write_batch(batch).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let all = shared
        .primary
        .read(|c| {
            samples::list(
                c,
                &SampleFilter {
                    limit: Some(500),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert_eq!(all.len(), 500);
    let mut ids: Vec<i64> = all.iter().map(|s| s.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 500, "every id is unique");
    for instance in ["a", "b"] {
        let mut own: Vec<(i64, i64)> = all
            .iter()
            .filter(|s| s.instance == instance)
            .map(|s| (s.id, s.at_ms))
            .collect();
        own.sort_unstable();
        assert!(
            own.windows(2)
                .all(|w| w[0].1 <= w[1].1 || w[0].1 / 100 != w[1].1 / 100),
            "one writer's batches are in the order it wrote them"
        );
    }
}

fn raw(n: u32) -> RawSample {
    use fast_proxy_sampling::{Answer, AttemptLog, Outcome, RequestHead, UnusualFlags};
    RawSample {
        at_ms: i64::from(n),
        request_id: format!("req-{n}"),
        triggers: [Trigger::Slow].into_iter().collect(),
        subsystem: "shop".into(),
        route: "/".into(),
        head: RequestHead {
            method: "GET".into(),
            path: "/".into(),
            query: Some("token=ZQ".into()),
            headers: vec![("authorization".into(), "Bearer ZQ".into())],
            flags: UnusualFlags::default(),
            client: "203.0.113.9:1".parse().unwrap(),
        },
        outcome: Outcome {
            status: 200,
            elapsed_ms: 3000,
            answer: Answer::Upstream,
            attempts: AttemptLog::default(),
            bytes_in: 0,
            bytes_out: 0,
            client_cancelled: false,
        },
        attempts: Vec::new(),
        response_headers: Vec::new(),
        request_body: None,
        response_body: None,
        body_omitted: None,
        suppressed: None,
        reservation: None,
    }
}

#[test]
fn the_sampler_with_the_real_writer_stores_redacted_samples_and_flushes_at_shutdown() {
    let shared = Shared::new();
    let writer = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
    let runtime = SamplerRuntime::start(
        SamplerConfig {
            queue_depth: 64,
            batch_size: 100,
            batch_interval: Duration::from_secs(3600),
            body_budget_bytes: 1_000_000,
            instance: "inst-1".into(),
        },
        Redactor::default(),
        writer,
    )
    .unwrap();
    let sampler = runtime.handle();
    for n in 0..30 {
        sampler.try_submit(raw(n)).unwrap();
    }
    runtime.shutdown();
    assert_eq!(rows(&shared.primary), 30, "the partial batch was flushed");
    let reader = SampleReader::open(&shared.path, TIMEOUT).unwrap();
    let newest = reader
        .list(&SampleQuery {
            limit: Some(1),
            ..Default::default()
        })
        .unwrap();
    let stored = reader.get(newest[0].id).unwrap();
    assert_eq!(stored.instance, "inst-1");
    assert_eq!(stored.request.query.as_deref(), Some("token=[redacted]"));
    let bytes = std::fs::read(&shared.path).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("Bearer ZQ"),
        "no raw secret reached the database file"
    );
}

// ---- the pruner ----

const DAY_MS: i64 = 86_400_000;

fn real_now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// `recent` samples from just now and `old` from two days ago, each with a
/// path of `path_bytes` so the byte limit has something to count.
fn aged_table(recent: i64, old: i64, path_bytes: usize) -> (Shared, i64) {
    let mut shared = Shared::new();
    let now = 10 * DAY_MS;
    let batch: Vec<Sample> = (0..recent + old)
        .map(|n| {
            let mut s = sample(now - 10 + n, "a", "shop");
            if n < old {
                s.at_ms = now - 2 * DAY_MS + n;
            }
            s.request.path = "p".repeat(path_bytes);
            s
        })
        .collect();
    let rows: Vec<_> = batch.iter().map(crate::convert::to_row).collect();
    shared
        .primary
        .write_unrecorded(|c| samples::insert_batch(c, &rows))
        .unwrap();
    (shared, now)
}

#[test]
fn samples_older_than_the_retention_are_pruned_and_nothing_else() {
    let (mut shared, now) = aged_table(40, 20, 10);
    set_retention(&shared.primary, 1, 10_000, 1 << 30);
    let report = prune_once(&mut shared.primary, now, 7, Duration::ZERO).unwrap();
    assert_eq!((report.deleted, report.usage.rows), (20, 40));
    let again = prune_once(&mut shared.primary, now, 7, Duration::ZERO).unwrap();
    assert_eq!(again.deleted, 0, "a second pass finds nothing to do");
}

#[test]
fn the_row_limit_keeps_the_newest_rows() {
    let (mut shared, now) = aged_table(250, 50, 10);
    set_retention(&shared.primary, 365, 120, 1 << 30);
    let report = prune_once(&mut shared.primary, now, 7, Duration::ZERO).unwrap();
    assert_eq!((report.deleted, report.usage.rows), (180, 120));
}

#[test]
fn the_byte_limit_stops_once_the_table_fits() {
    // 20 samples of about 150 KB: three MB, against a limit of one MB.
    let (mut shared, now) = aged_table(20, 0, 150_000);
    set_retention(&shared.primary, 365, 10_000, 1 << 20);
    let report = prune_once(&mut shared.primary, now, 3, Duration::ZERO).unwrap();
    assert!(
        report.usage.bytes <= 1 << 20,
        "{} bytes left",
        report.usage.bytes
    );
    assert!(
        report.usage.bytes > (1 << 20) - 160_000,
        "it did not delete more than it had to"
    );
    assert!(report.usage.rows >= 6 && report.usage.rows < 20);
}

#[test]
fn age_rows_and_bytes_apply_together() {
    let (mut shared, now) = aged_table(250, 50, 10);
    set_retention(&shared.primary, 1, 100, 1 << 30);
    let report = prune_once(&mut shared.primary, now, 7, Duration::ZERO).unwrap();
    assert_eq!(
        report.usage.rows, 100,
        "the 50 old ones go by age, then 150 more by the row limit"
    );
    assert_eq!(report.deleted, 200);
}

#[test]
fn a_changed_retention_applies_on_the_next_pass() {
    let mut shared = Shared::new();
    insert_many(&mut shared.primary, "a", 500, 10);
    set_retention(&shared.primary, 7, 10_000, 1 << 30);
    let first = prune_once(&mut shared.primary, 1_000, 100, Duration::ZERO).unwrap();
    assert_eq!(first.deleted, 0);
    set_retention(&shared.primary, 7, 200, 1 << 30);
    let second = prune_once(&mut shared.primary, 1_000, 100, Duration::ZERO).unwrap();
    assert_eq!(second.usage.rows, 200);
    assert_eq!(second.deleted, 300);
}

#[test]
fn the_oldest_samples_are_the_ones_pruned() {
    let mut shared = Shared::new();
    insert_many(&mut shared.primary, "a", 300, 10);
    set_retention(&shared.primary, 365, 100, 1 << 30);
    prune_once(&mut shared.primary, 1_000, 50, Duration::ZERO).unwrap();
    let left = shared
        .primary
        .read(|c| {
            samples::list(
                c,
                &SampleFilter {
                    limit: Some(500),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert_eq!(left.len(), 100);
    assert!(
        left.iter().all(|s| s.at_ms >= 200),
        "samples 200 to 299 remain"
    );
}

#[test]
fn two_pruners_racing_neither_fail_nor_delete_more_than_a_batch_too_many() {
    let mut shared = Shared::new();
    insert_many(&mut shared.primary, "a", 5_000, 10);
    set_retention(&shared.primary, 365, 1_000, 1 << 30);
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let mut db = shared.secondary();
            std::thread::spawn(move || prune_once(&mut db, 1_000, 100, Duration::ZERO).unwrap())
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let left = rows(&shared.primary);
    assert!(
        (900..=1_000).contains(&left),
        "{left} rows left of a limit of 1000"
    );
}

#[test]
fn a_large_prune_never_holds_the_write_lock_for_long_and_configuration_writes_get_in() {
    let mut shared = Shared::new();
    insert_many(&mut shared.primary, "a", 40_000, 5);
    set_retention(&shared.primary, 365, 1_000, 1 << 30);
    let done = Arc::new(AtomicBool::new(false));
    let pruning = {
        let (mut db, done) = (shared.secondary(), done.clone());
        std::thread::spawn(move || {
            let report = prune_once(&mut db, 1_000, 1_000, Duration::from_millis(10)).unwrap();
            done.store(true, Ordering::SeqCst);
            report
        })
    };
    let mut slowest = Duration::ZERO;
    let mut writes = 0;
    while !done.load(Ordering::SeqCst) {
        let started = Instant::now();
        shared
            .primary
            .write(
                |conn| {
                    let id = subsystems::create(
                        conn,
                        &SubsystemFields {
                            name: SubsystemName::parse(&format!("s{writes}")).unwrap(),
                            enabled: true,
                            description: String::new(),
                            https: fast_proxy_model::HttpsPolicy::default(),
                        },
                    )?;
                    Ok(fast_proxy_store::Mutation {
                        value: id,
                        summary: "create".into(),
                    })
                },
                |_| Ok(()),
            )
            .unwrap();
        slowest = slowest.max(started.elapsed());
        writes += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
    let report = pruning.join().unwrap();
    assert_eq!(report.usage.rows, 1_000);
    assert_eq!(report.deleted, 39_000);
    assert!(writes > 0);
    assert!(
        slowest < Duration::from_secs(2),
        "a configuration write waited {slowest:?} for the pruner"
    );
}

#[test]
fn the_pruner_thread_prunes_counts_and_stops() {
    let mut shared = Shared::new();
    insert_many_from(&mut shared.primary, "a", real_now_ms(), 300, 10);
    set_retention(&shared.primary, 365, 100, 1 << 30);
    let pruner = Pruner::start(
        &shared.path,
        PrunerConfig {
            interval: Duration::from_millis(20),
            batch: 50,
            pause: Duration::ZERO,
            busy_timeout: TIMEOUT,
        },
    )
    .unwrap();
    let stats = pruner.stats().clone();
    let started = Instant::now();
    while stats.read().deleted < 200 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the first pass never finished"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(stats.read().rows, 100);
    assert!(stats.read().bytes > 0);
    pruner.stop();
    assert_eq!(rows(&shared.primary), 100);
    assert_eq!(stats.read().failed_passes, 0);
}

#[test]
fn a_pass_that_cannot_get_the_lock_is_counted_and_the_next_one_works() {
    let mut shared = Shared::new();
    insert_many_from(&mut shared.primary, "a", real_now_ms(), 300, 10);
    set_retention(&shared.primary, 365, 100, 1 << 30);
    let holder = rusqlite::Connection::open(&shared.path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let pruner = Pruner::start(
        &shared.path,
        PrunerConfig {
            interval: Duration::from_millis(20),
            batch: 100,
            pause: Duration::ZERO,
            busy_timeout: Duration::from_millis(50),
        },
    )
    .unwrap();
    let stats = pruner.stats().clone();
    let started = Instant::now();
    while stats.read().failed_passes == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "no failure was counted"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    holder.execute_batch("ROLLBACK").unwrap();
    while stats.read().rows != 100 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the pruner never recovered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    pruner.stop();
}

// ---- the reader ----

fn two_instances() -> (Shared, SampleReader) {
    let shared = Shared::new();
    let mut a = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
    let mut b = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
    let mut slow = sample(3, "a", "shop");
    slow.triggers = vec!["slow".into()];
    slow.primary = "slow".into();
    slow.total_ms = 5000;
    slow.response.status = 200;
    let mut other_route = sample(4, "b", "blog");
    other_route.route = "/static".into();
    other_route.client = "198.51.100.1:7".into();
    other_route.response.status = 404;
    write(
        &mut a,
        vec![sample(1, "a", "shop"), sample(2, "a", "shop"), slow],
    );
    write(&mut b, vec![other_route, sample(5, "b", "shop")]);
    let reader = SampleReader::open(&shared.path, TIMEOUT).unwrap();
    (shared, reader)
}

fn ids(reader: &SampleReader, query: &SampleQuery) -> Vec<String> {
    reader
        .list(query)
        .unwrap()
        .into_iter()
        .map(|s| s.request_id)
        .collect()
}

#[test]
fn a_reader_sees_the_samples_of_every_instance_and_every_filter_works() {
    let (_shared, reader) = two_instances();
    let all = ids(&reader, &SampleQuery::default());
    assert_eq!(all.len(), 5, "both instances' samples in one list");
    assert_eq!(all[0], "req-b-5", "newest first");
    let q = |f: &dyn Fn(&mut SampleQuery)| {
        let mut query = SampleQuery::default();
        f(&mut query);
        ids(&reader, &query)
    };
    assert_eq!(q(&|x| x.triggers = vec!["slow".into()]), ["req-a-3"]);
    assert_eq!(q(&|x| x.status_class = Some(4)), ["req-b-4"]);
    assert_eq!(q(&|x| x.subsystem = Some("blog".into())), ["req-b-4"]);
    assert_eq!(q(&|x| x.route = Some("/static".into())), ["req-b-4"]);
    assert_eq!(q(&|x| x.instance = Some("a".into())).len(), 3);
    assert_eq!(q(&|x| x.instance = Some("b".into())).len(), 2);
    assert_eq!(
        q(&|x| x.client = Some("198.51.100.1:7".into())),
        ["req-b-4"]
    );
    assert_eq!(q(&|x| x.min_ms = Some(1000)), ["req-a-3"]);
    assert_eq!(q(&|x| x.since_ms = Some(4)).len(), 2);
}

#[test]
fn an_invalid_query_is_refused_before_it_reaches_the_database() {
    let (_shared, reader) = two_instances();
    let err = reader
        .list(&SampleQuery {
            triggers: vec!["server_error".into()],
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "trigger"));
    let err = reader
        .list(&SampleQuery {
            limit: Some(0),
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "limit"));
}

#[test]
fn a_cursor_stays_stable_while_another_instance_writes() {
    let (shared, reader) = two_instances();
    let first = reader
        .list(&SampleQuery {
            limit: Some(2),
            ..Default::default()
        })
        .unwrap();
    let mut other = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
    write(
        &mut other,
        vec![sample(6, "c", "shop"), sample(7, "c", "shop")],
    );
    let second = reader
        .list(&SampleQuery {
            limit: Some(2),
            before_id: Some(first[1].id),
            ..Default::default()
        })
        .unwrap();
    let third = reader
        .list(&SampleQuery {
            limit: Some(5),
            before_id: Some(second[1].id),
            ..Default::default()
        })
        .unwrap();
    let seen: Vec<String> = first
        .iter()
        .chain(&second)
        .chain(&third)
        .map(|s| s.request_id.clone())
        .collect();
    assert_eq!(
        seen.len(),
        5,
        "no repeats and none missed, the two new ones are above the cursor"
    );
    assert_eq!(seen[0], "req-b-5");
}

#[test]
fn a_list_never_reads_the_detail_but_a_single_sample_does() {
    let (shared, reader) = two_instances();
    shared
        .primary
        .read(|c| {
            c.execute("UPDATE sample SET detail = 'not json'", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        reader.list(&SampleQuery::default()).unwrap().len(),
        5,
        "the list does not parse it"
    );
    let id = reader.list(&SampleQuery::default()).unwrap()[0].id;
    assert!(matches!(
        reader.get(id).unwrap_err(),
        StoreError::Internal(_)
    ));
}

#[test]
fn one_sample_comes_back_whole_with_its_id_and_a_pruned_one_is_not_found() {
    let (_shared, reader) = two_instances();
    let listed = reader.list(&SampleQuery::default()).unwrap();
    let whole = reader.get(listed[0].id).unwrap();
    assert_eq!(whole.id, Some(listed[0].id));
    assert_eq!(whole.request_id, "req-b-5");
    assert_eq!(whole.request.headers[0].0, "accept");
    assert!(matches!(
        reader.get(9_999).unwrap_err(),
        StoreError::NotFound(_)
    ));
}

#[test]
fn the_export_visits_every_matching_sample_newest_first_across_pages() {
    let mut shared = Shared::new();
    insert_many(&mut shared.primary, "a", 450, 5);
    let reader = SampleReader::open(&shared.path, TIMEOUT).unwrap();
    let mut seen = Vec::new();
    let outcome = reader
        .export_each(&SampleQuery::default(), |sample| {
            seen.push(sample.at_ms);
            Ok::<(), ()>(())
        })
        .unwrap();
    assert!(outcome.is_ok());
    assert_eq!(
        seen.len(),
        450,
        "more than two pages, the query's limit is not applied"
    );
    assert!(
        seen.windows(2).all(|w| w[0] > w[1]),
        "newest first, no repeats"
    );
}

#[test]
fn the_export_honours_the_filters_and_stops_at_the_first_error_from_the_visitor() {
    let (_shared, reader) = two_instances();
    let mut seen = 0;
    reader
        .export_each(
            &SampleQuery {
                instance: Some("b".into()),
                ..Default::default()
            },
            |_| {
                seen += 1;
                Ok::<(), ()>(())
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(seen, 2);
    let mut visited = 0;
    let outcome = reader
        .export_each(&SampleQuery::default(), |_| {
            visited += 1;
            if visited == 2 {
                Err("the client went away")
            } else {
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(outcome, Err("the client went away"));
    assert_eq!(visited, 2);
}

#[test]
fn clearing_empties_the_table_for_every_instance_and_says_how_many() {
    let mut shared = Shared::new();
    insert_many(&mut shared.primary, "a", 1_300, 5);
    insert_many(&mut shared.primary, "b", 200, 5);
    let reader = SampleReader::open(&shared.path, TIMEOUT).unwrap();
    assert_eq!(reader.usage().unwrap().rows, 1_500);
    assert_eq!(reader.clear().unwrap(), 1_500);
    assert_eq!(reader.usage().unwrap().rows, 0);
    assert_eq!(reader.clear().unwrap(), 0);
}

#[test]
fn the_reader_does_not_block_the_writer_and_sees_new_samples() {
    let shared = Shared::new();
    let reader = SampleReader::open(&shared.path, TIMEOUT).unwrap();
    assert!(reader.list(&SampleQuery::default()).unwrap().is_empty());
    let mut writer = SampleWriter::open(&shared.path, TIMEOUT).unwrap();
    write(&mut writer, vec![sample(1, "a", "shop")]);
    assert_eq!(reader.list(&SampleQuery::default()).unwrap().len(), 1);
}
