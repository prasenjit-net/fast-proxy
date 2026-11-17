//! The sampling redactor on arbitrary headers, queries and bodies, whole or
//! cut (ADR 0021, M7 5.4).
#![no_main]

use std::sync::OnceLock;

use fast_proxy_model::SamplingSettings;
use fast_proxy_sampling::{Redactor, REDACTED};
use libfuzzer_sys::{arbitrary, fuzz_target};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input<'a> {
    header: (&'a str, &'a str),
    query: &'a str,
    content_type: &'a str,
    body: &'a [u8],
    truncated: bool,
}

fuzz_target!(|input: Input| {
    static REDACTOR: OnceLock<Redactor> = OnceLock::new();
    let redactor = REDACTOR.get_or_init(|| {
        Redactor::new(
            &SamplingSettings::default(),
            &["x-customer-secret".to_string()],
        )
    });
    let (name, value) = input.header;
    let shown = redactor.header_value(name, value);
    if redactor.is_sensitive_header(name) {
        assert_eq!(shown, REDACTED, "a sensitive header never shows its value");
    }
    let _ = redactor.query(input.query);
    let _ = redactor.body(input.content_type, input.body, input.truncated);
});
