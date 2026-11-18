//! Which hosts get an ACME certificate, and the allowlist parser (M4 section 11).
#![no_main]

use fast_proxy_acme::eligible_names;
use fast_proxy_model::{DomainAllowlist, HostPattern};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (Vec<String>, Vec<&str>)| {
    let (allowed, hosts) = input;
    let Ok(allowlist) = DomainAllowlist::parse(&allowed) else {
        return;
    };
    let hosts: Vec<HostPattern> = hosts
        .iter()
        .filter_map(|raw| HostPattern::parse(raw).ok())
        .collect();
    let eligibility = eligible_names(&hosts, &allowlist);
    for name in &eligibility.eligible {
        assert!(!name.starts_with('*'), "a wildcard needs DNS-01: {name}");
    }
});
