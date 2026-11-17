//! Every admin API write body through serde and the validation its handler
//! runs (M7 5.4): every write's input.
#![no_main]

use fast_proxy_admin::fuzzing::accept_body;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (u8, &[u8])| {
    let (kind, json) = input;
    accept_body(kind, json);
});
