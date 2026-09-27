//! The peer communicator: every request the client makes, as the reference's makes it.
//!
//! Each request: the throttle (the outgoing rate limiter, rechecked every second); the codec;
//! the session, with the route's receive limit and timeout; the decode; the peer's sequential
//! error counter reset and its latency recorded; the route's reply check. Every failure is
//! counted on the peer, and at `maxPeerSequentialErrors` the peer is disconnected and forgotten.
//! The class of the error decides the rest: a reply that fails its schema and every error of
//! class `Error` are only counted; any other class (a decoder's `RangeError`, `TypeError` or
//! `SyntaxError`) also disconnects the peer, except on getBlocks and getUnconfirmedTransactions.
//!
//! | Route | Receive limit | Timeout |
//! |---|---|---|
//! | getStatus | 100 KiB | the ping's timeout, at most 5 s |
//! | getPeers | 100 KiB | 5 s |
//! | getCommonBlocks | 100 KiB | the time left, at most 5 s |
//! | getBlocks | 20 MiB, or 1 KiB per block with headers only | `getBlocksTimeout` (30 s), or the caller's |
//! | getUnconfirmedTransactions | the milestone's `block.maxPayload` | 10 s |
//! | postBlock | 100 KiB | 10 s |
//! | postTransactions | 100 KiB | 10 s, on the peer's own queue |

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::crypto::slots::SlotInfo;
use heartwood_crypto::{Block, BlockId, TransactionId};
use heartwood_kernel::Clock;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress};

use crate::download::DownloadedBlock;
use crate::errors::{DownloadError, PingError};
use crate::options::P2pOptions;
use crate::peer::{PeerState, SharedPeer};
use crate::repository::PeerRepository;
use crate::seams::{PeerClient, PeerRules, PeerTransport, PortProbe, StateCheck, VerifierLink};
use crate::view::{ChainView, OnlineSink, PeerEvents};

/// The most blocks one getBlocks asks for.
pub const MAX_DOWNLOAD_BLOCKS: u32 = 400;

/// A getBlocks request as the communicator's callers give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlocksRequest {
    /// The height after which blocks are wanted.
    pub from_height: u32,
    /// The most blocks wanted; [`MAX_DOWNLOAD_BLOCKS`] by default.
    pub block_limit: u32,
    /// Whether only the headers are wanted.
    pub headers_only: bool,
    /// Whether an empty answer is logged.
    pub silent: bool,
    /// The request's timeout; `getBlocksTimeout` when `None`.
    pub timeout: Option<Duration>,
}

impl BlocksRequest {
    /// The blocks after `from_height`, at most [`MAX_DOWNLOAD_BLOCKS`], with transactions, logged
    /// when empty, with the configured timeout.
    pub fn after(from_height: u32) -> BlocksRequest {
        BlocksRequest {
            from_height,
            block_limit: MAX_DOWNLOAD_BLOCKS,
            headers_only: false,
            silent: false,
            timeout: None,
        }
    }
}

/// What a communicator is made of.
pub struct CommunicatorParts<T, S, R, P> {
    /// The sessions to peers.
    pub transport: Arc<T>,
    /// The verification of a pinged peer's state.
    pub verifier: Arc<S>,
    /// The wire's requests and reply checks, the rate limiter and the peer rules.
    pub rules: Arc<R>,
    /// The HTTP probe of plugin ports.
    pub probe: Arc<P>,
    /// The peers, which the attestations and the disconnects read and change.
    pub repository: Arc<PeerRepository>,
    /// The node's chain.
    pub chain: Arc<dyn ChainView>,
    /// The record of online delegates.
    pub online: Arc<dyn OnlineSink>,
    /// Where peer events go.
    pub events: Arc<dyn PeerEvents>,
    /// The p2p options.
    pub options: Arc<P2pOptions>,
    /// The wall clock of infractions, pings and slots.
    pub clock: Arc<dyn Clock>,
}

/// The peer communicator.
pub struct Communicator<T, S, R, P> {
    #[expect(dead_code, reason = "the communicator's methods read it")]
    parts: CommunicatorParts<T, S, R, P>,
}

impl<T, S, R, P> Communicator<T, S, R, P>
where
    T: PeerTransport + 'static,
    S: StateCheck + 'static,
    R: PeerRules + 'static,
    P: PortProbe + 'static,
{
    /// A communicator of `parts`.
    pub fn new(parts: CommunicatorParts<T, S, R, P>) -> Communicator<T, S, R, P> {
        Communicator { parts }
    }
}

impl<T, S, R, P> PeerClient for Communicator<T, S, R, P>
where
    T: PeerTransport + 'static,
    S: StateCheck + 'static,
    R: PeerRules + 'static,
    P: PortProbe + 'static,
{
    async fn ping(
        &self,
        peer: &SharedPeer,
        timeout: Duration,
        slot: Option<SlotInfo>,
        force: bool,
        skip_common_blocks: bool,
    ) -> Result<Option<PeerState>, PingError> {
        let _ = (peer, timeout, slot, force, skip_common_blocks);
        todo!("ping")
    }

    async fn ping_ports(&self, peer: &SharedPeer) {
        let _ = peer;
        todo!("the port probes")
    }

    async fn get_peers(&self, peer: &SharedPeer, silent: bool) -> Option<Vec<PeerAddress>> {
        let _ = (peer, silent);
        todo!("getPeers")
    }

    async fn has_common_blocks(
        &self,
        peer: &SharedPeer,
        ids: &[BlockId],
        timeout: Duration,
    ) -> Option<CommonBlock> {
        let _ = (peer, ids, timeout);
        todo!("getCommonBlocks")
    }

    async fn get_peer_blocks(
        &self,
        peer: &SharedPeer,
        request: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        let _ = (peer, request);
        todo!("getBlocks")
    }

    async fn get_unconfirmed_transactions(
        &self,
        peer: &SharedPeer,
        exclude: &[TransactionId],
    ) -> Vec<Vec<u8>> {
        let _ = (peer, exclude);
        todo!("getUnconfirmedTransactions")
    }

    async fn post_block(&self, peer: &SharedPeer, block: &Block) {
        let _ = (peer, block);
        todo!("postBlock")
    }

    fn post_transactions(&self, peer: &SharedPeer, transactions: Arc<Vec<Vec<u8>>>) {
        let _ = (peer, transactions);
        todo!("postTransactions on the peer's queue")
    }

    fn would_throttle(&self, ip: IpAddr, route: RouteId) -> bool {
        let _ = (ip, route);
        todo!("the throttle check")
    }

    fn disconnect(&self, ip: IpAddr) {
        let _ = ip;
        todo!("disconnect")
    }

    fn remove_peer(&self, ip: IpAddr) {
        let _ = ip;
        todo!("the peer.disconnect event")
    }
}

impl<T, S, R, P> VerifierLink for Communicator<T, S, R, P>
where
    T: PeerTransport + 'static,
    S: StateCheck + 'static,
    R: PeerRules + 'static,
    P: PortProbe + 'static,
{
    async fn common_blocks(
        &self,
        peer: &SharedPeer,
        ids: &[BlockId],
        timeout: Duration,
    ) -> Option<CommonBlock> {
        let _ = (peer, ids, timeout);
        todo!("getCommonBlocks for the verifier")
    }

    async fn headers(&self, peer: &SharedPeer, from: u32, limit: u32) -> Vec<DownloadedBlock> {
        let _ = (peer, from, limit);
        todo!("getBlocks of headers for the verifier")
    }
}
