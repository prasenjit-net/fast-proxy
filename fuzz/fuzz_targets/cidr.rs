//! `Cidr::parse`, its written form and membership (M7 5.4): trusted proxies
//! and maintenance bypass.
#![no_main]

use fast_proxy_model::Cidr;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    // `network NUL address`, so seeds can be written by hand.
    let (raw, address) = text.split_once('\0').unwrap_or((text, ""));
    let address = address.parse::<std::net::IpAddr>().ok();
    let Ok(network) = Cidr::parse("cidr", raw) else {
        return;
    };
    let written = network.to_cidr_string();
    assert_eq!(
        Cidr::parse("cidr", &written).as_ref(),
        Ok(&network),
        "{written}"
    );
    if let Some(address) = address {
        let _ = network.contains(address);
    }
});
