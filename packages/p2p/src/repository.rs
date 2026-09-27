//! The peers and the pending peers, keyed by IP in insertion order, as the reference's maps keep
//! them: setting a known IP keeps its place, forgetting one and setting it again moves it last.
//!
//! The repository also keeps the protocol violations of the peers it no longer holds, for the
//! rest of their window, so that a violating peer is not accepted again while a violation is in
//! its window ([`PeerRepository::has_violation`]).
//!
//! The repository is shared by the communicator, the acceptance and the monitor; each call locks
//! it briefly and never across an `.await`, and never while it holds a peer's lock.

use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use indexmap::IndexMap;

use crate::peer::{FAULT_WINDOW_MS, SharedPeer};

/// The tables.
#[derive(Debug, Default)]
struct Tables {
    peers: IndexMap<IpAddr, SharedPeer>,
    pending: IndexMap<IpAddr, SharedPeer>,
    /// The violation times of peers the repository no longer holds.
    violations: IndexMap<IpAddr, Vec<i64>>,
}

/// The peers and the pending peers.
#[derive(Debug, Default)]
pub struct PeerRepository {
    tables: Mutex<Tables>,
}

impl PeerRepository {
    /// An empty repository.
    pub fn new() -> PeerRepository {
        PeerRepository::default()
    }

    fn with<R>(&self, read: impl FnOnce(&mut Tables) -> R) -> R {
        let mut tables = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        read(&mut tables)
    }

    /// The peers, in insertion order.
    pub fn peers(&self) -> Vec<SharedPeer> {
        self.with(|tables| tables.peers.values().cloned().collect())
    }

    /// How many peers there are.
    pub fn len(&self) -> usize {
        self.with(|tables| tables.peers.len())
    }

    /// Whether there is no peer.
    pub fn is_empty(&self) -> bool {
        self.with(|tables| tables.peers.is_empty())
    }

    /// The peer at `ip`, if any.
    pub fn get(&self, ip: IpAddr) -> Option<SharedPeer> {
        self.with(|tables| tables.peers.get(&ip).cloned())
    }

    /// Store `peer` under its IP: in the place of a peer already there, else last.
    pub fn set(&self, peer: SharedPeer) {
        self.with(|tables| tables.peers.insert(peer.ip(), peer));
    }

    /// Forget the peer at `ip`, if any. Its protocol violations are kept for the rest of their
    /// window.
    pub fn forget(&self, ip: IpAddr) {
        if let Some(peer) = self.with(|tables| tables.peers.shift_remove(&ip)) {
            self.keep_violations(&peer);
        }
    }

    /// Keep the protocol violations of `peer`, which the repository does not hold (a new peer
    /// whose acceptance failed), for the rest of their window.
    pub fn keep_violations(&self, peer: &SharedPeer) {
        let times = peer.with(|peer| peer.violations.clone());
        if !times.is_empty() {
            self.with(|tables| {
                let kept = tables.violations.entry(peer.ip()).or_default();
                for time in times {
                    if !kept.contains(&time) {
                        kept.push(time);
                    }
                }
            });
        }
    }

    /// Whether the IP has a protocol violation in its window at `now_ms`: the peer held at `ip`,
    /// or a peer at `ip` that was forgotten. The kept violations outside their window are
    /// forgotten.
    pub fn has_violation(&self, ip: IpAddr, now_ms: i64) -> bool {
        if self
            .get(ip)
            .is_some_and(|peer| peer.with(|peer| peer.has_violation(now_ms)))
        {
            return true;
        }
        self.with(|tables| {
            tables.violations.retain(|_, times| {
                times.retain(|time| now_ms.saturating_sub(*time) <= FAULT_WINDOW_MS);
                !times.is_empty()
            });
            tables.violations.contains_key(&ip)
        })
    }

    /// Whether there is a peer at `ip`.
    pub fn has(&self, ip: IpAddr) -> bool {
        self.with(|tables| tables.peers.contains_key(&ip))
    }

    /// The pending peers, in insertion order.
    pub fn pending(&self) -> Vec<SharedPeer> {
        self.with(|tables| tables.pending.values().cloned().collect())
    }

    /// The pending peer at `ip`, if any.
    pub fn get_pending(&self, ip: IpAddr) -> Option<SharedPeer> {
        self.with(|tables| tables.pending.get(&ip).cloned())
    }

    /// Mark `peer` pending under its IP.
    pub fn set_pending(&self, peer: SharedPeer) {
        self.with(|tables| tables.pending.insert(peer.ip(), peer));
    }

    /// Forget the pending peer at `ip`, if any.
    pub fn forget_pending(&self, ip: IpAddr) {
        self.with(|tables| tables.pending.shift_remove(&ip));
    }

    /// Whether a peer at `ip` is pending.
    pub fn has_pending(&self, ip: IpAddr) -> bool {
        self.with(|tables| tables.pending.contains_key(&ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::Peer;

    fn peer(last: u8, port: u16) -> SharedPeer {
        SharedPeer::new(Peer::new(IpAddr::from([192, 0, 2, last]), port))
    }

    fn order(repository: &PeerRepository) -> Vec<(u8, u16)> {
        repository
            .peers()
            .iter()
            .map(|peer| match peer.ip() {
                IpAddr::V4(ip) => (ip.octets()[3], peer.port()),
                IpAddr::V6(_) => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn peers_keep_insertion_order_by_ip() {
        let repository = PeerRepository::new();
        assert!(repository.is_empty());
        repository.set(peer(1, 4002));
        repository.set(peer(2, 4002));
        repository.set(peer(3, 4002));
        // A known IP keeps its place and takes the new peer.
        repository.set(peer(1, 4003));
        assert_eq!(order(&repository), [(1, 4003), (2, 4002), (3, 4002)]);
        // Forgotten and set again, it goes last.
        repository.forget(IpAddr::from([192, 0, 2, 2]));
        repository.set(peer(2, 4004));
        assert_eq!(order(&repository), [(1, 4003), (3, 4002), (2, 4004)]);
        assert_eq!(repository.len(), 3);
        assert!(repository.has(IpAddr::from([192, 0, 2, 3])));
        assert!(repository.get(IpAddr::from([192, 0, 2, 9])).is_none());
    }

    #[test]
    fn violations_outlive_a_forgotten_peer_for_their_window() {
        let repository = PeerRepository::new();
        let violator = peer(4, 4002);
        let ip = violator.ip();
        violator.with(|peer| peer.add_violation(1_000));
        repository.set(violator);
        assert!(repository.has_violation(ip, 1_000));
        repository.forget(ip);
        assert!(!repository.has(ip));
        assert!(repository.has_violation(ip, 601_000));
        assert!(!repository.has_violation(ip, 601_001));
        // A peer that was never stored, and one with infractions only.
        let refused = peer(5, 4002);
        refused.with(|peer| peer.add_violation(2_000));
        repository.keep_violations(&refused);
        assert!(repository.has_violation(refused.ip(), 2_000));
        let slow = peer(6, 4002);
        slow.with(|peer| peer.add_infraction(2_000));
        repository.set(slow.clone());
        repository.forget(slow.ip());
        assert!(!repository.has_violation(slow.ip(), 2_000));
    }

    #[test]
    fn pending_peers_are_a_table_of_their_own() {
        let repository = PeerRepository::new();
        let pending = peer(7, 4002);
        repository.set_pending(pending.clone());
        assert!(repository.has_pending(pending.ip()));
        assert!(!repository.has(pending.ip()));
        assert!(repository.get_pending(pending.ip()).unwrap().same(&pending));
        assert_eq!(repository.pending().len(), 1);
        repository.forget_pending(pending.ip());
        assert!(!repository.has_pending(pending.ip()));
    }
}
