//! Entry points for the fuzz targets in `fuzz/` (M7 F1, ADR 0032). Compiled only
//! with the `fuzzing` feature, so none of this is public API: every admin write
//! body goes through serde and then the same validation its handler runs.

use serde::de::DeserializeOwned;

use super::ca_bundles::CaBundleRequest;
use super::certificates::{ReplaceRequest, SelfSignedRequest, UploadRequest};
use super::endpoints::StateRequest;
use super::hosts::HostRequest;
use super::input::FieldErrors;
use super::maintenance::{settings_from, MaintenanceRequest};
use super::rate_limits::RateLimitRequest;
use super::resolve::ResolveRequest;
use super::routes::RouteRequest;
use super::sampling::{OverrideRequest, SettingsRequest};
use super::subsystems::SubsystemRequest;
use super::tokens::CreateTokenRequest;
use super::upstream_dto::{parse_endpoint, EndpointRequest, UpstreamRequest};

/// Every write body the admin API takes, in a fixed order.
pub const BODIES: [&str; 14] = [
    "subsystem",
    "route",
    "upstream_create",
    "upstream_update",
    "endpoint",
    "rate_limit",
    "maintenance",
    "host",
    "endpoint_state",
    "sampling_settings",
    "sampling_override",
    "token",
    "certificate",
    "resolve",
];

/// Parses `json` as the body `BODIES[kind % BODIES.len()]` and validates it the
/// way its handler does. Returns whether it was accepted; a panic is the bug.
pub fn accept_body(kind: u8, json: &[u8]) -> bool {
    match usize::from(kind) % BODIES.len() {
        0 => with::<SubsystemRequest>(json, |body| body.into_edit().is_ok()),
        1 => with::<RouteRequest>(json, |body| body.into_fields().is_ok()),
        2 => with::<UpstreamRequest>(json, |body| body.parse(true).is_ok()),
        3 => with::<UpstreamRequest>(json, |body| body.parse(false).is_ok()),
        4 => with::<EndpointRequest>(json, |body| {
            let mut errors = FieldErrors::default();
            parse_endpoint(&body, "", &mut errors).is_some() && errors.finish().is_ok()
        }),
        5 => with::<RateLimitRequest>(json, |body| body.into_policy().is_ok()),
        6 => with::<MaintenanceRequest>(json, |body| settings_from(body).is_ok()),
        7 => parses::<HostRequest>(json),
        8 => parses::<StateRequest>(json),
        9 => parses::<SettingsRequest>(json),
        10 => parses::<OverrideRequest>(json),
        11 => parses::<CreateTokenRequest>(json),
        12 => {
            parses::<UploadRequest>(json)
                | parses::<ReplaceRequest>(json)
                | parses::<SelfSignedRequest>(json)
                | parses::<CaBundleRequest>(json)
        }
        _ => parses::<ResolveRequest>(json),
    }
}

fn with<T: DeserializeOwned>(json: &[u8], validate: impl FnOnce(T) -> bool) -> bool {
    serde_json::from_slice::<T>(json).is_ok_and(validate)
}

fn parses<T: DeserializeOwned>(json: &[u8]) -> bool {
    serde_json::from_slice::<T>(json).is_ok()
}
