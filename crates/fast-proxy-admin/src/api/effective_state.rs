use fast_proxy_balancing::EndpointRuntime;
use fast_proxy_model::EndpointState;

/// Whether an endpoint is taking new requests, and if not, why: the operator's
/// decision first, then what health checking and ejection say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effective {
    Healthy,
    /// Active health checks fail it.
    FailedChecks,
    /// Repeated failures took it out of rotation for `remaining_ms` more.
    Ejected {
        remaining_ms: u64,
    },
    Draining,
    Disabled,
}

impl Effective {
    /// Classifies an endpoint as of `now_ms` on the process clock.
    pub fn classify(state: EndpointState, runtime: &EndpointRuntime, now_ms: u64) -> Self {
        match state {
            EndpointState::Draining => Self::Draining,
            EndpointState::Disabled => Self::Disabled,
            EndpointState::Enabled if !runtime.is_active_healthy() => Self::FailedChecks,
            EndpointState::Enabled => match runtime.status(now_ms).ejected_until_ms {
                Some(until) => Self::Ejected {
                    remaining_ms: until.saturating_sub(now_ms),
                },
                None => Self::Healthy,
            },
        }
    }

    /// The API's name for the state.
    pub fn name(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::FailedChecks => "unhealthy",
            Self::Ejected { .. } => "ejected",
            Self::Draining => "draining",
            Self::Disabled => "disabled",
        }
    }

    /// A sentence for people.
    pub fn reason(self) -> String {
        match self {
            Self::Healthy => "taking requests".to_string(),
            Self::FailedChecks => "failed health checks".to_string(),
            Self::Ejected { remaining_ms } => {
                format!(
                    "ejected after repeated failures, back in {} s",
                    remaining_ms.div_ceil(1000)
                )
            }
            Self::Draining => "draining: no new requests".to_string(),
            Self::Disabled => "disabled: no requests".to_string(),
        }
    }

    /// Why the endpoint is out of rotation, for the summary; `None` if it is in.
    pub fn summary_reason(self) -> Option<&'static str> {
        match self {
            Self::Healthy => None,
            Self::FailedChecks => Some("failed health checks"),
            Self::Ejected { .. } => Some("ejected after repeated failures"),
            Self::Draining => Some("draining"),
            Self::Disabled => Some("disabled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_operators_decision_comes_before_health() {
        let runtime = EndpointRuntime::new();
        runtime.record_probe(false, 1, 1);
        assert_eq!(
            Effective::classify(EndpointState::Draining, &runtime, 0),
            Effective::Draining
        );
        assert_eq!(
            Effective::classify(EndpointState::Disabled, &runtime, 0),
            Effective::Disabled
        );
        assert_eq!(
            Effective::classify(EndpointState::Enabled, &runtime, 0),
            Effective::FailedChecks
        );
    }

    #[test]
    fn a_healthy_endpoint_is_healthy_and_an_ejected_one_says_for_how_long() {
        let runtime = EndpointRuntime::new();
        assert_eq!(
            Effective::classify(EndpointState::Enabled, &runtime, 0),
            Effective::Healthy
        );
        let group = [fast_proxy_balancing::Candidate {
            runtime: std::sync::Arc::new(EndpointRuntime::new()),
            weight: 1,
            admin_enabled: true,
        }];
        let policy = fast_proxy_model::PassiveHealth {
            consecutive_failures: 1,
            max_ejection_percent: 100,
            ..fast_proxy_model::PassiveHealth::default()
        };
        fast_proxy_balancing::record_outcome(
            &group,
            0,
            fast_proxy_balancing::Outcome::Failure,
            &policy,
            1_000,
        );
        let ejected = Effective::classify(EndpointState::Enabled, &group[0].runtime, 1_000);
        assert_eq!(
            ejected,
            Effective::Ejected {
                remaining_ms: 10_000
            }
        );
        assert_eq!(ejected.name(), "ejected");
        assert!(
            ejected.reason().contains("back in 10 s"),
            "{}",
            ejected.reason()
        );
    }

    #[test]
    fn every_state_has_a_name_a_reason_and_a_summary() {
        let all = [
            Effective::Healthy,
            Effective::FailedChecks,
            Effective::Ejected { remaining_ms: 1 },
            Effective::Draining,
            Effective::Disabled,
        ];
        let names: Vec<_> = all.iter().map(|e| e.name()).collect();
        assert_eq!(
            names,
            ["healthy", "unhealthy", "ejected", "draining", "disabled"]
        );
        assert!(all.iter().all(|e| !e.reason().is_empty()));
        assert_eq!(Effective::Healthy.summary_reason(), None);
        assert!(all[1..].iter().all(|e| e.summary_reason().is_some()));
    }
}
