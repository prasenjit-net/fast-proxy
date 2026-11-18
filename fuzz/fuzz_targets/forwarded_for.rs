//! The trusted-proxy `X-Forwarded-For` walk (M7 5.4): it decides the rate-limit
//! key and the maintenance bypass.
#![no_main]

use fast_proxy_dataplane::TrustedProxies;
use libfuzzer_sys::{arbitrary, fuzz_target};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    trusted: Vec<String>,
    peer: [u8; 17],
    headers: Vec<(Vec<u8>, Vec<u8>)>,
}

fuzz_target!(|input: Input| {
    let Ok(trusted) = TrustedProxies::parse(&input.trusted) else {
        return;
    };
    let peer = fast_proxy_fuzz::ip(input.peer);
    let headers = fast_proxy_fuzz::headers(&input.headers);
    let client = trusted.effective_client(peer, &headers);
    if !trusted.contains(peer) {
        assert_eq!(
            client, peer,
            "an untrusted peer is the client, whatever it sends"
        );
    }
    if client != peer {
        assert!(
            headers.contains_key("x-forwarded-for"),
            "a client other than the peer comes from X-Forwarded-For"
        );
    }
});
