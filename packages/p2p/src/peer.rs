//! A peer, with the reference's fields and rules, and the handle through which tasks share it.
//!
//! The reference mutates one peer object from every task that uses it: a ping sets its state
//! while a download from the same peer counts its errors. [`SharedPeer`] keeps that: tasks hold
//! the handle and change the peer in short sections ([`SharedPeer::with`]), never across an
//! `.await`, so a slow request to a peer never holds up another request to it.
//!
//! Two records of recent faults, each a list of times with a 600 s window:
//!
//! - infractions: the reference's own, a cleanse ping still running at the cleanse's deadline;
//! - violations: protocol violations that no honest reference peer produces (a reply that fails
//!   decoding or its schema, a nes protocol error, a bogus common-block reply, a fork header that
//!   fails its checks, a downloaded block that fails decoding or verification).
//!
//! A peer with at least 3 infractions and violations together in the window is ignored by the
//! ping, as the reference ignores one with 3 infractions ([`Peer::is_ignored`]); only a peer with
//! at least one violation in the window is also no source of blocks, transactions or peers and is
//! not accepted again ([`Peer::has_violation`]).

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};

use heartwood_crypto::PublicKeyBytes;
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::messages::peer::{Plugins, StatusHeader};
use heartwood_p2p_wire::status::CheckedStatus;
use indexmap::IndexMap;

/// The window of infractions and violations, in milliseconds.
pub const FAULT_WINDOW_MS: i64 = 600_000;
/// How many infractions and violations in the window make a peer ignored.
pub const IGNORED_AT: usize = 3;
/// How long after a ping a peer counts as recently pinged, in milliseconds: dayjs's whole minutes
/// below 2.
pub const RECENTLY_PINGED_MS: i64 = 120_000;

/// The state a peer claimed in its last accepted getStatus reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerState {
    /// The height of the peer's last block.
    pub height: u32,
    /// Whether the peer may forge in the current slot.
    pub forging_allowed: bool,
    /// The peer's current slot: the wire's unsigned value, or the same 32 bits read as signed
    /// (`currentSlot >> 0`) when our own slot is negative.
    pub current_slot: i64,
    /// The header of the peer's last block, as the reply check left it (the generator's key
    /// lowercased).
    pub header: StatusHeader,
    /// The header's sum of amounts.
    pub total_amount: Amount,
    /// The header's sum of fees.
    pub total_fee: Amount,
    /// The header's reward.
    pub reward: Amount,
}

impl PeerState {
    /// The state of a checked getStatus reply; `None` only for a reply without a state, a height,
    /// a slot, a forging flag or a header, which the reply check never accepts.
    pub fn from_checked(status: &CheckedStatus) -> Option<PeerState> {
        let state = status.message.state.as_ref()?;
        Some(PeerState {
            height: state.height?,
            forging_allowed: state.forging_allowed?,
            current_slot: i64::from(state.current_slot?),
            header: state.header.clone()?,
            total_amount: status.total_amount,
            total_fee: status.total_fee,
            reward: status.reward,
        })
    }

    /// The header's height (0 when absent, which a checked reply never has).
    pub fn header_height(&self) -> u32 {
        self.header.height.unwrap_or(0)
    }

    /// The header's block id, as the reply gave it.
    pub fn header_id(&self) -> Option<&str> {
        self.header.id.as_deref()
    }
}

/// The result of a full verification of a peer's claimed chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verification {
    /// Our height when the peer was verified.
    pub my_height: u32,
    /// The peer's claimed height.
    pub their_height: u32,
    /// The height of the highest block both chains have.
    pub highest_common_height: u32,
}

impl Verification {
    /// Whether the chains fork: the highest common block is at neither chain's height.
    pub fn forked(&self) -> bool {
        self.highest_common_height != self.my_height
            && self.highest_common_height != self.their_height
    }
}

/// The result of a fast verification, which looks for no common block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastVerification {
    /// Our height when the peer was verified.
    pub my_height: u32,
    /// The peer's claimed height.
    pub their_height: u32,
    /// The height of the highest block both chains have, when the peer's last block is ours.
    pub highest_common_height: Option<u32>,
}

impl FastVerification {
    /// Whether the chains fork: without a common height (or with 0, which the reference reads as
    /// none), always; otherwise when the common height is at neither chain's height.
    pub fn forked(&self) -> bool {
        match self.highest_common_height {
            Some(common) if common != 0 => common != self.my_height && common != self.their_height,
            _ => true,
        }
    }
}

/// A peer, with the reference's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    /// The address, as the reference's address rules processed it.
    pub ip: IpAddr,
    /// The peer-to-peer port.
    pub port: u16,
    /// The version the peer reported, once known.
    pub version: Option<String>,
    /// The state of the last accepted getStatus reply, once pinged.
    pub state: Option<PeerState>,
    /// The plugins of the last accepted getStatus reply.
    pub plugins: Plugins,
    /// The port probe's result per plugin: the plugin's port when it answered 200, else -1.
    pub ports: IndexMap<String, i32>,
    /// The delegate keys attributed to this peer by its status attestations.
    pub public_keys: Vec<PublicKeyBytes>,
    /// The time of the last successful request, in milliseconds.
    pub latency: Option<u32>,
    /// When the peer was last pinged successfully, in milliseconds since the Unix epoch.
    pub last_pinged: Option<i64>,
    /// Whether the peer has not been pinged successfully yet (or was marked stale since).
    pub stale: bool,
    /// Failed requests since the last successful one.
    pub sequential_errors: u32,
    /// The times of the infractions, in milliseconds since the Unix epoch.
    pub infractions: Vec<i64>,
    /// The times of the protocol violations, in milliseconds since the Unix epoch.
    pub violations: Vec<i64>,
    /// The result of the last full verification.
    pub verification: Option<Verification>,
    /// The result of the last fast verification.
    pub fast_verification: Option<FastVerification>,
}

impl Peer {
    /// A new peer at `ip` and `port`: stale, never pinged, with no faults.
    pub fn new(ip: IpAddr, port: u16) -> Peer {
        Peer {
            ip,
            port,
            version: None,
            state: None,
            plugins: Plugins::default(),
            ports: IndexMap::new(),
            public_keys: Vec::new(),
            latency: None,
            last_pinged: None,
            stale: true,
            sequential_errors: 0,
            infractions: Vec::new(),
            violations: Vec::new(),
            verification: None,
            fast_verification: None,
        }
    }

    /// The peer's socket address.
    pub fn address(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }

    /// The height of the peer's last state, when it has one.
    pub fn height(&self) -> Option<u32> {
        self.state.as_ref().map(|state| state.height)
    }

    /// Whether the last full verification succeeded.
    pub fn is_verified(&self) -> bool {
        self.verification.is_some()
    }

    /// Whether the last full verification found the chains forked.
    pub fn is_forked(&self) -> bool {
        self.verification.is_some_and(|result| result.forked())
    }

    /// Whether the peer was pinged successfully less than 2 minutes before `now_ms`.
    pub fn recently_pinged(&self, now_ms: i64) -> bool {
        self.last_pinged
            .is_some_and(|pinged| now_ms.saturating_sub(pinged) < RECENTLY_PINGED_MS)
    }

    /// Record an infraction at `now_ms`. Two at the same millisecond count once, as the
    /// reference's set of times keeps them.
    pub fn add_infraction(&mut self, now_ms: i64) {
        if !self.infractions.contains(&now_ms) {
            self.infractions.push(now_ms);
        }
    }

    /// Record a protocol violation at `now_ms`. Two at the same millisecond count once.
    pub fn add_violation(&mut self, now_ms: i64) {
        if !self.violations.contains(&now_ms) {
            self.violations.push(now_ms);
        }
    }

    /// Forget the infractions and violations older than the window at `now_ms`.
    pub fn prune_faults(&mut self, now_ms: i64) {
        let recent = |time: &i64| now_ms.saturating_sub(*time) <= FAULT_WINDOW_MS;
        self.infractions.retain(recent);
        self.violations.retain(recent);
    }

    /// Whether the ping ignores the peer at `now_ms`: at least 3 infractions and violations
    /// together in the window. The faults outside the window are forgotten.
    pub fn is_ignored(&mut self, now_ms: i64) -> bool {
        self.prune_faults(now_ms);
        self.infractions.len().saturating_add(self.violations.len()) >= IGNORED_AT
    }

    /// Whether the peer has at least one protocol violation in the window at `now_ms`: it is then
    /// no source of blocks, transactions or peers and is not accepted again. The faults outside
    /// the window are forgotten.
    pub fn has_violation(&mut self, now_ms: i64) -> bool {
        self.prune_faults(now_ms);
        !self.violations.is_empty()
    }
}

/// A peer shared between the tasks that use it. Cloning the handle shares the peer.
///
/// The address never changes, so it is read without the lock.
#[derive(Debug, Clone)]
pub struct SharedPeer {
    ip: IpAddr,
    port: u16,
    peer: Arc<Mutex<Peer>>,
}

impl SharedPeer {
    /// A handle to `peer`.
    pub fn new(peer: Peer) -> SharedPeer {
        SharedPeer {
            ip: peer.ip,
            port: peer.port,
            peer: Arc::new(Mutex::new(peer)),
        }
    }

    /// The peer's address.
    pub fn ip(&self) -> IpAddr {
        self.ip
    }

    /// The peer's port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The peer's socket address.
    pub fn address(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }

    /// Run `change` on the peer. The peer is locked while `change` runs, so it must not wait for
    /// anything.
    pub fn with<R>(&self, change: impl FnOnce(&mut Peer) -> R) -> R {
        let mut peer = self.peer.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut peer)
    }

    /// A copy of the peer as it is now.
    pub fn snapshot(&self) -> Peer {
        self.with(|peer| peer.clone())
    }

    /// Whether `other` is a handle to the same peer object (not merely a peer at the same
    /// address).
    pub fn same(&self, other: &SharedPeer) -> bool {
        Arc::ptr_eq(&self.peer, &other.peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> Peer {
        Peer::new(IpAddr::from([192, 0, 2, 1]), 4002)
    }

    #[test]
    fn a_verification_is_forked_when_the_common_height_is_at_neither_height() {
        let result = |common| Verification {
            my_height: 10,
            their_height: 12,
            highest_common_height: common,
        };
        assert!(!result(10).forked());
        assert!(!result(12).forked());
        assert!(result(9).forked());
        let fast = |common| FastVerification {
            my_height: 10,
            their_height: 8,
            highest_common_height: common,
        };
        assert!(fast(None).forked());
        assert!(fast(Some(0)).forked());
        assert!(!fast(Some(8)).forked());
        assert!(fast(Some(7)).forked());
    }

    #[test]
    fn three_faults_in_the_window_ignore_a_peer_and_one_violation_makes_it_no_source() {
        let mut peer = peer();
        peer.add_infraction(1_000);
        peer.add_infraction(1_000);
        peer.add_infraction(2_000);
        assert!(!peer.is_ignored(3_000));
        assert!(!peer.has_violation(3_000));
        peer.add_violation(4_000);
        assert!(peer.is_ignored(4_000));
        assert!(peer.has_violation(4_000));
        // The first two leave the window, the violation stays.
        assert!(!peer.is_ignored(602_500));
        assert!(peer.has_violation(602_500));
        assert!(!peer.has_violation(604_001));
        // An honest slow peer: infractions only, never a violation.
        let mut slow = self::peer();
        for time in [1, 2, 3] {
            slow.add_infraction(time);
        }
        assert!(slow.is_ignored(10));
        assert!(!slow.has_violation(10));
    }

    #[test]
    fn a_ping_is_recent_for_just_under_two_minutes() {
        let mut peer = peer();
        assert!(!peer.recently_pinged(0));
        peer.last_pinged = Some(1_000);
        assert!(peer.recently_pinged(120_999));
        assert!(!peer.recently_pinged(121_000));
    }

    #[test]
    fn handles_share_one_peer() {
        let one = SharedPeer::new(peer());
        let two = one.clone();
        two.with(|peer| peer.sequential_errors = 2);
        assert_eq!(one.snapshot().sequential_errors, 2);
        assert!(one.same(&two));
        assert!(!one.same(&SharedPeer::new(self::peer())));
        assert_eq!(one.address(), "192.0.2.1:4002".parse().unwrap());
    }
}
