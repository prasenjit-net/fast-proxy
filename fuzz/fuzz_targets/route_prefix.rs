//! `RoutePrefix::parse` and the route matcher (M7 5.4).
#![no_main]

use fast_proxy_model::RoutePrefix;
use fast_proxy_routing::RouteTable;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (Vec<&str>, &str)| {
    let (prefixes, path) = input;
    let parsed: Vec<RoutePrefix> = prefixes
        .iter()
        .filter_map(|raw| RoutePrefix::parse(raw).ok())
        .collect();
    for prefix in &parsed {
        assert_eq!(
            RoutePrefix::parse(prefix.as_str()).as_ref(),
            Ok(prefix),
            "a prefix's normal form parses back to it"
        );
    }
    let table = RouteTable::build(parsed.into_iter().map(|prefix| (prefix.clone(), prefix)));
    if let Some(found) = table.find(path) {
        assert!(
            path.ends_with(found.remainder),
            "the remainder is the rest of the path"
        );
        let consumed = &path[..path.len() - found.remainder.len()];
        let prefix = found.route.as_str().trim_end_matches('/');
        assert!(
            consumed.trim_end_matches('/') == prefix || prefix.is_empty(),
            "the consumed part is the matched prefix: {consumed:?} for {prefix:?}"
        );
    }
});
