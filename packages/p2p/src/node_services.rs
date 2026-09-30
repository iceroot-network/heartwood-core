//! What the server reads of the node and asks of it, and what it reads of the peer side: the
//! traits the node implements for the server, and the types they share.
//!
//! The P2P crate has no edge to the state, the pool, the store or the blockchain service. The
//! server's controllers and plugins reach the node through [`NodeServices`], the peer repository
//! and the network monitor through [`PeerSide`], and the node's own delegates through
//! [`AttestedKeys`]. The node implements the first two; the attestation session implements the
//! third. The tests fake all three.
//!
//! Every trait here is object-safe: its asynchronous methods return boxed futures, so the server
//! holds them as `Arc<dyn ...>` whatever the node's types are.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use heartwood_crypto::{Block, BlockHeader, BlockId, PublicKeyBytes, TransactionId};
use heartwood_kernel::ChainLink;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::BlockListing;
use heartwood_p2p_wire::internal::{CurrentRound, EmitEvent, NetworkState};
use heartwood_p2p_wire::status::PeerConfig;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::watch;

use crate::errors::LookupError;
use crate::monitor::CleanseOptions;
use crate::peer::Peer;
use crate::scoring::Violation;
use crate::view::ChainView;

/// What AwaitBlock waits on: whether the machine is in `newBlock`, and whether the job queue runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueState {
    /// The machine is in `newBlock`.
    pub new_block: bool,
    /// A job runs.
    pub running: bool,
}

/// How a public request's AwaitBlock step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AwaitEnd {
    /// Nothing to wait for: the machine was not in `newBlock` with the queue running.
    NoWait,
    /// The queue drained.
    Drained,
    /// The bound ended the wait.
    Bound,
    /// The request's socket closed before the drain or the bound; the wait still ran to one of
    /// them, as the reference runs a request's steps after its socket has closed.
    SocketClosed,
}

/// One public request's AwaitBlock step, as the server saw it: the record of
/// [`NodeServices::await_block_step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitRecord {
    /// The route.
    pub route: RouteId,
    /// The caller's address, normalised.
    pub caller: IpAddr,
    /// When the request reached the step, in milliseconds since the Unix epoch (UTC).
    pub at_ms: i64,
    /// The whole wait; zero when there was nothing to wait for.
    pub waited: Duration,
    /// How the wait ended.
    pub end: AwaitEnd,
    /// The chain's tip when the step began.
    pub tip_before: u32,
    /// The chain's tip when it ended; the answer is built after it.
    pub tip_after: u32,
}

/// A step of a status's attestation, as the status server saw it: the record of
/// [`NodeServices::attestation_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttestationStep {
    /// A changed status was offered to the forger's session.
    Offered,
    /// The forger's verified signatures attached to it.
    Signed {
        /// The attestations attached.
        attestations: usize,
    },
    /// A getStatus served it unsigned after the wait.
    ServedUnsigned,
}

/// One step of a status's attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationRecord {
    /// When, in milliseconds since the Unix epoch (UTC).
    pub at_ms: i64,
    /// The step.
    pub step: AttestationStep,
    /// The SHA-256 of the status's text, in hex, which names its offer.
    pub status: String,
    /// The status's slot.
    pub slot: i64,
}

/// A collation of the pool for getUnconfirmedTransactions: the pool's size and the candidate
/// transactions, serialised, in the collator's order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collation {
    /// The number of transactions in the pool.
    pub pool_size: usize,
    /// The collated transactions.
    pub transactions: Vec<Vec<u8>>,
}

/// One of the four events a forger may emit (`emitEvent`); the relay dispatches no other name.
#[derive(Debug, Clone, PartialEq)]
pub enum ForgerEvent {
    /// `forger.started`.
    Started(Value),
    /// `forger.failed`.
    Failed(Value),
    /// `block.forged`.
    BlockForged(Value),
    /// `transaction.forged`.
    TransactionForged(Value),
}

impl ForgerEvent {
    /// The event of `emitted`, when its name is one of the four.
    pub fn from_emit(emitted: EmitEvent) -> Option<ForgerEvent> {
        let body = emitted.body;
        match emitted.event.as_str() {
            "forger.started" => Some(ForgerEvent::Started(body)),
            "forger.failed" => Some(ForgerEvent::Failed(body)),
            "block.forged" => Some(ForgerEvent::BlockForged(body)),
            "transaction.forged" => Some(ForgerEvent::TransactionForged(body)),
            _ => None,
        }
    }
}

/// The node stopped, so it answers no more work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the node has stopped")]
pub struct NodeUnavailable;

/// The node, as the server's controllers and plugins read it and ask it for work.
pub trait NodeServices: Send + Sync {
    /// Whether the blockchain has started; every route answers 499 "App is not ready" before.
    fn is_booted(&self) -> bool;

    /// When the node first reported itself synced (`blockchain.synced`), in milliseconds since
    /// the Unix epoch; `None` before. The socket-close bans apply from 60 s after it.
    fn first_synced_ms(&self) -> Option<i64>;

    /// The machine's `newBlock` state and the job queue's running state, for AwaitBlock.
    fn queue_state(&self) -> watch::Receiver<QueueState>;

    /// The node's chain, as the client reads it.
    fn chain(&self) -> Arc<dyn ChainView>;

    /// The last downloaded block, or the tip when there is none (the reference's
    /// `getLastDownloadedBlock`).
    fn last_downloaded_block(&self) -> ChainLink;

    /// Another receipt of the pinged block, when `header` is that block (the reference's
    /// `pingBlock`); whether it was.
    fn ping_block(&self, header: &BlockHeader) -> bool;

    /// A block posted to this node, from its forger or from the peer at `ip`: the reference's
    /// `handleIncomingBlock(block, fromForger, ip)`.
    fn handle_incoming_block(&self, block: Block, from_forger: bool, ip: IpAddr);

    /// The rank of the delegate named `username`, when it is a delegate with a rank.
    fn delegate_rank(&self, username: &str) -> Option<u32>;

    /// Whether the delegate named `username` already forged in the round of `height` (the
    /// reference's `getRewardForBlockInRound(height, wallet).alreadyForged`, for the reduced
    /// reward line of postBlock).
    fn already_forged(&self, height: u32, username: &str) -> Result<bool, LookupError>;

    /// The active delegates' keys of the round of the tip plus one, in `getActiveDelegates`
    /// order.
    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError>;

    /// At most `limit` stored blocks from height `from`, with their transactions unless
    /// `headers_only`, as the reference's `getBlocksForDownload` gives them.
    fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<BlockListing>, LookupError>;

    /// The headers of the blocks among `ids`, the ids a peer asked for that can name a block (of
    /// the `asked` it sent; an id in capitals names none): from memory when it holds as many as
    /// were asked, else from the store, as the reference's `getCommonBlocks` reads them.
    fn common_blocks(&self, ids: &[BlockId], asked: usize)
    -> Result<Vec<BlockHeader>, LookupError>;

    /// The number of transactions in the pool.
    fn pool_size(&self) -> usize;

    /// The collator's candidates at the tip plus one, on the node thread: validated for a
    /// forger, without the transactions of `exclude`.
    fn collate(
        &self,
        validate: bool,
        exclude: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>>;

    /// The P2P channel's pool processor over `transactions`; the ids it accepted.
    fn post_transactions(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>>;

    /// The reference's getCurrentRound answer at `now_ms`.
    fn current_round(&self, now_ms: i64) -> Result<CurrentRound, LookupError>;

    /// The slot of `timestamp` (of now when `None`) with the block time at the tip plus one.
    fn slot_number(&self, timestamp: Option<i64>, now_ms: i64) -> i64;

    /// The blockchain's `forceWakeup`.
    fn force_wakeup(&self);

    /// Dispatch a forger's event.
    fn forger_event(&self, event: ForgerEvent);

    /// The node's peer configuration for its status.
    fn peer_config(&self) -> PeerConfig;

    /// A getNetworkState answer served to `caller`: nothing, except in a harness build, which
    /// records it.
    fn network_state_served(&self, caller: IpAddr, state: &NetworkState) {
        let _ = (caller, state);
    }

    /// A public request's AwaitBlock step ended, as `record` says: nothing, except in a harness
    /// build, which appends the record to its start's file as one JSON line. Internal routes pass
    /// the step without a record.
    fn await_block_step(&self, record: &AwaitRecord) {
        let _ = record;
    }

    /// A step of a status's attestation, as `record` says (an offer, its signatures' attachment,
    /// a getStatus served unsigned after the wait): nothing, except in a harness build, which
    /// appends the record to its start's file as one JSON line.
    fn attestation_step(&self, record: &AttestationRecord) {
        let _ = record;
    }
}

/// The peer side as the server reads and changes it: the repository and the network monitor.
pub trait PeerSide: Send + Sync {
    /// Try to accept the caller at `ip` as a peer at `port`, in the background (the pre-handler's
    /// peer acceptance).
    fn accept(&self, ip: IpAddr, port: u16);

    /// Whether `ip` is a known peer.
    fn is_peer(&self, ip: IpAddr) -> bool;

    /// Record `version`, from a request's headers, for the known peer at `ip`.
    fn record_version(&self, ip: IpAddr, version: &str);

    /// Mark the known peer at `ip` stale: its socket closed.
    fn mark_stale(&self, ip: IpAddr);

    /// Record a transport violation of the caller at `ip`.
    fn record_violation(&self, ip: IpAddr, violation: Violation);

    /// The peers, in the repository's order.
    fn peers(&self) -> Vec<Peer>;

    /// Remove `keys` from every peer's attested keys, until each peer's next attestation: the
    /// delegates on this node, which the reference takes from the other peers when it analyses
    /// the network state and in the fork health check.
    fn remove_keys(&self, keys: &[PublicKeyBytes]);

    /// Whether the node starts a new network and has not left the cold start.
    fn is_cold_start(&self) -> bool;

    /// End the cold start.
    fn complete_cold_start(&self);

    /// A cleanse of the peers with `options` (getNetworkState's forced fast cleanse).
    fn cleanse(&self, options: CleanseOptions) -> BoxFuture<'_, ()>;
}

/// The delegates on this node: the keys whose status signatures verified in the forger's current
/// attestation session, in declared order, duplicates kept.
pub trait AttestedKeys: Send + Sync {
    /// The attested keys; empty without a session.
    fn attested(&self) -> Vec<PublicKeyBytes>;
}

/// No delegate on this node: a relay without a forger, and the tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAttestedKeys;

impl AttestedKeys for NoAttestedKeys {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_four_forger_events_are_dispatched() {
        let emit = |event: &str| EmitEvent {
            event: event.to_owned(),
            body: json!({"height": 2}),
        };
        let body = json!({"height": 2});
        assert_eq!(
            ForgerEvent::from_emit(emit("block.forged")),
            Some(ForgerEvent::BlockForged(body.clone()))
        );
        assert_eq!(
            ForgerEvent::from_emit(emit("forger.started")),
            Some(ForgerEvent::Started(body.clone()))
        );
        assert!(ForgerEvent::from_emit(emit("forger.failed")).is_some());
        assert!(ForgerEvent::from_emit(emit("transaction.forged")).is_some());
        assert_eq!(ForgerEvent::from_emit(emit("block.applied")), None);
        assert!(NoAttestedKeys.attested().is_empty());
    }
}
