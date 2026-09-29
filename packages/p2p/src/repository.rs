//! The peers and the pending peers, keyed by IP in insertion order, as the reference's maps keep
//! them: setting a known IP keeps its place, forgetting one and setting it again moves it last.
//!
//! The repository also keeps the latest protocol violation of each address it does not hold (a
//! forgotten peer, a new peer whose acceptance failed, a caller of the server that is no peer),
//! for the rest of its window, so that a violating address is not accepted while a violation is
//! in its window ([`PeerRepository::has_violation`]). It keeps at most [`KEPT_VIOLATIONS`]
//! addresses, letting the oldest violation go first, and lets each go once its window has passed,
//! so no caller can grow the table or the cost of a probe.
//!
//! The repository is shared by the communicator, the acceptance and the monitor; each call locks
//! it briefly and never across an `.await`, and never while it holds a peer's lock.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use indexmap::IndexMap;

use crate::peer::{FAULT_WINDOW_MS, SharedPeer};

/// How many addresses the repository keeps a violation of when it holds no peer at them; past
/// it, the address whose violation is the oldest is let go first.
pub const KEPT_VIOLATIONS: usize = 16_384;

/// The tables.
#[derive(Debug, Default)]
struct Tables {
    peers: IndexMap<IpAddr, SharedPeer>,
    pending: IndexMap<IpAddr, SharedPeer>,
    violations: Violations,
}

/// The latest violation time of each address the repository holds no peer at.
#[derive(Debug, Default)]
struct Violations {
    latest: HashMap<IpAddr, i64>,
    /// The times as they were noted; one whose address was noted again later is stale.
    order: VecDeque<(IpAddr, i64)>,
}

impl Violations {
    /// Note a violation of `ip` at `time_ms`, then let the expired ones go and keep the bound.
    fn note(&mut self, ip: IpAddr, time_ms: i64) {
        if self.latest.get(&ip).is_some_and(|kept| *kept >= time_ms) {
            return;
        }
        self.latest.insert(ip, time_ms);
        self.order.push_back((ip, time_ms));
        self.expire(time_ms);
        let notes = KEPT_VIOLATIONS.saturating_mul(2);
        while self.latest.len() > KEPT_VIOLATIONS || self.order.len() > notes {
            self.pop();
        }
    }

    /// Whether `ip` has a violation in its window at `now_ms`.
    fn has(&mut self, ip: IpAddr, now_ms: i64) -> bool {
        self.expire(now_ms);
        (self.latest.get(&ip)).is_some_and(|time| in_window(*time, now_ms))
    }

    fn expire(&mut self, now_ms: i64) {
        while (self.order.front()).is_some_and(|(_, time)| !in_window(*time, now_ms)) {
            self.pop();
        }
    }

    /// Let the oldest note go, and its address with it unless it was noted again later.
    fn pop(&mut self) {
        if let Some((ip, time)) = self.order.pop_front()
            && self.latest.get(&ip) == Some(&time)
        {
            self.latest.remove(&ip);
        }
    }
}

fn in_window(time_ms: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(time_ms) <= FAULT_WINDOW_MS
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

    /// Forget the peer at `ip`, if any. Its latest protocol violation is kept for the rest of its
    /// window.
    pub fn forget(&self, ip: IpAddr) {
        if let Some(peer) = self.with(|tables| tables.peers.shift_remove(&ip)) {
            self.keep_violations(&peer);
        }
    }

    /// Keep the latest protocol violation of `peer`, which the repository does not hold (a new
    /// peer whose acceptance failed), for the rest of its window.
    pub fn keep_violations(&self, peer: &SharedPeer) {
        let latest = peer.with(|peer| peer.violations.iter().max().copied());
        if let Some(time_ms) = latest {
            self.note_violation(peer.ip(), time_ms);
        }
    }

    /// Keep a protocol violation at `time_ms` of `ip`, an address the repository holds no peer
    /// at, for the rest of its window.
    pub fn note_violation(&self, ip: IpAddr, time_ms: i64) {
        self.with(|tables| tables.violations.note(ip, time_ms));
    }

    /// Whether the IP has a protocol violation in its window at `now_ms`: the peer held at `ip`,
    /// or a kept violation of `ip`. The kept violations outside their window are let go.
    pub fn has_violation(&self, ip: IpAddr, now_ms: i64) -> bool {
        if self
            .get(ip)
            .is_some_and(|peer| peer.with(|peer| peer.has_violation(now_ms)))
        {
            return true;
        }
        self.with(|tables| tables.violations.has(ip, now_ms))
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

    /// Mark `peer` pending under its IP unless a peer at that IP is pending already, in one step;
    /// whether it was marked.
    pub fn try_set_pending(&self, peer: SharedPeer) -> bool {
        self.with(|tables| match tables.pending.entry(peer.ip()) {
            indexmap::map::Entry::Occupied(_) => false,
            indexmap::map::Entry::Vacant(entry) => {
                entry.insert(peer);
                true
            }
        })
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
    fn the_violations_of_addresses_that_are_no_peer_are_bounded() {
        let repository = PeerRepository::new();
        let address =
            |n: u32| IpAddr::from([0x2001, 0xdb8, 0, 0, 0, 0, (n >> 16) as u16, n as u16]);
        for n in 0..100_000 {
            let refused = SharedPeer::new(Peer::new(address(n), 4002));
            refused.with(|peer| peer.add_violation(1_000 + i64::from(n)));
            repository.keep_violations(&refused);
        }
        let kept = repository.with(|tables| tables.violations.latest.len());
        assert_eq!(kept, KEPT_VIOLATIONS);
        // The latest are kept, in their window; the oldest were let go first.
        assert!(repository.has_violation(address(99_999), 101_000));
        assert!(!repository.has_violation(address(0), 101_000));
        // An address noted again keeps one entry, with its latest time.
        let again = address(99_998);
        repository.note_violation(again, 200_000);
        repository.note_violation(again, 150_000);
        assert_eq!(
            repository.with(|tables| tables.violations.latest.len()),
            KEPT_VIOLATIONS
        );
        assert!(repository.has_violation(again, 800_000));
        assert!(!repository.has_violation(address(99_999), 800_000));
        // Past every window, the table is empty.
        assert!(!repository.has_violation(again, 800_001));
        assert_eq!(repository.with(|tables| tables.violations.order.len()), 0);
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
        assert!(!repository.try_set_pending(peer(7, 4003)));
        assert_eq!(repository.get_pending(pending.ip()).unwrap().port(), 4002);
        repository.forget_pending(pending.ip());
        assert!(!repository.has_pending(pending.ip()));
        assert!(repository.try_set_pending(peer(7, 4003)));
    }
}
