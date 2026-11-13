//! Resolves, for every subsystem and route, the sampling policy: the global
//! settings under the subsystem's override under the route's. Checked against
//! the configuration like maintenance is, so an override can never dangle.

use std::collections::HashMap;
use std::sync::Arc;

use fast_proxy_model::{
    Config, RouteId, SamplingOverride, SamplingSettings, SamplingTarget, SubsystemId,
};
use fast_proxy_sampling::SamplingPolicy;

use crate::{CompileError, SamplingInputs};

pub(crate) struct SamplingTable<'a> {
    settings: &'a SamplingSettings,
    subsystems: HashMap<SubsystemId, &'a SamplingOverride>,
    routes: HashMap<RouteId, &'a SamplingOverride>,
}

impl<'a> SamplingTable<'a> {
    pub(crate) fn build(config: &Config, inputs: &'a SamplingInputs) -> Result<Self, CompileError> {
        let mut table = Self {
            settings: &inputs.settings,
            subsystems: HashMap::new(),
            routes: HashMap::new(),
        };
        for (target, settings) in &inputs.overrides {
            match target {
                SamplingTarget::Subsystem(id) => {
                    if !config.subsystems.iter().any(|s| s.id == *id) {
                        return Err(CompileError::UnknownSamplingSubsystem { id: id.0 });
                    }
                    table.subsystems.insert(*id, settings);
                }
                SamplingTarget::Route(id) => {
                    let exists = config
                        .subsystems
                        .iter()
                        .flat_map(|s| &s.routes)
                        .any(|route| route.id == *id);
                    if !exists {
                        return Err(CompileError::UnknownSamplingRoute { id: id.0 });
                    }
                    table.routes.insert(*id, settings);
                }
            }
        }
        Ok(table)
    }

    /// The policy for requests of a subsystem that no route took (and, with no
    /// subsystem, for the global answer).
    pub(crate) fn subsystem_policy(&self, id: SubsystemId) -> Arc<SamplingPolicy> {
        Arc::new(SamplingPolicy::resolve(
            self.settings,
            self.subsystems.get(&id).copied(),
            None,
        ))
    }

    pub(crate) fn route_policy(
        &self,
        subsystem: SubsystemId,
        route: RouteId,
    ) -> Arc<SamplingPolicy> {
        Arc::new(SamplingPolicy::resolve(
            self.settings,
            self.subsystems.get(&subsystem).copied(),
            self.routes.get(&route).copied(),
        ))
    }

    /// The policy for a request no subsystem took.
    pub(crate) fn global_policy(&self) -> Arc<SamplingPolicy> {
        Arc::new(SamplingPolicy::resolve(self.settings, None, None))
    }
}
