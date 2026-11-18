//! The `config.toml` loader: parse, then the checks run before startup (M7 5.4).
#![no_main]

use fast_proxy_fuzz::config::AppConfig;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    if let Ok(config) = toml::from_str::<AppConfig>(text) {
        let _ = config.check();
        let _ = config.acme_warnings();
    }
});
