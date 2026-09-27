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
//!
//! Beyond the reference, a failure that no honest peer produces is recorded as a protocol
//! violation ([`crate::scoring`]), and a peer with a violation in its window is asked for no
//! blocks, transactions or peers: those requests answer empty without being sent.
//!
//! A communicator needs the node's version for the headers of its requests, so it cannot be made
//! without one ([`RequestError::EmptyVersion`]).

use std::collections::HashMap;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::join_all;
use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::crypto::slots::SlotInfo;
use heartwood_crypto::{Block, BlockId, Params, Transaction, TransactionId};
use heartwood_kernel::{Clock, Event};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::{blocks, peer as peer_codecs, transactions};
use heartwood_p2p_wire::errors::{DecodeError, ReplyRefused, RequestError};
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress};
use heartwood_p2p_wire::requests::{ClientRequest, CommonBlockIds, Headers};
use heartwood_p2p_wire::status::{self, CheckedStatus};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};

use crate::attestation::{self, AttestContext, Holder, current_slot};
use crate::download::DownloadedBlock;
use crate::errors::{DownloadError, PingError};
use crate::options::P2pOptions;
use crate::peer::{PeerState, SharedPeer};
use crate::repository::PeerRepository;
use crate::scoring::{self, Violation};
use crate::seams::{
    Deadline, PeerClient, PeerRules, PeerTransport, PortProbe, StateCheck, VerifierLink,
};
use crate::view::{ChainView, OnlineSink, PeerEvents};

/// The most blocks one getBlocks asks for.
pub const MAX_DOWNLOAD_BLOCKS: u32 = 400;

/// The client's receive limit when a route sets none of its own: 100 KiB.
const SMALL: usize = 100 * 1024;
/// The receive limit of a getBlocks with transactions: 20 MiB, the server socket's limit.
const LARGE: usize = 20 * 1024 * 1024;
/// The getPeers timeout, and the longest getStatus and getCommonBlocks timeout.
const SHORT: Duration = Duration::from_secs(5);
/// The timeout of getUnconfirmedTransactions, postBlock and postTransactions.
const LONG: Duration = Duration::from_secs(10);
/// The error class of a reply that fails its schema.
const VALIDATION: &str = "CoreValidationError";

/// A getBlocks request as the communicator's callers give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlocksRequest {
    /// The height after which blocks are wanted.
    pub from_height: u32,
    /// The most blocks wanted; [`MAX_DOWNLOAD_BLOCKS`] by default. A larger value asks for
    /// [`MAX_DOWNLOAD_BLOCKS`], since a reference server refuses more and bans the caller.
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
    /// The runtime the postTransactions queues run on, so a batch can be queued from any thread.
    pub runtime: tokio::runtime::Handle,
}

/// The peer communicator.
pub struct Communicator<T, S, R, P> {
    shared: Arc<Shared<T, S, R, P>>,
}

/// A batch of a peer's postTransactions queue.
type Batch = (SharedPeer, Arc<Vec<Vec<u8>>>);

/// What the requests need, shared with the tasks of the postTransactions queues.
struct Shared<T, S, R, P> {
    parts: CommunicatorParts<T, S, R, P>,
    /// Every request's headers.
    headers: Headers,
    queues: Mutex<HashMap<IpAddr, mpsc::UnboundedSender<Batch>>>,
    /// Held through each ping's attestation step, which reads and changes every peer's keys: the
    /// reference runs each step whole, so one ping's step sees the keys the last one attributed.
    attesting: Mutex<()>,
}

/// A failed request: the reference's error class, the violation it shows, and the schema's
/// refusal when there is one.
type Failure = (&'static str, Option<Violation>, Option<ReplyRefused>);

/// `n` as a size.
fn size(n: impl Into<u64>) -> usize {
    usize::try_from(n.into()).unwrap_or(usize::MAX)
}

impl<T, S, R, P> Communicator<T, S, R, P> {
    /// A communicator of `parts`; [`RequestError::EmptyVersion`] when the options' version is
    /// empty, since every request carries it.
    pub fn new(
        parts: CommunicatorParts<T, S, R, P>,
    ) -> Result<Communicator<T, S, R, P>, RequestError> {
        let headers = Headers::new(parts.options.version.clone(), u32::from(parts.options.port))?;
        let shared = Arc::new(Shared {
            parts,
            headers,
            queues: Mutex::default(),
            attesting: Mutex::default(),
        });
        Ok(Communicator { shared })
    }
}

impl<T, S, R, P> Shared<T, S, R, P>
where
    T: PeerTransport + 'static,
    S: StateCheck + 'static,
    R: PeerRules + 'static,
    P: PortProbe + 'static,
{
    /// The milestone in force at our last block, as the reference's configuration manager holds
    /// it.
    fn milestone(&self) -> &Params {
        let chain = &self.parts.chain;
        chain.milestones().at(chain.last_block().height())
    }

    /// The largest `value` of the milestones (0 without one), as the reference's reply schemas
    /// compute their bounds when they load.
    fn largest(&self, value: impl Fn(&Params) -> Option<u64>) -> usize {
        let milestones = self.parts.chain.milestones().all();
        milestones.iter().filter_map(value).max().map_or(0, size)
    }

    /// Whether `peer` has a protocol violation in its window: it is then no source.
    fn violator(&self, peer: &SharedPeer) -> bool {
        let now = self.parts.clock.now_ms();
        peer.with(|peer| peer.has_violation(now))
    }

    /// One request: the throttle, the codec, the session, the decode, the counter and latency,
    /// the reply check. `None` after a failure, which is counted.
    ///
    /// An amount beyond 128 bits fails at our decode; the reference decodes it, resets the
    /// counter and records the latency, and refuses it at its reply schema, so that order is kept.
    async fn emit<D, V>(
        &self,
        peer: &SharedPeer,
        request: ClientRequest,
        (timeout, limit): (Duration, usize),
        decode: impl FnOnce(&[u8]) -> Result<D, DecodeError>,
        check: impl FnOnce(D) -> Result<V, ReplyRefused>,
    ) -> Option<V> {
        let (parts, ip, route) = (&self.parts, peer.ip(), request.route());
        while let Some(wait) = parts.rules.throttle(ip, route, Instant::now()) {
            sleep(wait).await;
        }
        let payload = parts.rules.encode_request(&request, &self.headers);
        parts.transport.forget_error(ip);
        let started = Instant::now();
        let reply = parts
            .transport
            .emit(peer.address(), route, payload, timeout, limit);
        let answered = || {
            let latency = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
            peer.with(|peer| {
                peer.sequential_errors = 0;
                peer.latency = Some(latency);
            });
        };
        let failure: Failure = match reply.await.map(|bytes| decode(&bytes)) {
            Err(error) => (error.js_class(), Violation::of_session(&error), None),
            // A codec refusal has its class; an amount beyond 128 bits has none (the reference
            // reads it, and its reply schema then refuses it).
            Ok(Err(error)) => {
                if error == DecodeError::AmountTooWide {
                    answered();
                }
                let class = error.js_class().unwrap_or(VALIDATION);
                (class, Some(Violation::of_decode(error)), None)
            }
            Ok(Ok(decoded)) => {
                answered();
                match check(decoded) {
                    Ok(value) => return Some(value),
                    Err(refused) => (VALIDATION, Some(Violation::ReplySchema), Some(refused)),
                }
            }
        };
        self.fail(peer, failure, route);
        None
    }

    /// The reference's `handleSocketError`, with the violation recorded first. The reference's
    /// getBlocks and getUnconfirmedTransactions disconnect no peer for the error's class.
    fn fail(&self, peer: &SharedPeer, (class, violation, refused): Failure, route: RouteId) {
        let ip = peer.ip();
        self.parts.transport.set_error(ip, class);
        if let Some(violation) = violation {
            scoring::record(peer, violation, self.parts.clock.now_ms());
        }
        let errors = peer.with(|peer| {
            peer.sequential_errors = peer.sequential_errors.saturating_add(1);
            peer.sequential_errors
        });
        if errors >= self.parts.options.max_peer_sequential_errors {
            self.remove_peer(ip);
        }
        if let Some(refused) = refused {
            heartwood_kernel::debug!(
                "Socket data validation error (peer {ip}) : Response validation failed from peer {ip} : {refused} :warning:"
            );
        } else if class != VALIDATION
            && class != "Error"
            && !matches!(
                route,
                RouteId::GetBlocks | RouteId::GetUnconfirmedTransactions
            )
        {
            self.remove_peer(ip);
        }
    }

    /// The reference's `peer.disconnect` event and its listeners: the session terminated, the
    /// peer forgotten (its violations kept), its postTransactions queue dropped (the batches
    /// already queued are still sent, as the reference's queue keeps running).
    fn remove_peer(&self, ip: IpAddr) {
        self.parts.events.emit(Event::PeerDisconnect);
        self.parts.transport.disconnect(ip);
        self.parts.repository.forget(ip);
        let mut queues = self.queues.lock().unwrap_or_else(PoisonError::into_inner);
        queues.remove(&ip);
    }

    async fn common_blocks(
        &self,
        peer: &SharedPeer,
        ids: &[BlockId],
        timeout: Duration,
    ) -> Option<CommonBlock> {
        let ids = CommonBlockIds::new(ids.to_vec()).ok()?;
        let timeout = if timeout.is_zero() {
            SHORT
        } else {
            timeout.min(SHORT)
        };
        let rules = &self.parts.rules;
        self.emit(
            peer,
            ClientRequest::GetCommonBlocks { ids },
            (timeout, SMALL),
            peer_codecs::get_common_blocks_decode_response,
            |reply| rules.check_common_blocks(&reply),
        )
        .await
        .flatten()
    }

    async fn get_blocks(
        &self,
        peer: &SharedPeer,
        request: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        let parts = &self.parts;
        let (Some(last_block_height), Some(block_limit)) = (
            NonZeroU32::new(request.from_height),
            NonZeroU32::new(request.block_limit.min(MAX_DOWNLOAD_BLOCKS)),
        ) else {
            return Ok(Vec::new());
        };
        let headers_only = request.headers_only;
        let limit = if headers_only {
            size(block_limit.get()).saturating_mul(1024)
        } else {
            LARGE
        };
        let timeout = request.timeout.filter(|timeout| !timeout.is_zero());
        let get = ClientRequest::GetBlocks {
            last_block_height,
            block_limit,
            headers_only,
        };
        let listed = self
            .emit(
                peer,
                get,
                (timeout.unwrap_or(parts.options.get_blocks_timeout), limit),
                blocks::get_blocks_decode_response,
                |mut listed| parts.rules.check_blocks(&mut listed).map(|()| listed),
            )
            .await
            .unwrap_or_default();
        if listed.is_empty() && !request.silent {
            let (ip, from) = (peer.ip(), request.from_height);
            heartwood_kernel::debug!(
                "Peer {ip} did not return any blocks via height {from} :see_no_evil:"
            );
        }
        let params = self.milestone();
        let mut downloaded = Vec::with_capacity(listed.len());
        for mut header in listed {
            let mut transactions = Vec::with_capacity(header.transactions.len());
            let listed = std::mem::take(&mut header.transactions);
            for bytes in listed.into_iter().filter(|_| !headers_only) {
                let source = match Transaction::decode_trusted(&bytes, params) {
                    Ok(transaction) => {
                        transactions.push(transaction);
                        if parts.chain.is_idle() {
                            sleep(Duration::from_millis(1)).await;
                        }
                        continue;
                    }
                    Err(source) => source,
                };
                // A memo that is not UTF-8 the reference accepts, so honest peers serve it.
                if source != heartwood_crypto::errors::DecodeError::InvalidMemo {
                    scoring::record(peer, Violation::DownloadedBlock, parts.clock.now_ms());
                }
                let height = header.header.height.unwrap_or(0);
                return Err(DownloadError::Transaction { height, source });
            }
            let transactions = (!headers_only).then_some(transactions);
            downloaded.push(DownloadedBlock {
                header,
                transactions,
                ip: peer.ip(),
            });
        }
        Ok(downloaded)
    }

    /// The task of one IP's postTransactions queue: each batch posted in turn, then a pause of
    /// `ceil(1000 / rateLimitPostTransactions)` ms. It ends when its queue is dropped and empty.
    async fn run_queue(self: Arc<Self>, mut batches: mpsc::UnboundedReceiver<Batch>) {
        let rate = u64::from(self.parts.options.rate_limit_post_transactions);
        let spacing = Duration::from_millis(1000_u64.div_ceil(rate.max(1)));
        while let Some((peer, transactions)) = batches.recv().await {
            let transactions = Arc::unwrap_or_clone(transactions);
            let rules = &self.parts.rules;
            self.emit(
                &peer,
                ClientRequest::PostTransactions { transactions },
                (LONG, SMALL),
                transactions::post_transactions_decode_response,
                |accept| rules.check_post_transactions(&accept),
            )
            .await;
            sleep(spacing).await;
        }
    }

    /// The status attestations of a successful ping, applied: the keys attributed to `peer`, the
    /// keys taken from other peers, and the versions recorded in the online record. One step at a
    /// time across pings.
    fn attest(&self, peer: &SharedPeer, status: &CheckedStatus, slot: &SlotInfo) {
        let _step = self
            .attesting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let parts = &self.parts;
        let pinged = peer.with(|peer| Holder::of(peer));
        let mut others = parts.repository.peers();
        others.retain(|other| other.ip() != pinged.ip);
        let holders: Vec<Holder> = others
            .iter()
            .map(|other| other.with(|other| Holder::of(other)))
            .collect();
        let (last, keys) = (parts.chain.last_block(), parts.chain.delegate_keys());
        let last_id = last.id().to_hex();
        let context = AttestContext {
            slot,
            last_height: last.height(),
            last_id: &last_id,
            delegate_keys: &keys,
            milestones: parts.chain.milestones(),
        };
        let decided = attestation::attest(status, &pinged, &holders, &context);
        for (ip, key) in &decided.moved {
            for other in others.iter().filter(|other| other.ip() == *ip) {
                other.with(|other| other.public_keys.retain(|held| held != key));
            }
        }
        peer.with(|peer| peer.public_keys.clone_from(&decided.attributed));
        for record in decided.versions {
            parts
                .online
                .set_version(record.key, record.round, record.version);
        }
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
        attest: bool,
        force: bool,
        skip_common_blocks: bool,
    ) -> Result<Option<PeerState>, PingError> {
        let (shared, parts) = (&self.shared, &self.shared.parts);
        let deadline = Deadline::after(timeout);
        let now = parts.clock.now_ms();
        let skip = peer.with(|peer| {
            peer.version
                .get_or_insert_with(|| parts.options.version.clone());
            peer.is_ignored(now) || (!peer.stale && peer.recently_pinged(now) && !force)
        });
        if skip {
            return Ok(None);
        }
        let most = shared.largest(|params| Some(params.active_delegates()));
        let checked = shared
            .emit(
                peer,
                ClientRequest::GetStatus,
                (timeout.min(SHORT), SMALL),
                status::decode_response,
                |received| status::check_reply(received, most),
            )
            .await;
        let checked =
            checked.and_then(|checked| Some((PeerState::from_checked(&checked)?, checked)));
        let Some((mut state, checked)) = checked else {
            return Err(PingError::StatusResponse(peer.ip()));
        };
        // The network and the version gate (`validatePeerConfig`).
        let config = checked.message.config.clone().unwrap_or_default();
        let nethash = config
            .network
            .as_ref()
            .and_then(|network| network.nethash.as_deref());
        let network = parts.chain.network();
        if nethash != Some(network.nethash.as_str()) {
            return Err(PingError::VerificationFailed);
        }
        let version = config.version.clone().unwrap_or_default();
        peer.with(|peer| peer.version = Some(version.clone()));
        let (milestone, floor) = (shared.milestone(), &parts.options.minimum_versions);
        if !parts
            .rules
            .is_valid_version(Some(&version), milestone, floor, &network.name)
        {
            return Err(PingError::VerificationFailed);
        }
        if deadline.remaining().is_err() {
            let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            return Err(PingError::PingTimeout { timeout_ms });
        }
        let verifier = &parts.verifier;
        if skip_common_blocks {
            let result = verifier.check_state_fast(peer, &state).await;
            let result = result.map_err(PingError::Stopped)?;
            peer.with(|peer| peer.fast_verification = result);
            if result.is_none() {
                return Err(PingError::VerificationFailed);
            }
        } else {
            let result = verifier.check_state(self, peer, &state, deadline).await;
            let result = result.map_err(PingError::Stopped)?;
            peer.with(|peer| peer.verification = result);
        }
        // The full verification decides here even after a fast one (reproduced).
        if !peer.with(|peer| peer.is_verified()) {
            return Err(PingError::VerificationFailed);
        }
        // Our slot is read now, after the verification, as the reference reads it.
        let now = parts.clock.now_ms();
        let slot = attest
            .then(|| current_slot(parts.chain.milestones(), now))
            .flatten();
        if slot.is_some_and(|slot| slot.slot_number < 0) {
            let wire = u32::try_from(state.current_slot).unwrap_or(0);
            state.current_slot = i64::from(i32::from_ne_bytes(wire.to_ne_bytes()));
        }
        peer.with(|peer| {
            peer.last_pinged = Some(now);
            peer.plugins = config.plugins.clone();
            peer.public_keys.clear();
            peer.stale = false;
            peer.state = Some(state.clone());
        });
        if let Some(slot) = slot {
            shared.attest(peer, &checked, &slot);
        }
        Ok(Some(state))
    }

    async fn ping_ports(&self, peer: &SharedPeer) {
        let plugins: Vec<(String, u32)> = peer.with(|peer| {
            let listed = peer.plugins.iter();
            let listed: Vec<_> = listed
                .map(|(name, plugin)| (name.to_owned(), plugin.and_then(|p| p.port).unwrap_or(0)))
                .collect();
            for (name, _) in &listed {
                peer.ports.insert(name.clone(), -1);
            }
            listed
        });
        // The reference writes the probe's URL as `http://<ip>:<port>/`, which for an IPv6
        // address names another host, so an IPv6 peer's ports always stay -1.
        let ip = peer.ip();
        let probe = |port: u32| async move {
            match (ip, u16::try_from(port)) {
                (IpAddr::V4(_), Ok(port)) if port != 0 => {
                    self.shared.parts.probe.head(ip, port).await == Some(200)
                }
                _ => false,
            }
        };
        let open = join_all(plugins.iter().map(|(_, port)| probe(*port))).await;
        peer.with(|peer| {
            for ((name, port), _) in plugins.into_iter().zip(open).filter(|(_, open)| *open) {
                peer.ports.insert(name, i32::try_from(port).unwrap_or(-1));
            }
        });
    }

    async fn get_peers(&self, peer: &SharedPeer, silent: bool) -> Option<Vec<PeerAddress>> {
        let shared = &self.shared;
        if shared.violator(peer) {
            return None;
        }
        if !silent {
            let address = peer.address();
            heartwood_kernel::debug!("Fetching a fresh peer list from http://{address}");
        }
        let rules = &shared.parts.rules;
        shared
            .emit(
                peer,
                ClientRequest::GetPeers,
                (SHORT, SMALL),
                peer_codecs::get_peers_decode_response,
                |peers| rules.check_peers(&peers),
            )
            .await
    }

    async fn has_common_blocks(
        &self,
        peer: &SharedPeer,
        ids: &[BlockId],
        timeout: Duration,
    ) -> Option<CommonBlock> {
        self.shared.common_blocks(peer, ids, timeout).await
    }

    async fn get_peer_blocks(
        &self,
        peer: &SharedPeer,
        request: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        if self.shared.violator(peer) {
            return Ok(Vec::new());
        }
        self.shared.get_blocks(peer, request).await
    }

    async fn get_unconfirmed_transactions(
        &self,
        peer: &SharedPeer,
        exclude: &[TransactionId],
    ) -> Vec<Vec<u8>> {
        let (shared, parts) = (&self.shared, &self.shared.parts);
        if shared.violator(peer) {
            return Vec::new();
        }
        let most = shared.largest(|params| params.max_transactions().map(u64::from));
        let milestone = shared.milestone();
        let limit = milestone.max_payload().map_or(SMALL, size);
        let exclude = exclude.to_vec();
        let reply = shared
            .emit(
                peer,
                ClientRequest::GetUnconfirmedTransactions { exclude },
                (LONG, limit),
                transactions::get_unconfirmed_transactions_decode_response,
                |reply| parts.rules.check_unconfirmed(&reply, most).map(|()| reply),
            )
            .await;
        // More than the current milestone allows discards the whole reply.
        let allowed = milestone.max_transactions().map_or(usize::MAX, size);
        reply
            .map(|reply| reply.transactions)
            .filter(|transactions| transactions.len() <= allowed)
            .unwrap_or_default()
    }

    async fn post_block(&self, peer: &SharedPeer, block: &Block) {
        let shared = &self.shared;
        let block = serialiser::serialise_with_transactions(block.header(), block.transactions());
        let rules = &shared.parts.rules;
        let height = shared
            .emit(
                peer,
                ClientRequest::PostBlock { block },
                (LONG, SMALL),
                blocks::post_block_decode_response,
                |reply| rules.check_post_block(&reply),
            )
            .await;
        // The reply's height becomes the peer's (reproduced).
        if let Some(height) = height.flatten() {
            peer.with(|peer| {
                peer.state
                    .iter_mut()
                    .for_each(|state| state.height = height)
            });
        }
    }

    fn post_transactions(&self, peer: &SharedPeer, transactions: Arc<Vec<Vec<u8>>>) {
        let shared = &self.shared;
        let mut queues = shared.queues.lock().unwrap_or_else(PoisonError::into_inner);
        let open = queues.get(&peer.ip()).filter(|queue| !queue.is_closed());
        let queue = match open.cloned() {
            Some(queue) => queue,
            None => {
                let (queue, batches) = mpsc::unbounded_channel();
                let task = Arc::clone(shared).run_queue(batches);
                shared.parts.runtime.spawn(task);
                queues.insert(peer.ip(), queue.clone());
                queue
            }
        };
        drop(queues);
        // The task holds the receiver until the queue is dropped, so this cannot fail.
        let _ = queue.send((peer.clone(), transactions));
    }

    fn would_throttle(&self, ip: IpAddr, route: RouteId) -> bool {
        self.shared
            .parts
            .rules
            .would_throttle(ip, route, Instant::now())
    }

    fn disconnect(&self, ip: IpAddr) {
        self.shared.parts.transport.disconnect(ip);
    }

    fn remove_peer(&self, ip: IpAddr) {
        self.shared.remove_peer(ip);
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
        self.shared.common_blocks(peer, ids, timeout).await
    }

    async fn headers(&self, peer: &SharedPeer, from: u32, limit: u32) -> Vec<DownloadedBlock> {
        let request = BlocksRequest {
            block_limit: limit,
            headers_only: true,
            ..BlocksRequest::after(from)
        };
        self.shared
            .get_blocks(peer, request)
            .await
            .unwrap_or_default()
    }
}
