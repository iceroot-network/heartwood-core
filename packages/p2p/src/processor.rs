//! The acceptance of candidate peers (`validateAndAcceptPeer`).
//!
//! 1. `disableDiscovery` refuses every candidate (a warning).
//! 2. The address rules: processed as ipaddr.js processes it; refused unless its range is
//!    unicast (or private, link-local or unique-local when private ranges are allowed), when its
//!    text starts with `0`, or when it is one of the host's own addresses.
//! 3. Refused while the IP is pending, when the whitelist does not match or the blacklist does
//!    (the kernel's globs), and when `maxSameSubnetPeers` peers already share its /24, unless it
//!    is a seed; also refused while the peer has a protocol violation in its window.
//! 4. A known peer is pinged again (and its ports probed when it is stale); nothing else changes.
//! 5. A new peer is marked pending, pinged with `verifyTimeout` as a race against a timer (a ping
//!    that loses runs on), its ports probed, stored, logged `Accepted new peer <ip>:<port>
//!    (v<version>)` unless less verbose, and `peer.added` is reported. Any failure disconnects
//!    it; the pending mark is always removed.

use std::net::IpAddr;
use std::sync::Arc;

use heartwood_kernel::{Clock, Event, ip};
use heartwood_p2p_wire::replies::PeerAddress;

use crate::options::P2pOptions;
use crate::peer::{Peer, SharedPeer};
use crate::repository::PeerRepository;
use crate::seams::{PeerAcceptor, PeerClient, PeerRules};
use crate::view::PeerEvents;

/// How a candidate is accepted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AcceptOptions {
    /// A seed from `peers.json` or its sources: exempt from the same-subnet cap.
    pub seed: bool,
    /// No "Accepted new peer" line.
    pub less_verbose: bool,
}

/// What an acceptance is made of.
pub struct ProcessorParts<C, R> {
    /// The requests to peers.
    pub client: Arc<C>,
    /// The peer rules.
    pub rules: Arc<R>,
    /// The peers and the pending peers.
    pub repository: Arc<PeerRepository>,
    /// Where peer events go.
    pub events: Arc<dyn PeerEvents>,
    /// The p2p options.
    pub options: Arc<P2pOptions>,
    /// The host's own addresses, which are never peers.
    pub interfaces: Vec<IpAddr>,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
}

/// The acceptance of candidate peers.
pub struct PeerProcessor<C, R> {
    parts: ProcessorParts<C, R>,
}

impl<C, R> PeerProcessor<C, R>
where
    C: PeerClient + 'static,
    R: PeerRules + 'static,
{
    /// An acceptance of `parts`.
    pub fn new(parts: ProcessorParts<C, R>) -> PeerProcessor<C, R> {
        PeerProcessor { parts }
    }

    /// The ping of acceptance: a full ping with `verifyTimeout`, as a race against a timer of the
    /// same length; whether it succeeded in time. A ping that loses the race runs on in the
    /// background, as the reference's does, and may still change the peer and the attestations.
    async fn ping(&self, peer: &SharedPeer) -> bool {
        let (client, peer) = (Arc::clone(&self.parts.client), peer.clone());
        let timeout = self.parts.options.verify_timeout;
        let ping =
            tokio::spawn(async move { client.ping(&peer, timeout, true, false, false).await });
        matches!(tokio::time::timeout(timeout, ping).await, Ok(Ok(Ok(_))))
    }

    /// `acceptNewPeer`: a known peer is pinged again, and its ports probed when it was stale; a
    /// new one is pending while it is pinged, then probed, stored and reported.
    async fn accept(&self, ip: IpAddr, port: u16, options: AcceptOptions) {
        let parts = &self.parts;
        if let Some(known) = parts.repository.get(ip) {
            let stale = known.with(|peer| peer.stale);
            if self.ping(&known).await && stale {
                parts.client.ping_ports(&known).await;
            }
            return;
        }
        let peer = SharedPeer::new(Peer::new(ip, port));
        // The pending check of the address rules and the mark are one step, as the reference's
        // are: a second acceptance of the same new IP started meanwhile stops here.
        if !parts.repository.try_set_pending(peer.clone()) {
            return;
        }
        if self.ping(&peer).await {
            parts.client.ping_ports(&peer).await;
            parts.repository.set(peer.clone());
            if !options.less_verbose {
                let version = peer.with(|peer| peer.version.clone()).unwrap_or_default();
                heartwood_kernel::debug!(
                    "Accepted new peer {ip}:{port} (v{version}) :hugging_face:"
                );
            }
            parts.events.emit(Event::PeerAdded);
        } else {
            parts.client.disconnect(ip);
            parts.repository.keep_violations(&peer);
        }
        parts.repository.forget_pending(ip);
    }
}

impl<C, R> PeerAcceptor for PeerProcessor<C, R>
where
    C: PeerClient + 'static,
    R: PeerRules + 'static,
{
    fn validate_peer_ip(&self, peer: &mut PeerAddress, options: AcceptOptions) -> bool {
        let (parts, config) = (&self.parts, &self.parts.options);
        if config.disable_discovery {
            let ip = &peer.ip;
            heartwood_kernel::warning!(
                "Rejected {ip} because the relay is in non-discovery mode :see_no_evil:"
            );
            return false;
        }
        let private = config.allow_private_ip_ranges;
        if !parts
            .rules
            .is_valid_peer(&mut peer.ip, private, &parts.interfaces)
        {
            return false;
        }
        // The processed text of an address ipaddr.js accepts; a zone index, which only a
        // link-local address has, is refused here.
        let Ok(address) = peer.ip.parse::<IpAddr>() else {
            return false;
        };
        let repository = &parts.repository;
        let known = repository.peers();
        let same_subnet = known
            .iter()
            .filter(|known| parts.rules.same_subnet(known.ip(), address));
        !repository.has_pending(address)
            && ip::is_whitelisted(&config.whitelist, &peer.ip)
            && !ip::is_blacklisted(&config.blacklist, &peer.ip)
            && (options.seed || same_subnet.count() < config.max_same_subnet_peers)
            && !repository.has_violation(address, parts.clock.now_ms())
    }

    async fn validate_and_accept(&self, mut peer: PeerAddress, options: AcceptOptions) {
        if !self.validate_peer_ip(&mut peer, options) {
            return;
        }
        if let Ok(ip) = peer.ip.parse::<IpAddr>() {
            self.accept(ip, peer.port, options).await;
        }
    }
}
