//! The network state, as the reference's `NetworkState.analyze` builds it for getNetworkState:
//! the tip and the slot; the peers; the delegates on this node (the attested keys that are also
//! active in the round of the tip plus one, in declared order, duplicates kept) as a local peer
//! `127.0.0.1:<P2P port>` with the tip's state, those keys removed from every other peer; the
//! peers without an active delegate dropped. A cold start answers `ColdStart` and ends it; fewer
//! active delegates across the peers than `minimumDelegateReach` answer `BelowMinimumDelegates`;
//! otherwise each peer counts toward the quorum by its active delegates.

use heartwood_crypto::PublicKeyBytes;
use heartwood_p2p_wire::internal::NetworkState;

use crate::peer::Peer;
use crate::view::ChainView;

/// What the analysis reads.
pub struct AnalyzeInput<'a> {
    /// The node's chain.
    pub chain: &'a dyn ChainView,
    /// The peers, their attested keys as the repository holds them.
    pub peers: &'a [Peer],
    /// The attested keys of the node's forger.
    pub attested: &'a [PublicKeyBytes],
    /// The active delegates' keys of the round of the tip plus one.
    pub active_next_round: &'a [PublicKeyBytes],
    /// The node's P2P port.
    pub port: u16,
    /// The time, in milliseconds since the Unix epoch.
    pub now_ms: i64,
    /// Whether the node is in its cold start.
    pub cold_start: bool,
    /// The `minimumDelegateReach` option, if set.
    pub minimum_delegate_reach: Option<u32>,
}

/// What the analysis found, and the keys to remove from the repository's peers until their next
/// attestation, as the reference reassigns them.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The network state.
    pub state: NetworkState,
    /// The delegates on this node, removed from the other peers.
    pub on_this_node: Vec<PublicKeyBytes>,
}

/// The analysis of `input`.
pub fn analyze(input: &AnalyzeInput<'_>) -> Analysis {
    let _ = input;
    todo!("the network state's analysis")
}
