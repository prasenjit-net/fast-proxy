//! Hop-by-hop removal and the size checks before an endpoint is chosen (M7 5.4).
#![no_main]

use fast_proxy_dataplane::fuzzing::{
    check_size, is_upgrade_request, strip_hop_by_hop, strip_hop_by_hop_keeping_upgrade,
};
use fast_proxy_dataplane::RequestLimits;
use http::Uri;
use libfuzzer_sys::{arbitrary, fuzz_target};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    target: String,
    max_uri_bytes: u16,
    max_body_bytes: Option<u64>,
    route_cap: Option<u64>,
}

const HOP_BY_HOP: [&str; 7] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fuzz_target!(|input: Input| {
    let headers = fast_proxy_fuzz::headers(&input.headers);
    let _ = is_upgrade_request(&headers);

    let mut stripped = headers.clone();
    strip_hop_by_hop(&mut stripped);
    for name in HOP_BY_HOP {
        assert!(!stripped.contains_key(name), "{name} survived");
    }
    for value in headers.get_all("connection") {
        for token in value.to_str().unwrap_or("").split(',') {
            let token = token.trim();
            if !token.is_empty() {
                assert!(
                    !stripped.contains_key(token.to_ascii_lowercase().as_str()),
                    "{token}, named by Connection, survived"
                );
            }
        }
    }
    let mut upgrade = headers.clone();
    strip_hop_by_hop_keeping_upgrade(&mut upgrade);
    assert!(!upgrade.contains_key("transfer-encoding"));

    let Ok(uri) = input.target.parse::<Uri>() else {
        return;
    };
    let limits = RequestLimits {
        max_uri_bytes: usize::from(input.max_uri_bytes),
        max_body_bytes: input.max_body_bytes,
        ..RequestLimits::default()
    };
    match check_size(&uri, &headers, limits, input.route_cap) {
        Ok(cap) => assert_eq!(cap, input.route_cap.or(input.max_body_bytes)),
        Err(status) => assert!(status == 413 || status == 414, "{status}"),
    }
});
