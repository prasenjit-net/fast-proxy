use fast_proxy_model::{Config, SubsystemConfig};
use serde::Serialize;

use super::detail::{routes_in_match_order, RouteDto};
use super::https_dto::HttpsDto;
use super::upstream_dto::{upstream_dto, UpstreamDto};

/// A whole configuration as shown when viewing a revision.
#[derive(Debug, Serialize)]
pub struct ConfigView {
    pub subsystems: Vec<SubsystemView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsystemView {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub description: String,
    /// The HTTPS settings the revision held (the certificate itself is not part
    /// of a revision, only its id). `effective` says whether they would act now.
    pub https: HttpsDto,
    pub hosts: Vec<String>,
    pub upstreams: Vec<UpstreamDto>,
    pub routes: Vec<RouteDto>,
}

impl ConfigView {
    pub fn from_config(config: &Config, tls_enabled: bool) -> Self {
        Self {
            subsystems: config
                .subsystems
                .iter()
                .map(|subsystem| subsystem_view(subsystem, tls_enabled))
                .collect(),
        }
    }
}

fn subsystem_view(subsystem: &SubsystemConfig, tls_enabled: bool) -> SubsystemView {
    SubsystemView {
        id: subsystem.id.0,
        name: subsystem.name.as_str().to_string(),
        enabled: subsystem.enabled,
        description: subsystem.description.clone(),
        https: HttpsDto::new(&subsystem.https, tls_enabled, &subsystem.hosts, None),
        hosts: subsystem
            .hosts
            .iter()
            .map(|h| h.to_pattern_string())
            .collect(),
        upstreams: subsystem.upstream_groups.iter().map(upstream_dto).collect(),
        routes: routes_in_match_order(subsystem),
    }
}
