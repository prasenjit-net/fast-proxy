//! A broadcast of endpoint state changes (health, ejection) for the admin
//! plane's live view. Publishing never blocks and never fails when nobody
//! listens.

use tokio::sync::broadcast;

/// How many events a slow subscriber may lag behind before it misses some.
const CAPACITY: usize = 256;

/// What changed about an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointChange {
    /// Active probes marked the endpoint unhealthy.
    Unhealthy { reason: String },
    /// Active probes marked the endpoint healthy again.
    Healthy,
    /// Repeated failures took the endpoint out of rotation until `until_ms`
    /// on the process clock; this was its `times`-th ejection in a row.
    Ejected { until_ms: u64, times: u32 },
    /// A trial request succeeded and the endpoint is back in rotation.
    Recovered,
}

/// One endpoint's state change, named by config names only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointEvent {
    pub subsystem: String,
    pub upstream: String,
    /// The endpoint's `host:port`.
    pub endpoint: String,
    pub change: EndpointChange,
}

/// The shared bus. Cloning gives another handle on the same channel.
#[derive(Debug, Clone)]
pub struct EndpointEvents {
    sender: broadcast::Sender<EndpointEvent>,
}

impl EndpointEvents {
    pub fn new() -> Self {
        Self {
            sender: broadcast::channel(CAPACITY).0,
        }
    }

    /// Announces a change to every current subscriber.
    pub fn publish(&self, event: EndpointEvent) {
        // send() only fails when nobody is subscribed, which is fine.
        let _ = self.sender.send(event);
    }

    /// Receives every change published from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<EndpointEvent> {
        self.sender.subscribe()
    }
}

impl Default for EndpointEvents {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(change: EndpointChange) -> EndpointEvent {
        EndpointEvent {
            subsystem: "shop".into(),
            upstream: "backend".into(),
            endpoint: "a:1".into(),
            change,
        }
    }

    #[tokio::test]
    async fn every_subscriber_sees_published_events_in_order() {
        let bus = EndpointEvents::new();
        let (mut first, mut second) = (bus.subscribe(), bus.clone().subscribe());
        bus.publish(event(EndpointChange::Healthy));
        bus.publish(event(EndpointChange::Recovered));
        for receiver in [&mut first, &mut second] {
            assert_eq!(
                receiver.recv().await.unwrap().change,
                EndpointChange::Healthy
            );
            assert_eq!(
                receiver.recv().await.unwrap().change,
                EndpointChange::Recovered
            );
        }
    }

    #[test]
    fn publishing_without_subscribers_is_harmless() {
        EndpointEvents::default().publish(event(EndpointChange::Healthy));
    }
}
