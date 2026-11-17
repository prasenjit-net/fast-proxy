//! `HostPattern::parse`, its written form, and host matching (M7 5.4).
#![no_main]

use fast_proxy_model::{HostPattern, SubsystemId};
use fast_proxy_routing::{dns_host, HostTable};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    // `pattern NUL host-header`, so seeds can be written by hand.
    let (pattern, host_header) = text.split_once('\0').unwrap_or((text, ""));
    let _ = dns_host(host_header);
    let Ok(parsed) = HostPattern::parse(pattern) else {
        return;
    };
    let written = parsed.to_pattern_string();
    assert_eq!(
        HostPattern::parse(&written).as_ref(),
        Ok(&parsed),
        "a pattern's written form parses back to it"
    );
    let table = HostTable::build([(&parsed, SubsystemId(1))]);
    let _ = table.lookup(host_header);
    if !written.starts_with('*') {
        assert_eq!(
            table.lookup(&written),
            Some(SubsystemId(1)),
            "an exact host finds itself"
        );
    }
});
