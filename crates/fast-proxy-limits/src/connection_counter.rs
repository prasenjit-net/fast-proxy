//! Counts open client connections, in total and per peer address, and refuses a
//! new one over either cap (ADR 0029). It is checked when a connection is
//! accepted, before any TLS handshake, so a refused peer costs almost nothing.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

const SHARDS: usize = 16;

/// Which cap refused a connection; a fixed set, so it can be a metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionRefusal {
    /// The process holds `max_connections` already.
    Total,
    /// This peer holds `max_connections_per_client` already.
    PerClient,
}

impl ConnectionRefusal {
    pub fn label(self) -> &'static str {
        match self {
            Self::Total => "max_connections",
            Self::PerClient => "max_connections_per_client",
        }
    }
}

/// The caps; 0 for `per_client` turns the per-peer check off (behind a load
/// balancer every peer is the balancer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionCaps {
    pub total: usize,
    pub per_client: usize,
}

/// Open connections, in total and by peer.
#[derive(Debug)]
pub struct ConnectionCounter {
    caps: ConnectionCaps,
    total: AtomicUsize,
    by_peer: Box<[Mutex<HashMap<IpAddr, usize>>]>,
}

impl ConnectionCounter {
    pub fn new(caps: ConnectionCaps) -> Arc<Self> {
        Arc::new(Self {
            caps,
            total: AtomicUsize::new(0),
            by_peer: (0..SHARDS).map(|_| Mutex::default()).collect(),
        })
    }

    /// Counts a new connection from `peer`, or says which cap it would pass.
    /// The connection stays counted until the permit drops.
    pub fn try_open(self: &Arc<Self>, peer: IpAddr) -> Result<ConnectionPermit, ConnectionRefusal> {
        self.total
            .try_update(Ordering::AcqRel, Ordering::Acquire, |open| {
                (open < self.caps.total).then_some(open + 1)
            })
            .map_err(|_| ConnectionRefusal::Total)?;
        if self.caps.per_client > 0 {
            let mut peers = self.shard(peer);
            let open = peers.entry(peer).or_insert(0);
            if *open >= self.caps.per_client {
                drop(peers);
                self.total.fetch_sub(1, Ordering::AcqRel);
                return Err(ConnectionRefusal::PerClient);
            }
            *open += 1;
        }
        Ok(ConnectionPermit {
            counter: Arc::clone(self),
            peer,
        })
    }

    /// Connections open now.
    pub fn open(&self) -> usize {
        self.total.load(Ordering::Acquire)
    }

    /// Connections `peer` holds now.
    pub fn open_from(&self, peer: IpAddr) -> usize {
        self.shard(peer).get(&peer).copied().unwrap_or(0)
    }

    fn shard(&self, peer: IpAddr) -> std::sync::MutexGuard<'_, HashMap<IpAddr, usize>> {
        let index = match peer {
            IpAddr::V4(v4) => usize::from(v4.octets()[3]),
            IpAddr::V6(v6) => usize::from(v6.octets()[15]),
        };
        self.by_peer[index % SHARDS]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self, peer: IpAddr) {
        if self.caps.per_client > 0 {
            let mut peers = self.shard(peer);
            if let Some(open) = peers.get_mut(&peer) {
                *open -= 1;
                if *open == 0 {
                    peers.remove(&peer);
                }
            }
        }
        self.total.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One counted connection; dropping it gives its place back.
#[derive(Debug)]
pub struct ConnectionPermit {
    counter: Arc<ConnectionCounter>,
    peer: IpAddr,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.counter.close(self.peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn counter(total: usize, per_client: usize) -> Arc<ConnectionCounter> {
        ConnectionCounter::new(ConnectionCaps { total, per_client })
    }

    #[test]
    fn one_peer_is_capped_while_others_still_connect() {
        let counter = counter(100, 2);
        let a = ip("192.0.2.1");
        let held: Vec<_> = (0..2).map(|_| counter.try_open(a).unwrap()).collect();
        assert_eq!(
            counter.try_open(a).unwrap_err(),
            ConnectionRefusal::PerClient
        );
        assert!(counter.try_open(ip("192.0.2.2")).is_ok());
        assert_eq!(counter.open_from(a), 2);
        drop(held);
        assert_eq!(
            counter.open_from(a),
            0,
            "the entry goes with the last connection"
        );
        assert!(counter.try_open(a).is_ok());
    }

    #[test]
    fn the_total_cap_holds_across_peers_and_frees_on_close() {
        let counter = counter(3, 0);
        let held: Vec<_> = (1..=3)
            .map(|n| counter.try_open(ip(&format!("192.0.2.{n}"))).unwrap())
            .collect();
        assert_eq!(
            counter.try_open(ip("192.0.2.9")).unwrap_err(),
            ConnectionRefusal::Total
        );
        assert_eq!(counter.open(), 3);
        drop(held);
        assert_eq!(counter.open(), 0);
    }

    #[test]
    fn a_per_client_refusal_does_not_leak_a_place_in_the_total() {
        let counter = counter(10, 1);
        let a = ip("192.0.2.1");
        let _held = counter.try_open(a).unwrap();
        for _ in 0..50 {
            assert!(counter.try_open(a).is_err());
        }
        assert_eq!(counter.open(), 1);
    }

    #[test]
    fn concurrent_opens_never_pass_either_cap() {
        let counter = counter(8, 3);
        let peers = ["192.0.2.1", "192.0.2.2", "192.0.2.3", "192.0.2.4"];
        let workers: Vec<_> = (0..16)
            .map(|n| {
                let counter = counter.clone();
                let peer = ip(peers[n % peers.len()]);
                std::thread::spawn(move || {
                    for _ in 0..2_000 {
                        if let Ok(permit) = counter.try_open(peer) {
                            assert!(counter.open() <= 8);
                            assert!(counter.open_from(peer) <= 3);
                            drop(permit);
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(counter.open(), 0);
    }

    #[test]
    fn refusals_have_fixed_labels() {
        assert_eq!(ConnectionRefusal::Total.label(), "max_connections");
        assert_eq!(
            ConnectionRefusal::PerClient.label(),
            "max_connections_per_client"
        );
    }
}
