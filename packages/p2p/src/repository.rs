//! The peers and the pending peers, keyed by IP in insertion order, as the reference's maps keep
//! them: setting a known IP keeps its place, forgetting one and setting it again moves it last.
//!
//! The repository is shared by the communicator, the acceptance and the monitor; each call locks
//! it briefly and never across an `.await`.

use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use indexmap::IndexMap;

use crate::peer::SharedPeer;

/// The two tables.
#[derive(Debug, Default)]
struct Tables {
    peers: IndexMap<IpAddr, SharedPeer>,
    pending: IndexMap<IpAddr, SharedPeer>,
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

    /// Forget the peer at `ip`, if any.
    pub fn forget(&self, ip: IpAddr) {
        self.with(|tables| tables.peers.shift_remove(&ip));
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
