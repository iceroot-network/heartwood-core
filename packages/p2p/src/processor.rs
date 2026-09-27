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
//! 5. A new peer is marked pending, pinged with `verifyTimeout` as a race against a timer, its
//!    ports probed, stored, logged `Accepted new peer <ip>:<port> (v<version>)` unless less
//!    verbose, and `peer.added` is reported. Any failure disconnects it; the pending mark is
//!    always removed.

use std::net::IpAddr;
use std::sync::Arc;

use heartwood_kernel::Clock;
use heartwood_p2p_wire::replies::PeerAddress;

use crate::options::P2pOptions;
use crate::repository::PeerRepository;
use crate::seams::{PeerAcceptor, PeerClient, PeerRules};
use crate::view::{ChainView, PeerEvents};

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
    /// The node's chain, for the slot of the ping.
    pub chain: Arc<dyn ChainView>,
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
    #[expect(dead_code, reason = "the acceptance's methods read it")]
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
}

impl<C, R> PeerAcceptor for PeerProcessor<C, R>
where
    C: PeerClient + 'static,
    R: PeerRules + 'static,
{
    fn validate_peer_ip(&self, peer: &mut PeerAddress, options: AcceptOptions) -> bool {
        let _ = (peer, options);
        todo!("the address rules")
    }

    async fn validate_and_accept(&self, peer: PeerAddress, options: AcceptOptions) {
        let _ = (peer, options);
        todo!("the acceptance")
    }
}
