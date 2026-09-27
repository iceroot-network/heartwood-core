//! The acceptance of candidate peers over a fake client, the peer rules' stand-ins and the
//! kernel's address lists, on tokio's paused clock: discovery off, the address rules, the pending
//! mark, the whitelist and the blacklist, the same-subnet cap, a known peer pinged again, a new
//! peer pinged, probed, stored and reported, and the ping's race against `verifyTimeout`.

mod peer_fakes;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_crypto::crypto::slots::SlotInfo;
use heartwood_crypto::{Block, BlockId, TransactionId};
use heartwood_kernel::{Event, ManualClock};
use heartwood_p2p::communicator::BlocksRequest;
use heartwood_p2p::errors::{DownloadError, PingError};
use heartwood_p2p::seams::{PeerAcceptor, PeerClient};
use heartwood_p2p::{
    AcceptOptions, DownloadedBlock, P2pOptions, PeerProcessor, PeerRepository, PeerState,
    ProcessorParts, SharedPeer,
};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress};
use peer_fakes::*;

/// How the fake client answers a ping.
#[derive(Debug, Clone, Copy)]
enum Ping {
    /// The peer answers, with this version, after this long.
    Answers(&'static str, Duration),
    /// The ping fails.
    Fails,
}

/// A client whose ping answers as it was told per IP (by default at once, version 4.3.1), and
/// which records the pings, probes and disconnects.
#[derive(Default)]
struct Client {
    plans: Mutex<HashMap<IpAddr, Ping>>,
    pinged: Mutex<Vec<(IpAddr, Duration, bool)>>,
    probed: Mutex<Vec<IpAddr>>,
    disconnected: Mutex<Vec<IpAddr>>,
    pending_while_pinged: Mutex<Vec<bool>>,
    repository: Mutex<Option<Arc<PeerRepository>>>,
}

impl Client {
    fn plan(&self, ip: IpAddr, ping: Ping) {
        self.plans.lock().unwrap().insert(ip, ping);
    }

    fn pinged(&self) -> Vec<IpAddr> {
        self.pinged
            .lock()
            .unwrap()
            .iter()
            .map(|(ip, ..)| *ip)
            .collect()
    }
}

impl PeerClient for Client {
    async fn ping(
        &self,
        peer: &SharedPeer,
        timeout: Duration,
        slot: Option<SlotInfo>,
        force: bool,
        skip_common_blocks: bool,
    ) -> Result<Option<PeerState>, PingError> {
        assert!(!force && !skip_common_blocks);
        self.pinged
            .lock()
            .unwrap()
            .push((peer.ip(), timeout, slot.is_some()));
        if let Some(repository) = self.repository.lock().unwrap().as_ref() {
            self.pending_while_pinged
                .lock()
                .unwrap()
                .push(repository.has_pending(peer.ip()));
        }
        let plan = self.plans.lock().unwrap().get(&peer.ip()).copied();
        match plan.unwrap_or(Ping::Answers("4.3.1", Duration::ZERO)) {
            Ping::Answers(version, after) => {
                tokio::time::sleep(after).await;
                peer.with(|peer| {
                    peer.version = Some(version.to_owned());
                    peer.stale = false;
                });
                Ok(None)
            }
            Ping::Fails => Err(PingError::VerificationFailed),
        }
    }

    async fn ping_ports(&self, peer: &SharedPeer) {
        self.probed.lock().unwrap().push(peer.ip());
    }

    async fn get_peers(&self, _peer: &SharedPeer, _silent: bool) -> Option<Vec<PeerAddress>> {
        unreachable!("acceptance asks for no peers")
    }

    async fn has_common_blocks(
        &self,
        _peer: &SharedPeer,
        _ids: &[BlockId],
        _timeout: Duration,
    ) -> Option<CommonBlock> {
        unreachable!("acceptance asks for no common blocks")
    }

    async fn get_peer_blocks(
        &self,
        _peer: &SharedPeer,
        _request: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        unreachable!("acceptance asks for no blocks")
    }

    async fn get_unconfirmed_transactions(
        &self,
        _peer: &SharedPeer,
        _exclude: &[TransactionId],
    ) -> Vec<Vec<u8>> {
        unreachable!("acceptance asks for no transactions")
    }

    async fn post_block(&self, _peer: &SharedPeer, _block: &Block) {
        unreachable!("acceptance posts nothing")
    }

    fn post_transactions(&self, _peer: &SharedPeer, _transactions: Arc<Vec<Vec<u8>>>) {
        unreachable!("acceptance posts nothing")
    }

    fn would_throttle(&self, _ip: IpAddr, _route: RouteId) -> bool {
        false
    }

    fn disconnect(&self, ip: IpAddr) {
        self.disconnected.lock().unwrap().push(ip);
    }

    fn remove_peer(&self, _ip: IpAddr) {
        unreachable!("acceptance removes no peer")
    }
}

struct Acceptance {
    client: Arc<Client>,
    repository: Arc<PeerRepository>,
    events: Arc<Events>,
    clock: Arc<ManualClock>,
    processor: PeerProcessor<Client, Rules>,
}

/// The host's own address in these tests.
const OWN: [u8; 4] = [198, 51, 100, 200];

fn acceptance(options: P2pOptions) -> Acceptance {
    let client = Arc::new(Client::default());
    let repository = Arc::new(PeerRepository::new());
    *client.repository.lock().unwrap() = Some(Arc::clone(&repository));
    let events = Arc::new(Events::default());
    let clock = Arc::new(ManualClock::new(START_MS));
    let processor = PeerProcessor::new(ProcessorParts {
        client: Arc::clone(&client),
        rules: Arc::new(Rules::default()),
        repository: Arc::clone(&repository),
        chain: Arc::new(Chain::at(10)),
        events: events.clone(),
        options: Arc::new(options),
        interfaces: vec![IpAddr::from(OWN)],
        clock: clock.clone(),
    });
    Acceptance {
        client,
        repository,
        events,
        clock,
        processor,
    }
}

fn candidate(ip: &str) -> PeerAddress {
    PeerAddress {
        ip: ip.to_owned(),
        port: 4002,
    }
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

const PLAIN: AcceptOptions = AcceptOptions {
    seed: false,
    less_verbose: false,
};

#[tokio::test(start_paused = true)]
async fn a_new_peer_is_pending_while_pinged_then_probed_stored_and_reported() {
    let accept = acceptance(options());
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), PLAIN)
        .await;
    let stored = accept.repository.get(ip("192.0.2.1")).unwrap();
    assert_eq!(stored.port(), 4002);
    assert_eq!(stored.snapshot().version.as_deref(), Some("4.3.1"));
    assert!(!accept.repository.has_pending(ip("192.0.2.1")));
    assert_eq!(*accept.client.pending_while_pinged.lock().unwrap(), [true]);
    // A full ping with verifyTimeout and the current slot.
    let pinged = accept.client.pinged.lock().unwrap().clone();
    assert_eq!(pinged, [(ip("192.0.2.1"), Duration::from_secs(60), true)]);
    assert_eq!(*accept.client.probed.lock().unwrap(), [ip("192.0.2.1")]);
    assert_eq!(accept.events.all(), [Event::PeerAdded]);
    assert!(accept.client.disconnected.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_failed_or_late_ping_disconnects_the_new_peer() {
    let accept = acceptance(options());
    accept.client.plan(ip("192.0.2.1"), Ping::Fails);
    let late = Ping::Answers("4.3.1", Duration::from_secs(61));
    accept.client.plan(ip("192.0.2.2"), late);
    for address in ["192.0.2.1", "192.0.2.2"] {
        accept
            .processor
            .validate_and_accept(candidate(address), PLAIN)
            .await;
    }
    assert!(accept.repository.is_empty());
    assert!(accept.repository.pending().is_empty());
    let disconnected = accept.client.disconnected.lock().unwrap().clone();
    assert_eq!(disconnected, [ip("192.0.2.1"), ip("192.0.2.2")]);
    assert!(accept.client.probed.lock().unwrap().is_empty());
    assert!(accept.events.all().is_empty());
    // A ping that answers just inside verifyTimeout wins the race.
    accept.client.plan(
        ip("192.0.2.3"),
        Ping::Answers("4.3.1", Duration::from_secs(59)),
    );
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.3"), PLAIN)
        .await;
    assert!(accept.repository.has(ip("192.0.2.3")));
}

#[tokio::test(start_paused = true)]
async fn a_known_peer_is_pinged_again_and_probed_only_when_it_was_stale() {
    let accept = acceptance(options());
    let known = peer(1);
    accept.repository.set(known.clone());
    // Stale: pinged, then probed.
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), PLAIN)
        .await;
    assert_eq!(accept.client.pinged(), [known.ip()]);
    assert_eq!(*accept.client.probed.lock().unwrap(), [known.ip()]);
    // Not stale any more: pinged, not probed; a failed ping changes nothing.
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), PLAIN)
        .await;
    accept.client.plan(known.ip(), Ping::Fails);
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), PLAIN)
        .await;
    assert_eq!(accept.client.pinged().len(), 3);
    assert_eq!(accept.client.probed.lock().unwrap().len(), 1);
    assert!(accept.repository.get(known.ip()).unwrap().same(&known));
    assert!(accept.events.all().is_empty());
    assert!(accept.client.disconnected.lock().unwrap().is_empty());
    assert_eq!(
        *accept.client.pending_while_pinged.lock().unwrap(),
        [false; 3]
    );
}

#[tokio::test(start_paused = true)]
async fn discovery_off_refuses_every_candidate() {
    let accept = acceptance(P2pOptions {
        disable_discovery: true,
        ..options()
    });
    let mut seed = candidate("192.0.2.1");
    let seeded = AcceptOptions {
        seed: true,
        less_verbose: true,
    };
    assert!(!accept.processor.validate_peer_ip(&mut seed, seeded));
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), seeded)
        .await;
    assert!(accept.client.pinged().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_address_rules_process_and_refuse_addresses() {
    let accept = acceptance(options());
    let valid = |address: &str| {
        let mut peer = candidate(address);
        let valid = accept.processor.validate_peer_ip(&mut peer, PLAIN);
        (valid, peer.ip)
    };
    // An IPv4-mapped IPv6 address becomes IPv4.
    assert_eq!(valid("::ffff:192.0.2.7"), (true, "192.0.2.7".to_owned()));
    assert!(valid("2001:db8::1").0);
    for refused in [
        "127.0.0.1",
        "10.0.0.1",
        "0.1.2.3",
        "not an address",
        "198.51.100.200",
    ] {
        assert!(!valid(refused).0, "{refused}");
    }
    let private = acceptance(P2pOptions {
        allow_private_ip_ranges: true,
        ..options()
    });
    let mut peer = candidate("10.0.0.1");
    assert!(private.processor.validate_peer_ip(&mut peer, PLAIN));
}

#[tokio::test(start_paused = true)]
async fn pending_peers_and_the_address_lists_refuse_candidates() {
    let accept = acceptance(P2pOptions {
        whitelist: vec!["192.0.2.*".to_owned(), "2001:db8::*".to_owned()],
        blacklist: vec!["192.0.2.66".to_owned()],
        ..options()
    });
    let valid = |address: &str| {
        accept
            .processor
            .validate_peer_ip(&mut candidate(address), PLAIN)
    };
    assert!(valid("192.0.2.1"));
    assert!(valid("2001:db8::5"));
    assert!(!valid("198.51.100.1"));
    assert!(!valid("192.0.2.66"));
    accept.repository.set_pending(peer(9));
    assert!(!valid("192.0.2.9"));
    // An empty whitelist allows everyone.
    let open = acceptance(P2pOptions {
        whitelist: Vec::new(),
        ..options()
    });
    assert!(
        open.processor
            .validate_peer_ip(&mut candidate("198.51.100.1"), PLAIN)
    );
}

#[tokio::test(start_paused = true)]
async fn the_same_subnet_cap_refuses_all_but_seeds() {
    let accept = acceptance(P2pOptions {
        max_same_subnet_peers: 2,
        ..options()
    });
    accept.repository.set(peer(1));
    accept.repository.set(peer(2));
    let seed = AcceptOptions {
        seed: true,
        less_verbose: true,
    };
    assert!(
        !accept
            .processor
            .validate_peer_ip(&mut candidate("192.0.2.3"), PLAIN)
    );
    assert!(
        accept
            .processor
            .validate_peer_ip(&mut candidate("192.0.2.3"), seed)
    );
    // Another /24, and IPv6, are not capped by it.
    assert!(
        accept
            .processor
            .validate_peer_ip(&mut candidate("192.0.3.3"), PLAIN)
    );
    assert!(
        accept
            .processor
            .validate_peer_ip(&mut candidate("2001:db8::3"), PLAIN)
    );
}

#[tokio::test(start_paused = true)]
async fn a_peer_with_a_violation_is_not_accepted_again_within_its_window() {
    let accept = acceptance(options());
    let violator = peer(1);
    violator.with(|peer| peer.add_violation(START_MS));
    accept.repository.set(violator.clone());
    // Known, with a violation: not pinged through acceptance.
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), PLAIN)
        .await;
    assert!(accept.client.pinged().is_empty());
    // Forgotten (a cleanse or three errors), then named again by discovery.
    accept.repository.forget(violator.ip());
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), PLAIN)
        .await;
    assert!(!accept.repository.has(violator.ip()));
    // After the 600 s window it is accepted again.
    accept.clock.advance(600_001);
    accept
        .processor
        .validate_and_accept(candidate("192.0.2.1"), PLAIN)
        .await;
    assert!(accept.repository.has(violator.ip()));
    assert_eq!(accept.events.all(), [Event::PeerAdded]);
}
