use std::collections::HashMap;
use std::sync::Arc;

use fast_proxy_model::{EndpointAddr, EndpointId};

use crate::EndpointRuntime;

/// The runtime state of every endpoint in the live configuration, keyed by
/// endpoint id **and** address: an edit that moves an endpoint to a new
/// address starts it fresh, while unrelated edits keep its state.
#[derive(Debug, Default, Clone)]
pub struct RuntimeRegistry {
    entries: HashMap<(EndpointId, EndpointAddr), Arc<EndpointRuntime>>,
}

impl RuntimeRegistry {
    /// An empty registry (nothing carried over).
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry for a new configuration listing `wanted` endpoints:
    /// existing runtimes are reused, new ones are created, and endpoints no
    /// longer listed are dropped. The old registry is left untouched, so a
    /// compile that fails changes nothing.
    pub fn rebuilt_for<'a>(
        &self,
        wanted: impl IntoIterator<Item = (EndpointId, &'a EndpointAddr)>,
    ) -> Self {
        let entries = wanted
            .into_iter()
            .map(|(id, addr)| {
                let key = (id, addr.clone());
                let runtime = self.entries.get(&key).cloned().unwrap_or_default();
                (key, runtime)
            })
            .collect();
        Self { entries }
    }

    /// The runtime for an endpoint, if it is in the registry.
    pub fn get(&self, id: EndpointId, addr: &EndpointAddr) -> Option<Arc<EndpointRuntime>> {
        self.entries.get(&(id, addr.clone())).cloned()
    }

    /// How many endpoints have state.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no endpoint has state.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> EndpointAddr {
        EndpointAddr::parse(text).unwrap()
    }

    #[test]
    fn unchanged_endpoints_keep_their_runtime() {
        let (a, b) = (addr("a:1"), addr("b:1"));
        let first = RuntimeRegistry::new().rebuilt_for([(EndpointId(1), &a), (EndpointId(2), &b)]);
        let kept = first.get(EndpointId(1), &a).unwrap();
        let second = first.rebuilt_for([(EndpointId(1), &a)]);
        assert!(Arc::ptr_eq(&kept, &second.get(EndpointId(1), &a).unwrap()));
        assert_eq!(second.len(), 1);
        assert!(
            second.get(EndpointId(2), &b).is_none(),
            "removed endpoints are dropped"
        );
    }

    #[test]
    fn a_changed_address_or_id_starts_fresh() {
        let (a, moved) = (addr("a:1"), addr("a:2"));
        let first = RuntimeRegistry::new().rebuilt_for([(EndpointId(1), &a)]);
        let old = first.get(EndpointId(1), &a).unwrap();
        let second = first.rebuilt_for([(EndpointId(1), &moved)]);
        assert!(!Arc::ptr_eq(
            &old,
            &second.get(EndpointId(1), &moved).unwrap()
        ));
        let third = first.rebuilt_for([(EndpointId(9), &a)]);
        assert!(!Arc::ptr_eq(&old, &third.get(EndpointId(9), &a).unwrap()));
    }

    #[test]
    fn rebuilding_does_not_disturb_the_original() {
        let a = addr("a:1");
        let first = RuntimeRegistry::new().rebuilt_for([(EndpointId(1), &a)]);
        let _ = first.rebuilt_for([]);
        assert!(!first.is_empty());
        assert!(first.get(EndpointId(1), &a).is_some());
    }
}
