//! The pipeline's resolve step on an arbitrary request against a fixed
//! snapshot, no network (M7 5.4): the whole request decision.
#![no_main]

use std::sync::OnceLock;

use fast_proxy_dataplane::fuzzing::{resolve, Decision};
use fast_proxy_routing::Snapshot;
use http::{Method, Request};
use libfuzzer_sys::{arbitrary, fuzz_target};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    method: String,
    target: String,
    host_pick: u8,
    host: String,
    /// No `Host` header at all (the URI's authority, or nothing, is used).
    no_host: bool,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    client: [u8; 17],
}

fuzz_target!(|input: Input| {
    static SNAPSHOT: OnceLock<Snapshot> = OnceLock::new();
    let snapshot = SNAPSHOT.get_or_init(fast_proxy_fuzz::snapshot);
    let Ok(method) = Method::from_bytes(input.method.as_bytes()) else {
        return;
    };
    let mut builder = Request::builder().method(method).uri(input.target.as_str());
    if !input.no_host {
        builder = builder.header(
            http::header::HOST,
            fast_proxy_fuzz::host(input.host_pick, &input.host),
        );
    }
    let Ok(mut request) = builder.body(()) else {
        return;
    };
    request
        .headers_mut()
        .extend(fast_proxy_fuzz::headers(&input.headers));
    let client = fast_proxy_fuzz::ip(input.client);
    match resolve(snapshot, &request, client, &fast_proxy_fuzz::endpoint()) {
        Decision::Forward(Some(uri)) => {
            assert!(uri.path().starts_with('/'), "{uri}");
            assert_eq!(uri.authority().map(|a| a.as_str()), Some("10.0.0.1:8080"));
        }
        Decision::Redirect { location, .. } => {
            assert!(location.starts_with("https://"), "{location}");
        }
        Decision::Respond(status) => assert!((400..600).contains(&status), "{status}"),
        Decision::Forward(None) | Decision::Maintenance | Decision::AcmeChallenge => {}
    }
});
