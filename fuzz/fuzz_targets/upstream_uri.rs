//! `build_upstream_uri` on every route of the fixture (M7 5.4): the rewrite
//! builds what the upstream sees.
#![no_main]

use fast_proxy_routing::{build_upstream_uri, Lookup};
use http::Uri;
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

fuzz_target!(|input: (u8, &str)| {
    static SNAPSHOT: OnceLock<fast_proxy_routing::Snapshot> = OnceLock::new();
    let snapshot = SNAPSHOT.get_or_init(fast_proxy_fuzz::snapshot);
    let (pick, target) = input;
    let Ok(original) = target.parse::<Uri>() else {
        return;
    };
    let host =
        ["shop.example.com", "a.shop.example.com", "blog.example.com"][usize::from(pick) % 3];
    let Lookup::Matched { route, .. } = snapshot.lookup(host, original.path()) else {
        return;
    };
    let consumed = original.path().len() - route.remainder.len();
    if let Some(uri) = build_upstream_uri(
        route.route,
        &fast_proxy_fuzz::endpoint(),
        &original,
        consumed,
    ) {
        assert!(uri.path().starts_with('/'), "{uri}");
        assert_eq!(uri.authority().map(|a| a.as_str()), Some("10.0.0.1:8080"));
        assert_eq!(uri.query(), original.query(), "the query is kept as sent");
    }
});
