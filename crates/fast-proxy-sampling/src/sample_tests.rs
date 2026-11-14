use fast_proxy_model::{Trigger, TriggerSet};

use super::*;
use crate::facts::{Answer, AttemptLog, Outcome, RequestHead, UnusualFlags};

fn raw() -> RawSample {
    RawSample {
        at_ms: 1_700_000_000_000,
        request_id: "req-1".into(),
        triggers: [Trigger::UpstreamFailure, Trigger::Retried]
            .into_iter()
            .collect(),
        subsystem: "shop".into(),
        route: "/api".into(),
        head: RequestHead {
            method: "POST".into(),
            path: "/api/orders".into(),
            query: Some("page=2&token=ZQsecret".into()),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("authorization".into(), "Bearer ZQsecret".into()),
                ("cookie".into(), "s=ZQsecret".into()),
            ],
            flags: UnusualFlags::default().with(UnusualFlags::HUGE_PATH),
            client: "203.0.113.9:51544".parse().unwrap(),
        },
        outcome: Outcome {
            status: 504,
            elapsed_ms: 30_012,
            answer: Answer::ProxyFailure,
            attempts: AttemptLog::default(),
            bytes_in: 812,
            bytes_out: 0,
            client_cancelled: false,
        },
        attempts: vec![
            AttemptDetail {
                endpoint: "10.0.0.4:8080".into(),
                failure: Some("connect_refused".into()),
                status: None,
            },
            AttemptDetail {
                endpoint: "10.0.0.5:8080".into(),
                failure: Some("response_timeout".into()),
                status: None,
            },
        ],
        response_headers: vec![
            ("set-cookie".into(), "id=ZQsecret".into()),
            ("content-type".into(), "text/plain".into()),
        ],
        request_body: Some(CapturedBody {
            content_type: "application/json".into(),
            bytes: br#"{"card":"ZQ4111","note":"hi"}"#.to_vec(),
            truncated: false,
        }),
        response_body: None,
        body_omitted: None,
        suppressed: Some(Suppressed {
            count: 4812,
            since_ms: 60_000,
        }),
        reservation: None,
    }
}

fn sample() -> Sample {
    Sample::from_raw(raw(), &Redactor::default(), "a1b2")
}

#[test]
fn a_raw_sample_becomes_a_redacted_sample() {
    let s = sample();
    assert_eq!(s.instance, "a1b2");
    assert_eq!(s.triggers, ["upstream_failure", "retried"]);
    assert_eq!(s.primary, "upstream_failure");
    assert_eq!(s.client, "203.0.113.9:51544");
    assert_eq!(s.request.query.as_deref(), Some("page=2&token=[redacted]"));
    assert_eq!(
        s.request.headers,
        [
            ("content-type".to_string(), "application/json".to_string()),
            ("authorization".to_string(), "[redacted]".to_string()),
            ("cookie".to_string(), "[redacted]".to_string()),
        ]
    );
    assert_eq!(s.response.headers[0].1, "[redacted]");
    assert_eq!(
        s.request.body.as_ref().unwrap().text,
        r#"{"card":"[redacted]","note":"hi"}"#
    );
    assert_eq!(s.request.unusual, ["huge_path"]);
    assert_eq!(
        (s.response.status, s.total_ms, s.request.bytes),
        (504, 30_012, 812)
    );
    assert_eq!(s.upstream.attempts.len(), 2);
    assert_eq!(
        s.upstream.attempts[1].failure.as_deref(),
        Some("response_timeout")
    );
    assert_eq!(
        s.suppressed,
        Some(SampleSuppressed {
            count: 4812,
            since_ms: 60_000
        })
    );
}

#[test]
fn nothing_secret_is_in_the_stored_json() {
    let json = sample().to_detail_json();
    assert!(!json.contains("ZQ"), "{json}");
}

#[test]
fn the_json_uses_the_api_spelling_and_round_trips() {
    let s = sample();
    let json = s.to_detail_json();
    for key in [
        "\"atMs\"",
        "\"requestId\"",
        "\"totalMs\"",
        "\"bodyOmitted\"",
        "\"sinceMs\"",
    ] {
        assert!(json.contains(key), "{key} in {json}");
    }
    assert!(
        !json.contains("\"id\""),
        "no id before the sample is stored"
    );
    assert_eq!(Sample::from_detail_json(&json).unwrap(), s);
    let mut stored = s.clone();
    stored.id = Some(42);
    let back = Sample::from_detail_json(&stored.to_detail_json()).unwrap();
    assert_eq!(back.id, Some(42));
}

#[test]
fn a_sample_with_no_suppressed_count_or_flags_leaves_them_out() {
    let mut r = raw();
    r.suppressed = None;
    r.head.flags = UnusualFlags::default();
    let json = Sample::from_raw(r, &Redactor::default(), "i").to_detail_json();
    assert!(!json.contains("suppressed"));
    assert!(!json.contains("unusual"));
}

#[test]
fn the_trigger_set_round_trips_through_the_names() {
    let s = sample();
    let expected: TriggerSet = [Trigger::UpstreamFailure, Trigger::Retried]
        .into_iter()
        .collect();
    assert_eq!(s.trigger_set(), expected);
}

#[test]
fn headers_and_values_are_capped() {
    let mut r = raw();
    r.head.headers = (0..100)
        .map(|n| (format!("x-h{n}"), "v".repeat(5000)))
        .collect();
    let s = Sample::from_raw(r, &Redactor::default(), "i");
    assert_eq!(s.request.headers.len(), MAX_HEADERS);
    assert!(s
        .request
        .headers
        .iter()
        .all(|(_, v)| v.len() == MAX_HEADER_VALUE_BYTES));
}

#[test]
fn path_and_query_are_capped_on_a_character_boundary() {
    let mut r = raw();
    r.head.path = "é".repeat(MAX_PATH_BYTES);
    r.head.query = Some("q=".to_string() + &"ü".repeat(MAX_PATH_BYTES));
    let s = Sample::from_raw(r, &Redactor::default(), "i");
    assert!(s.request.path.len() <= MAX_PATH_BYTES);
    assert!(s.request.query.unwrap().len() <= MAX_PATH_BYTES);
    assert_eq!(cap("abc", 10), "abc");
    assert_eq!(cap("aé", 2), "a", "never splits a character");
}

#[test]
fn a_body_of_an_unsupported_type_is_left_out_with_the_reason() {
    let mut r = raw();
    r.request_body = Some(CapturedBody {
        content_type: "image/png".into(),
        bytes: b"ZQ".to_vec(),
        truncated: false,
    });
    let s = Sample::from_raw(r, &Redactor::default(), "i");
    assert!(s.request.body.is_none());
    assert_eq!(s.response.body_omitted.as_deref(), Some("content_type"));
    assert!(!s.to_detail_json().contains("ZQ"));
}

#[test]
fn the_reason_there_is_no_body_is_kept() {
    let mut r = raw();
    r.request_body = None;
    r.body_omitted = Some("budget");
    let s = Sample::from_raw(r, &Redactor::default(), "i");
    assert_eq!(s.response.body_omitted.as_deref(), Some("budget"));
}

#[test]
fn a_truncated_body_says_so() {
    let mut r = raw();
    r.request_body = Some(CapturedBody {
        content_type: "text/plain".into(),
        bytes: b"hello wor".to_vec(),
        truncated: true,
    });
    let s = Sample::from_raw(r, &Redactor::default(), "i");
    assert!(s.request.body.unwrap().truncated);
}

#[test]
fn the_stored_size_is_the_detail_plus_the_short_text_columns() {
    let s = sample();
    let detail = s.to_detail_json();
    let expected = detail.len()
        + "a1b2".len()
        + "req-1".len()
        + "shop".len()
        + "/api".len()
        + "203.0.113.9:51544".len()
        + "POST".len();
    assert_eq!(s.stored_size(&detail), expected);
}

#[test]
fn the_export_is_one_valid_json_document_per_line() {
    let mut other = sample();
    other.request_id = "req-2".into();
    other.id = Some(7);
    let mut out = Vec::new();
    write_ndjson(&mut out, [&sample(), &other]).unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(text.ends_with('\n'));
    assert_eq!(Sample::from_detail_json(lines[0]).unwrap(), sample());
    assert_eq!(Sample::from_detail_json(lines[1]).unwrap().id, Some(7));
    assert!(lines.iter().all(|line| !line.contains('\n')));
}

#[test]
fn an_empty_export_is_empty() {
    let mut out = Vec::new();
    write_ndjson(&mut out, []).unwrap();
    assert!(out.is_empty());
}
