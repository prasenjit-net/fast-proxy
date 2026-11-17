//! The sampler's scan of a request head for unusual traits (M6 section 11).
#![no_main]

use fast_proxy_dataplane::fuzzing::unusual_flags;
use http::{Method, Uri};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, &str, Vec<(Vec<u8>, Vec<u8>)>)| {
    let (method, target, pairs) = input;
    let (Ok(method), Ok(uri)) = (Method::from_bytes(method.as_bytes()), target.parse::<Uri>())
    else {
        return;
    };
    let headers = fast_proxy_fuzz::headers(&pairs);
    let _ = unusual_flags(&headers, &method, &uri);
});
