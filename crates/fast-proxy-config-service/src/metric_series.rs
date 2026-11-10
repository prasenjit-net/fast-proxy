use std::collections::HashSet;

use fast_proxy_routing::Snapshot;
use fast_proxy_telemetry::{MaintenanceReading, NO_ROUTE};

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

/// The maintenance scopes the snapshot has on, as the `fastproxy_maintenance_active`
/// gauge reports them. Labels are configuration names: `_none` where a scope
/// has no subsystem or route.
pub(crate) fn maintenance_readings(snapshot: &Snapshot) -> Vec<MaintenanceReading> {
    let mut readings = Vec::new();
    if let Some(scope) = &snapshot.maintenance {
        readings.push(MaintenanceReading {
            scope: scope.scope.label(),
            subsystem: NO_ROUTE.to_string(),
            route: NO_ROUTE.to_string(),
        });
    }
    for subsystem in snapshot.subsystems.values() {
        let name = subsystem.name.as_str().to_string();
        if let Some(scope) = &subsystem.maintenance {
            readings.push(MaintenanceReading {
                scope: scope.scope.label(),
                subsystem: name.clone(),
                route: NO_ROUTE.to_string(),
            });
        }
        for route in subsystem.routes.iter() {
            if let Some(scope) = &route.maintenance {
                readings.push(MaintenanceReading {
                    scope: scope.scope.label(),
                    subsystem: name.clone(),
                    route: route.prefix.clone(),
                });
            }
        }
    }
    readings
}
