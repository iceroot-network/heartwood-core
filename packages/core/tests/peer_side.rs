//! The peer side of the server over a monitor of fakes: the acceptance handed to the acceptor on
//! the runtime the side was given, the peer test, the version and the stale mark, and a violation
//! recorded against a peer, a pending peer and an address that is no peer yet; and the delegates
//! on this node as the fork check reads them, none before the server is made.

use std::net::IpAddr;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_core::node_services::MonitorSide;
use heartwood_core::views::{Links, OnThisNode};
use heartwood_crypto::{Block, BlockId, Milestones, Network, PublicKeyBytes, TransactionId};
use heartwood_kernel::Clock;
use heartwood_p2p::diversity::Sampler;
use heartwood_p2p::errors::{DownloadError, PingError};
use heartwood_p2p::monitor::MonitorParts;
use heartwood_p2p::scoring::Violation;
use heartwood_p2p::seams::{PeerAcceptor, PeerClient};
use heartwood_p2p::utils::rules::Rules;
use heartwood_p2p::view::{BlockPing, DelegateInfo, NoEvents};
use heartwood_p2p::{
    AcceptOptions, AttestedKeys, BlocksRequest, ChainView, DownloadedBlock, NetworkMonitor,
    NoAttestedKeys, P2pOptions, Peer, PeerRepository, PeerSide, PeerState, SharedPeer,
};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress};
use tokio::runtime::{Builder, Runtime};

/// The time of the violations, in milliseconds since the Unix epoch.
const NOW: i64 = 1_800_000_000_000;

/// How long a hand-over to the acceptor may take.
const PATIENCE: Duration = Duration::from_secs(5);

/// A clock that stands still.
struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        NOW
    }
}

/// A chain that nothing here reads.
struct NoChain;

impl ChainView for NoChain {
    fn last_block(&self) -> Arc<Block> {
        unreachable!("the peer side reads no block")
    }
    fn last_blocks(&self) -> Vec<Arc<Block>> {
        unreachable!("the peer side reads no block")
    }
    fn milestones(&self) -> &Milestones {
        unreachable!("the peer side reads no milestone")
    }
    fn network(&self) -> &Network {
        unreachable!("the peer side reads no network")
    }
    fn delegate_by_key(&self, _key: &PublicKeyBytes) -> Option<DelegateInfo> {
        unreachable!("the peer side reads no delegate")
    }
    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        unreachable!("the peer side reads no delegate")
    }
    fn is_synced(&self) -> bool {
        unreachable!("the peer side reads no sync state")
    }
    fn is_idle(&self) -> bool {
        unreachable!("the peer side reads no machine state")
    }
    fn block_ping(&self) -> Option<BlockPing> {
        unreachable!("the peer side reads no block ping")
    }
}

/// A client that no call of the peer side reaches.
struct NoClient;

impl PeerClient for NoClient {
    async fn ping(
        &self,
        _peer: &SharedPeer,
        _timeout: Duration,
        _attest: bool,
        _force: bool,
        _skip_common_blocks: bool,
    ) -> Result<Option<PeerState>, PingError> {
        unreachable!("no request is made")
    }
    async fn ping_ports(&self, _peer: &SharedPeer) {
        unreachable!("no request is made")
    }
    async fn get_peers(&self, _peer: &SharedPeer, _silent: bool) -> Option<Vec<PeerAddress>> {
        unreachable!("no request is made")
    }
    async fn has_common_blocks(
        &self,
        _peer: &SharedPeer,
        _ids: &[BlockId],
        _timeout: Duration,
    ) -> Option<CommonBlock> {
        unreachable!("no request is made")
    }
    async fn get_peer_blocks(
        &self,
        _peer: &SharedPeer,
        _request: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        unreachable!("no request is made")
    }
    async fn get_unconfirmed_transactions(
        &self,
        _peer: &SharedPeer,
        _exclude: &[TransactionId],
    ) -> Vec<Vec<u8>> {
        unreachable!("no request is made")
    }
    async fn post_block(&self, _peer: &SharedPeer, _block: &Block) {
        unreachable!("no request is made")
    }
    fn post_transactions(&self, _peer: &SharedPeer, _transactions: Arc<Vec<Vec<u8>>>) {
        unreachable!("no request is made")
    }
    fn would_throttle(&self, _ip: IpAddr, _route: RouteId) -> bool {
        unreachable!("no request is made")
    }
    fn disconnect(&self, _ip: IpAddr) {
        unreachable!("no request is made")
    }
    fn remove_peer(&self, _ip: IpAddr) {
        unreachable!("no request is made")
    }
}

/// An acceptance that reports what it was handed.
struct Acceptor {
    handed: Mutex<Sender<(PeerAddress, AcceptOptions)>>,
}

impl PeerAcceptor for Acceptor {
    fn validate_peer_ip(&self, _peer: &mut PeerAddress, _options: AcceptOptions) -> bool {
        unreachable!("the peer side hands the address over unchecked")
    }
    async fn validate_and_accept(&self, peer: PeerAddress, options: AcceptOptions) {
        let handed = self.handed.lock().unwrap();
        handed.send((peer, options)).unwrap();
    }
}

type Side = MonitorSide<NoClient, Acceptor, Rules>;

/// A peer side over its own runtime, the repository it reads, and what its acceptor is handed.
/// The runtime is returned with it: the side's `accept` spawns on it from any thread.
fn side() -> (Side, Runtime, Receiver<(PeerAddress, AcceptOptions)>) {
    let runtime = Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (sender, handed) = channel();
    let acceptor = Arc::new(Acceptor {
        handed: Mutex::new(sender),
    });
    let repository = Arc::new(PeerRepository::new());
    let clock: Arc<dyn Clock> = Arc::new(FixedClock);
    let parts = MonitorParts {
        client: Arc::new(NoClient),
        acceptor: Arc::clone(&acceptor),
        rules: Arc::new(Rules::new(100, 25)),
        repository: Arc::clone(&repository),
        chain: Arc::new(NoChain),
        events: Arc::new(NoEvents),
        options: Arc::new(P2pOptions::default()),
        clock: Arc::clone(&clock),
        sampler: Sampler::from_os().unwrap(),
        runtime: runtime.handle().clone(),
        attested: Arc::new(NoAttestedKeys),
    };
    let side = MonitorSide {
        monitor: Arc::new(NetworkMonitor::new(parts)),
        repository,
        acceptor,
        clock,
        runtime: runtime.handle().clone(),
    };
    (side, runtime, handed)
}

fn ip(last: u8) -> IpAddr {
    IpAddr::from([192, 0, 2, last])
}

/// A peer at `ip(last)`, in the repository's peers.
fn stored(side: &Side, last: u8) -> SharedPeer {
    let peer = SharedPeer::new(Peer::new(ip(last), 4002));
    side.repository.set(peer.clone());
    peer
}

/// A peer at `ip(last)`, in the repository's pending peers.
fn pending(side: &Side, last: u8) -> SharedPeer {
    let peer = SharedPeer::new(Peer::new(ip(last), 4002));
    side.repository.set_pending(peer.clone());
    peer
}

#[test]
fn accept_hands_the_address_to_the_acceptor_on_the_runtime() {
    let (side, runtime, handed) = side();
    // This thread is in no runtime: the acceptance can only run on the one the side was given.
    assert!(tokio::runtime::Handle::try_current().is_err());
    side.accept(ip(7), 4005);
    let (address, options) = handed
        .recv_timeout(PATIENCE)
        .expect("the acceptor is handed it");
    let expected = PeerAddress {
        ip: "192.0.2.7".to_owned(),
        port: 4005,
    };
    assert_eq!(address, expected);
    assert_eq!(options, AcceptOptions::default());
    assert!(handed.try_recv().is_err(), "once");
    runtime.shutdown_timeout(PATIENCE);
}

#[test]
fn is_peer_is_true_for_a_stored_peer_only() {
    let (side, _runtime, _handed) = side();
    stored(&side, 1);
    pending(&side, 2);
    assert!(side.is_peer(ip(1)));
    assert!(!side.is_peer(ip(2)), "a pending peer is no peer yet");
    assert!(!side.is_peer(ip(3)));
}

#[test]
fn record_version_sets_the_version_of_a_peer_only() {
    let (side, _runtime, _handed) = side();
    let (peer, waiting) = (stored(&side, 1), pending(&side, 2));
    side.record_version(ip(1), "3.9.1");
    side.record_version(ip(2), "3.9.2");
    side.record_version(ip(3), "3.9.3");
    assert_eq!(peer.snapshot().version.as_deref(), Some("3.9.1"));
    assert_eq!(waiting.snapshot().version, None);
    assert!(!side.is_peer(ip(3)), "an unknown address stays unknown");
    side.record_version(ip(1), "3.9.4");
    assert_eq!(
        peer.snapshot().version.as_deref(),
        Some("3.9.4"),
        "the last"
    );
}

#[test]
fn mark_stale_marks_a_peer_stale_only() {
    let (side, _runtime, _handed) = side();
    let (peer, waiting) = (stored(&side, 1), pending(&side, 2));
    peer.with(|peer| peer.stale = false);
    waiting.with(|peer| peer.stale = false);
    side.mark_stale(ip(1));
    side.mark_stale(ip(2));
    side.mark_stale(ip(3));
    assert!(peer.snapshot().stale);
    assert!(!waiting.snapshot().stale);
    assert!(!side.is_peer(ip(3)), "an unknown address stays unknown");
}

#[test]
fn a_violation_of_a_peer_is_recorded_on_the_peer() {
    let (side, _runtime, _handed) = side();
    let (peer, other) = (stored(&side, 1), stored(&side, 2));
    side.record_violation(ip(1), Violation::Protocol);
    assert_eq!(peer.snapshot().violations, vec![NOW]);
    assert!(side.repository.has_violation(ip(1), NOW));
    assert!(other.snapshot().violations.is_empty());
    assert!(!side.repository.has_violation(ip(2), NOW));
}

#[test]
fn a_violation_of_a_pending_peer_is_recorded_on_the_pending_peer() {
    let (side, _runtime, _handed) = side();
    let waiting = pending(&side, 1);
    side.record_violation(ip(1), Violation::ReplyDecode);
    assert_eq!(waiting.snapshot().violations, vec![NOW]);
    assert!(!side.is_peer(ip(1)), "the pending peer stays pending");
    assert!(side.repository.has_pending(ip(1)));
    // The pending peer carries it: nothing is kept for the address besides.
    assert!(!side.repository.has_violation(ip(1), NOW));
}

#[test]
fn a_violation_of_an_unknown_address_is_kept_in_the_repository() {
    let (side, _runtime, _handed) = side();
    stored(&side, 1);
    assert!(!side.repository.has_violation(ip(9), NOW));
    side.record_violation(ip(9), Violation::BogusCommonBlock);
    assert!(side.repository.has_violation(ip(9), NOW));
    assert!(!side.is_peer(ip(9)), "no peer is made of it");
    assert!(!side.repository.has_pending(ip(9)));
    assert!(!side.repository.has_violation(ip(1), NOW));
}

/// The keys of an attestation, as a fake gives them.
struct Keys(Vec<PublicKeyBytes>);

impl AttestedKeys for Keys {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        self.0.clone()
    }
}

fn key(byte: &str) -> PublicKeyBytes {
    PublicKeyBytes::from_hex(&format!("02{}", byte.repeat(32))).unwrap()
}

#[test]
fn on_this_node_has_no_keys_before_the_server_is_made() {
    let links = Arc::new(Links::default());
    let on_this_node = OnThisNode(Arc::clone(&links));
    assert!(on_this_node.attested().is_empty());
    // The attestation session's keys are another set: they are no delegates of the fork check.
    let _ = links.attested.set(Arc::new(Keys(vec![key("11")])));
    assert!(on_this_node.attested().is_empty());
}

#[test]
fn on_this_node_has_the_attested_keys_after_the_server_is_made() {
    let links = Arc::new(Links::default());
    let on_this_node = OnThisNode(Arc::clone(&links));
    let _ = links.attested.set(Arc::new(Keys(vec![key("11")])));
    let keys = vec![key("22"), key("33"), key("22")];
    let _ = links.on_this_node.set(Arc::new(Keys(keys.clone())));
    assert_eq!(on_this_node.attested(), keys, "in order, duplicates kept");
}
