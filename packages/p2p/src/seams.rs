//! The seams between the parts of the client: each trait is implemented by one part and called by
//! others, so every part can be tested against fakes of its neighbours.
//!
//! | Trait | Implemented by | Called by |
//! |---|---|---|
//! | [`PeerTransport`] | [`crate::connector::Connector`] | the communicator |
//! | [`PeerClient`] | [`crate::communicator::Communicator`] | the acceptance, the monitor, the node |
//! | [`VerifierLink`] | [`crate::communicator::Communicator`] | the verifier |
//! | [`StateCheck`] | [`crate::verifier::PeerVerifier`] | the communicator's ping |
//! | [`PeerAcceptor`] | [`crate::processor::PeerProcessor`] | the monitor's discovery and seeding |
//! | [`PeerRules`] | [`crate::utils::rules::Rules`] | the communicator, the acceptance, the monitor |
//! | [`PortProbe`] | [`crate::seeds::HttpProbe`] | the communicator's port probes |
//!
//! The asynchronous methods return `Send` futures, so they run on a multi-threaded runtime; the
//! parts are generic over these traits.

use std::cmp::Ordering;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::crypto::slots::SlotInfo;
use heartwood_crypto::{Block, BlockId, Params, TransactionId};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;
use heartwood_p2p_wire::errors::ReplyRefused;
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
use heartwood_p2p_wire::messages::peer::{GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress, UnconfirmedReply};
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use tokio::time::Instant;

use crate::communicator::BlocksRequest;
use crate::download::DownloadedBlock;
use crate::errors::{DownloadError, PastDeadline, PingError, SessionError};
use crate::peer::{FastVerification, PeerState, SharedPeer, Verification};
use crate::processor::AcceptOptions;

/// The time by which a peer verification must have sent its last request, on tokio's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Deadline(pub Instant);

impl Deadline {
    /// The deadline `timeout` from now (now itself if the sum overflowed, which no timeout of the
    /// reference comes near).
    pub fn after(timeout: Duration) -> Deadline {
        let now = Instant::now();
        Deadline(now.checked_add(timeout).unwrap_or(now))
    }

    /// The time left before the deadline, or [`PastDeadline`] when it is now or past (the
    /// reference's `throwIfPastDeadline`).
    pub fn remaining(&self) -> Result<Duration, PastDeadline> {
        let now = Instant::now();
        if self.0 <= now {
            return Err(PastDeadline);
        }
        Ok(self.0.duration_since(now))
    }
}

/// The connection to peers: one nes session per peer IP.
pub trait PeerTransport: Send + Sync {
    /// Send `payload` on `route` to the peer at `address` and wait for the response's payload. A
    /// session to the IP is opened first when there is none; the session's receive limit becomes
    /// `max_payload` and its request timeout `timeout`.
    fn emit(
        &self,
        address: SocketAddr,
        route: RouteId,
        payload: Vec<u8>,
        timeout: Duration,
        max_payload: usize,
    ) -> impl Future<Output = Result<Vec<u8>, SessionError>> + Send;

    /// Terminate and forget the session to `ip`, if there is one.
    fn disconnect(&self, ip: IpAddr);

    /// Record `name`, the JavaScript class of the last error, for `ip`.
    fn set_error(&self, ip: IpAddr, name: &'static str);

    /// Forget the last error of `ip`.
    fn forget_error(&self, ip: IpAddr);
}

/// The requests to peers, as the reference's peer communicator makes them. A failure is counted
/// against the peer and the caller gets the reference's empty answer, except where a method
/// returns an error.
pub trait PeerClient: Send + Sync {
    /// Ping `peer`: getStatus, the network and version checks, a verification of its chain
    /// (fast, without common blocks, when `skip_common_blocks`), then the status attestations for
    /// the current `slot`. `None` when the peer is ignored, or was pinged recently and is not
    /// stale and `force` is off.
    fn ping(
        &self,
        peer: &SharedPeer,
        timeout: Duration,
        slot: Option<SlotInfo>,
        force: bool,
        skip_common_blocks: bool,
    ) -> impl Future<Output = Result<Option<PeerState>, PingError>> + Send;

    /// Probe each of the peer's plugin ports with an HTTP `HEAD /`.
    fn ping_ports(&self, peer: &SharedPeer) -> impl Future<Output = ()> + Send;

    /// getPeers; `None` when the peer gave no valid reply.
    fn get_peers(
        &self,
        peer: &SharedPeer,
        silent: bool,
    ) -> impl Future<Output = Option<Vec<PeerAddress>>> + Send;

    /// getCommonBlocks with `ids`; the common block, or `None` for no valid reply and for
    /// `common: null`.
    fn has_common_blocks(
        &self,
        peer: &SharedPeer,
        ids: &[BlockId],
        timeout: Duration,
    ) -> impl Future<Output = Option<CommonBlock>> + Send;

    /// getBlocks; the blocks, empty when the peer gave no valid reply. A transaction that the
    /// reference's `fromBytesUnsafe` refuses fails the download ([`DownloadError`]), as the
    /// reference throws there.
    fn get_peer_blocks(
        &self,
        peer: &SharedPeer,
        request: BlocksRequest,
    ) -> impl Future<Output = Result<Vec<DownloadedBlock>, DownloadError>> + Send;

    /// getUnconfirmedTransactions without the transactions of `exclude`; the serialised
    /// transactions, empty when the peer gave no valid reply or more than the milestone allows.
    fn get_unconfirmed_transactions(
        &self,
        peer: &SharedPeer,
        exclude: &[TransactionId],
    ) -> impl Future<Output = Vec<Vec<u8>>> + Send;

    /// postBlock; a height in the reply becomes the peer's height.
    fn post_block(&self, peer: &SharedPeer, block: &Block) -> impl Future<Output = ()> + Send;

    /// Queue a postTransactions of `transactions` on the peer's own queue, whose requests are
    /// spaced by the postTransactions rate limit.
    fn post_transactions(&self, peer: &SharedPeer, transactions: Arc<Vec<Vec<u8>>>);

    /// Whether a request on `route` to `ip` would wait for the outgoing rate limiter now.
    fn would_throttle(&self, ip: IpAddr, route: RouteId) -> bool;

    /// Terminate and forget the session to `ip` (the connector's disconnect).
    fn disconnect(&self, ip: IpAddr);

    /// The reference's `peer.disconnect` event: terminate the session to `ip`, forget the peer and
    /// its postTransactions queue.
    fn remove_peer(&self, ip: IpAddr);
}

/// The two requests the verifier makes, through the communicator.
pub trait VerifierLink: Send + Sync {
    /// getCommonBlocks with `ids`: [`PeerClient::has_common_blocks`].
    fn common_blocks(
        &self,
        peer: &SharedPeer,
        ids: &[BlockId],
        timeout: Duration,
    ) -> impl Future<Output = Option<CommonBlock>> + Send;

    /// getBlocks of at most `limit` headers after height `from`, with the configured getBlocks
    /// timeout; empty when the peer gave no valid reply.
    fn headers(
        &self,
        peer: &SharedPeer,
        from: u32,
        limit: u32,
    ) -> impl Future<Output = Vec<DownloadedBlock>> + Send;
}

/// The verification of a peer's claimed state, as the ping calls it.
pub trait StateCheck: Send + Sync {
    /// The full verification (`checkState`): the claimed header, then the highest common block
    /// and the peer's blocks after it up to the round's end, fetched through `link`. `None` when
    /// the state does not verify; [`PastDeadline`] when the deadline passed before a request.
    fn check_state<L: VerifierLink>(
        &self,
        link: &L,
        peer: &SharedPeer,
        claimed: &PeerState,
        deadline: Deadline,
    ) -> impl Future<Output = Result<Option<Verification>, PastDeadline>> + Send;

    /// The fast verification (`checkStateFast`): the claimed header only; `None` when it does not
    /// verify.
    fn check_state_fast(
        &self,
        peer: &SharedPeer,
        claimed: &PeerState,
    ) -> impl Future<Output = Option<FastVerification>> + Send;
}

/// The acceptance of candidate peers.
pub trait PeerAcceptor: Send + Sync {
    /// The address rules (`validatePeerIp`); `peer.ip` becomes the processed address.
    fn validate_peer_ip(&self, peer: &mut PeerAddress, options: AcceptOptions) -> bool;

    /// `validateAndAcceptPeer`: the address rules, then a ping of a known peer, or the pending
    /// mark, a ping, the port probes and the store of a new one.
    fn validate_and_accept(
        &self,
        peer: PeerAddress,
        options: AcceptOptions,
    ) -> impl Future<Output = ()> + Send;
}

/// The wire's requests and reply checks, the outgoing rate limiter and the peer rules, as the
/// communicator, the acceptance and the monitor call them. Each method is the function of the
/// same name in `heartwood_p2p_wire` or in [`crate::utils`].
pub trait PeerRules: Send + Sync {
    /// [`ClientRequest::encode`].
    fn encode_request(&self, request: &ClientRequest, headers: &Headers) -> Vec<u8>;

    /// [`heartwood_p2p_wire::replies::check_peers`].
    fn check_peers(&self, peers: &[ListedPeer]) -> Result<Vec<PeerAddress>, ReplyRefused>;

    /// [`heartwood_p2p_wire::replies::check_common_blocks`].
    fn check_common_blocks(
        &self,
        reply: &GetCommonBlocksResponse,
    ) -> Result<Option<CommonBlock>, ReplyRefused>;

    /// [`heartwood_p2p_wire::replies::check_blocks`].
    fn check_blocks(&self, blocks: &mut [ListedBlock]) -> Result<(), ReplyRefused>;

    /// [`heartwood_p2p_wire::replies::check_post_block`].
    fn check_post_block(&self, reply: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused>;

    /// [`heartwood_p2p_wire::replies::check_unconfirmed`].
    fn check_unconfirmed(
        &self,
        reply: &UnconfirmedReply,
        max_transactions: usize,
    ) -> Result<(), ReplyRefused>;

    /// [`heartwood_p2p_wire::replies::check_post_transactions`].
    fn check_post_transactions(&self, accept: &[String]) -> Result<(), ReplyRefused>;

    /// The outgoing rate limiter's step before a request on `route` to `ip` at `now`: `None`
    /// after consuming a point, or the time to wait before asking again when the request would
    /// exceed a limit (nothing is consumed then).
    fn throttle(&self, ip: IpAddr, route: RouteId, now: Instant) -> Option<Duration>;

    /// Whether a request on `route` to `ip` at `now` would exceed a limit, without consuming.
    fn would_throttle(&self, ip: IpAddr, route: RouteId, now: Instant) -> bool;

    /// [`crate::utils::is_valid_peer::is_valid_peer`].
    fn is_valid_peer(&self, ip: &mut String, allow_private: bool, interfaces: &[IpAddr]) -> bool;

    /// [`crate::utils::is_valid_version::is_valid_version`].
    fn is_valid_version(
        &self,
        version: Option<&str>,
        milestone: &Params,
        configured: &[String],
        network_name: &str,
    ) -> bool;

    /// [`crate::utils::subnet::same_subnet`].
    fn same_subnet(&self, a: IpAddr, b: IpAddr) -> bool;

    /// [`crate::utils::npm_semver::compare`].
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering>;
}

/// The HTTP probe of a peer's plugin ports.
pub trait PortProbe: Send + Sync {
    /// An HTTP `HEAD /` to `ip` on `port`; the response's status code, or `None` when there was
    /// no response.
    fn head(&self, ip: IpAddr, port: u16) -> impl Future<Output = Option<u16>> + Send;
}
