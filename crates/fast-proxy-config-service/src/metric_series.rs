use std::collections::HashSet;

use fast_proxy_routing::Snapshot;

/// `(subsystem, upstream, endpoint)` for every endpoint in `snapshot`: the
/// series the per-endpoint metrics may keep.
pub(crate) fn live_endpoints(snapshot: &Snapshot) -> HashSet<(String, String, String)> {
    snapshot
        .subsystems
        .values()
        .flat_map(|subsystem| {
            subsystem.upstreams.values().flat_map(move |upstream| {
                upstream.endpoints.iter().map(move |endpoint| {
                    (
                        subsystem.name.as_str().to_string(),
                        upstream.name.clone(),
                        endpoint.addr.to_string(),
                    )
                })
            })
        })
        .collect()
}
