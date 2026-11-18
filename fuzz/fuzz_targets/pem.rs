//! PEM parsing and chain validation of uploaded certificates and CA bundles
//! (M7 5.4).
#![no_main]

use fast_proxy_tls::{parse_ca_bundle, parse_chain, parse_key, validate_upload};
use libfuzzer_sys::fuzz_target;

/// 2026-01-01, so validity windows are judged at a fixed time.
const NOW_MS: i64 = 1_767_225_600_000;

fuzz_target!(|text: &str| {
    // `chain NUL key`, so seeds can be real PEM files.
    let (chain, key) = text.split_once('\0').unwrap_or((text, ""));
    let _ = parse_chain(chain);
    let _ = parse_key(key);
    let _ = parse_ca_bundle(chain);
    let _ = validate_upload(chain, key, NOW_MS);
});
