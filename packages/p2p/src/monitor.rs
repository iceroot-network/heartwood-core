//! The network monitor: boot, discovery, cleansing, the network height, fork health, block and
//! transaction downloads, broadcasts and seeds, as the reference's network monitor runs them.
//!
//! - Boot: the seeds, then (unless `skipDiscovery`) the network status with a first discovery and
//!   one `Discovered N peers with v<version>` line per version.
//! - Network status, every 600 s (60 s below `minimumNetworkReach`, after re-seeding): discovery,
//!   then a cleanse when discovery pinged.
//! - Discovery: getPeers from 8 sampled peers, 50 sampled entries of each reply, merged by IP;
//!   every candidate accepted (less verbose) when pinging all, adding all, below the minimum, or
//!   knowing fewer than 3/4 as many peers as they sent.
//! - Cleanse: every peer (or `peerCount` sampled ones) pinged, 2.5 s when fast and
//!   `verifyTimeout` otherwise, as a race: after the delay the cleanse returns and each peer still
//!   pinging gets an infraction; a failed ping disconnects and forgets the peer (`peer.removed`).
//! - Network height: the median of the peers' heights, the element at `floor(n / 2)` of the sorted
//!   heights; 0 without peers.
//! - Fork health: peers counted once per ranked delegate they attest (and relay peers above our
//!   height added when fewer than half the delegates plus one are counted); the distinct common
//!   heights of the forked results sorted as strings and reversed (reproduced); a rollback to the
//!   first height where more peers are forked than ahead of it, at most 5,000 blocks.
//! - Download: chunks of 400 (a tenth after a download that returned nothing), chunk i asked of
//!   the repository's peer i first and then of the other peers in a random order, cached chunks
//!   above a missing one.
//! - Broadcast: a block to every peer (after the rest of 500 ms from a peer's recent block ping),
//!   transactions in batches through each peer's queue.

use std::cmp::Ordering;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::future::join_all;
use heartwood_crypto::{Block, PublicKeyBytes, Transaction, TransactionId};
use heartwood_kernel::{Clock, Event, debug, error, info, warning};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::messages::peer::Plugins;
use heartwood_p2p_wire::replies::PeerAddress;
use indexmap::{IndexMap, IndexSet};
use tokio::time::{sleep, timeout};

use crate::broadcaster::{self, grouped, plural};
use crate::chunk_cache::ChunkCache;
use crate::communicator::BlocksRequest;
use crate::diversity::{Sampler, round_robin};
use crate::download::DownloadedBlock;
use crate::errors::{DownloadError, SeedError};
use crate::options::P2pOptions;
use crate::peer::{Peer, SharedPeer, Verification};
use crate::processor::AcceptOptions;
use crate::repository::PeerRepository;
use crate::seams::{PeerAcceptor, PeerClient, PeerRules};
use crate::seeds;
use crate::view::{BlockPing, ChainView, PeerEvents};

/// The largest rollback a fork check asks for.
pub const MAX_ROLLBACK: u32 = 5_000;
/// The chunk size of a download that follows one that returned blocks.
pub const DEFAULT_CHUNK_SIZE: u32 = 400;
/// The ping delay of a fast cleanse.
pub const FAST_PING: Duration = Duration::from_millis(2_500);
/// How many peers discovery asks for their peers.
pub const DISCOVERY_SOURCES: usize = 8;
/// How many entries of each getPeers reply discovery takes.
pub const PEERS_PER_REPLY: usize = 50;
/// How many peers the transaction download asks, in turn.
pub const TRANSACTION_SOURCES: usize = 5;
/// How long a downloaded transaction id is remembered, in seconds.
pub const TRANSACTION_MEMORY_S: i64 = 30;
/// The shortest time between two `pingAll` cleanses, in milliseconds.
pub const PING_ALL_INTERVAL_MS: i64 = 10_000;
/// How long after a peer's first block ping the block is broadcast, in milliseconds.
pub const BLOCK_PING_WAIT_MS: i64 = 500;
/// The network status cadence.
pub const STATUS_INTERVAL: Duration = Duration::from_secs(600);
/// The network status cadence below `minimumNetworkReach`, after re-seeding.
pub const LOW_REACH_STATUS_INTERVAL: Duration = Duration::from_secs(60);
/// The wait before a fork check, and before the `pingAll` of a finished sync.
pub const SETTLE_DELAY: Duration = Duration::from_secs(1);
/// The timeout of a single block's download.
pub const SINGLE_BLOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// How a cleanse runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanseOptions {
    /// Ping with 2.5 s instead of `verifyTimeout`.
    pub fast: bool,
    /// Ping peers that were pinged recently too.
    pub force_ping: bool,
    /// Log the "Checking N peers" line.
    pub log: bool,
    /// Ping only this many sampled peers.
    pub peer_count: Option<usize>,
    /// Verify fast, without common blocks.
    pub skip_common_blocks: bool,
}

impl Default for CleanseOptions {
    fn default() -> CleanseOptions {
        CleanseOptions {
            fast: false,
            force_ping: false,
            log: true,
            peer_count: None,
            skip_common_blocks: false,
        }
    }
}

impl CleanseOptions {
    /// The fast forced cleanse without common blocks of `pingAll` and of the first idle entry.
    pub const PING_ALL: CleanseOptions = CleanseOptions {
        fast: true,
        force_ping: true,
        log: false,
        peer_count: None,
        skip_common_blocks: true,
    };
}

/// The result of a fork health check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkStatus {
    /// Whether the network is on another chain.
    pub forked: bool,
    /// How many blocks to roll back, when forked.
    pub blocks_to_rollback: Option<u32>,
}

/// A peer as the API lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerInfo {
    /// The address.
    pub ip: IpAddr,
    /// The peer-to-peer port.
    pub port: u16,
    /// The port probe's result per plugin.
    pub ports: IndexMap<String, i32>,
    /// The version the peer reported.
    pub version: Option<String>,
    /// The height of the peer's last state.
    pub height: Option<u32>,
    /// The time of the last successful request, in milliseconds.
    pub latency: Option<u32>,
    /// The plugins of the peer's last status.
    pub plugins: Plugins,
}

/// What the fork decision reads of a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForkPeer {
    /// How many active delegates the peer's attested keys name ([`active_count`]).
    pub active_delegates: usize,
    /// The height of the peer's last state, when it has one.
    pub height: Option<u32>,
    /// The result of the peer's last full verification.
    pub verification: Option<Verification>,
}

/// What a monitor is made of.
pub struct MonitorParts<C, A, R> {
    /// The requests to peers.
    pub client: Arc<C>,
    /// The acceptance of candidates.
    pub acceptor: Arc<A>,
    /// The peer rules.
    pub rules: Arc<R>,
    /// The peers.
    pub repository: Arc<PeerRepository>,
    /// The node's chain.
    pub chain: Arc<dyn ChainView>,
    /// Where peer events go.
    pub events: Arc<dyn PeerEvents>,
    /// The p2p options.
    pub options: Arc<P2pOptions>,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
    /// The random samples.
    pub sampler: Sampler,
    /// The runtime the background work runs on (the network status, the cleanse's pings, the
    /// port probes, `pingAll`), so the monitor's methods can be called from any thread.
    pub runtime: tokio::runtime::Handle,
}

/// How discovery accepts a candidate.
const CANDIDATE: AcceptOptions = AcceptOptions {
    seed: false,
    less_verbose: true,
};

/// How seeding accepts a seed.
const SEED: AcceptOptions = AcceptOptions {
    seed: true,
    less_verbose: true,
};

/// What the monitor keeps between calls.
#[derive(Debug, Default)]
struct State {
    /// The size of the next download's chunks.
    chunk_size: u32,
    /// Chunks downloaded above a chunk that failed.
    cache: ChunkCache,
    /// Whether the boot has not finished.
    initialising: bool,
    /// Whether the node started a new network and has not completed its cold start.
    cold_start: bool,
    /// When `pingAll` last cleansed, in milliseconds.
    last_pinged_ms: i64,
    /// The ids of downloaded transactions, with the second they were first seen.
    seen: IndexMap<TransactionId, i64>,
    /// The seeds: `peers.json`'s list, grown by its sources.
    seeds: Vec<PeerAddress>,
}

/// What a cleanse's pings report while it waits.
#[derive(Debug, Default)]
struct Tally {
    /// The peers whose ping has not finished.
    pinging: Vec<SharedPeer>,
    /// The failed pings, by error text.
    errors: IndexMap<String, usize>,
    /// How many pings failed.
    failed: usize,
}

/// The network monitor.
pub struct NetworkMonitor<C, A, R> {
    /// The parts.
    p: MonitorParts<C, A, R>,
    state: Mutex<State>,
    /// Whether the next network status is scheduled.
    scheduled: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn bump(count: &mut usize) {
    *count = count.saturating_add(1);
}

/// The key of a chunk from height `from`: `[<from>, <to>]`, or `[<from>, ..]` for the last one.
fn range_key(from: u64, to: Option<u64>) -> String {
    let to = to.map_or_else(|| "..".to_owned(), grouped);
    format!("[{}, {to}]", grouped(from))
}

impl<C, A, R> NetworkMonitor<C, A, R>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    /// A monitor of `parts`.
    pub fn new(parts: MonitorParts<C, A, R>) -> NetworkMonitor<C, A, R> {
        let seeds = parts.options.seeds.clone();
        NetworkMonitor {
            p: parts,
            state: Mutex::new(State {
                chunk_size: DEFAULT_CHUNK_SIZE,
                initialising: true,
                seeds,
                ..State::default()
            }),
            scheduled: AtomicBool::new(false),
        }
    }

    fn state<T>(&self, use_it: impl FnOnce(&mut State) -> T) -> T {
        use_it(&mut lock(&self.state))
    }

    /// `peers` without those with a protocol violation in the window: no source of blocks,
    /// transactions or peers.
    fn sources(&self, mut peers: Vec<SharedPeer>) -> Vec<SharedPeer> {
        let now = self.p.clock.now_ms();
        peers.retain(|peer| !peer.with(|p| p.has_violation(now)));
        peers
    }

    /// The repository's peers.
    fn all(&self) -> Vec<SharedPeer> {
        self.p.repository.peers()
    }

    /// At most `count` of `peers`, sampled across subnets.
    fn sample(&self, peers: Vec<SharedPeer>, count: usize) -> Vec<SharedPeer> {
        self.p.sampler.diverse(peers, count, SharedPeer::ip)
    }

    /// Whether a request on `route` to `peer` would wait for the rate limiter now.
    fn throttled(&self, peer: &SharedPeer, route: RouteId) -> bool {
        self.p.client.would_throttle(peer.ip(), route)
    }

    /// The size of the next download's chunks.
    pub fn chunk_size(&self) -> u32 {
        self.state(|s| s.chunk_size)
    }

    /// The keys of the cached chunks, oldest first.
    pub fn cached_chunks(&self) -> Vec<String> {
        self.state(|s| s.cache.keys().map(str::to_owned).collect())
    }

    /// Whether the node started a new network (`networkStart`) and has not completed it.
    pub fn is_cold_start(&self) -> bool {
        self.state(|s| s.cold_start)
    }

    /// The end of the cold start.
    pub fn complete_cold_start(&self) {
        self.state(|s| s.cold_start = false);
    }

    /// Boot: an HTTPS source is refused, the seeds are accepted, then the first network status
    /// runs unless `skipDiscovery`, with one `Discovered N peers with v<version>` line per version.
    /// The network status then schedules itself. The DNS and NTP checks are not built.
    pub async fn boot(self: &Arc<Self>) -> Result<(), SeedError> {
        seeds::check_sources(&self.p.options.sources)?;
        self.populate_seed_peers().await?;
        if self.p.options.skip_discovery {
            warning!(
                "Skipped peer discovery because the relay is in skip-discovery mode :see_no_evil:"
            );
        } else {
            self.update_network_status(true).await;
            let mut versions: IndexMap<String, usize> = IndexMap::new();
            for peer in self.all() {
                let version = peer
                    .with(|p| p.version.clone())
                    .unwrap_or("undefined".into());
                bump(versions.entry(version).or_default());
            }
            let ours = &self.p.options.version;
            for (version, count) in versions {
                let mark = match self.p.rules.compare_versions(&version, ours) {
                    Some(Ordering::Greater) => " :eyes:",
                    Some(Ordering::Less) => " :zzz:",
                    _ => "",
                };
                info!("Discovered {} with v{version}{mark}", plural("peer", count));
            }
        }
        self.state(|s| s.initialising = false);
        Ok(())
    }

    /// Discovery and a cleanse, then the next run scheduled: in 600 s, or in 60 s after re-seeding
    /// when below the minimum. With `disableDiscovery` nothing runs, and nothing is scheduled.
    pub async fn update_network_status(self: &Arc<Self>, initial: bool) {
        if self.p.options.network_start {
            self.state(|s| s.cold_start = true);
            warning!("Entering cold start because the relay is in genesis-start mode :snowflake:");
        }
        if self.p.options.disable_discovery {
            warning!("Skipped peer discovery because the relay is in non-discovery mode :mag:");
            return;
        }
        if self.discover_peers(initial, false, false).await {
            self.cleanse_peers(CleanseOptions::default()).await;
        }
        let mut next = STATUS_INTERVAL;
        if !self.has_minimum_peers(false) {
            // Only a boot without seeds fails here, and that boot stops the node.
            let _ = self.populate_seed_peers().await;
            next = LOW_REACH_STATUS_INTERVAL;
            info!("Couldn't find enough peers. Falling back to seed peers :fire:");
        }
        self.schedule_network_status(next);
    }

    /// Run the network status again after `after`, unless a run is scheduled already.
    fn schedule_network_status(self: &Arc<Self>, after: Duration) {
        if !self.scheduled.swap(true, SeqCst) {
            let monitor = Arc::clone(self);
            self.p.runtime.spawn(async move {
                sleep(after).await;
                monitor.scheduled.store(false, SeqCst);
                monitor.update_network_status(false).await;
            });
        }
    }

    /// Ping the peers as `options` asks, as a race against the ping delay (2.5 s when fast,
    /// `verifyTimeout` otherwise): the cleanse returns when every ping has finished or the delay
    /// has passed, whichever is first, and each peer still pinging then gets an infraction; the
    /// pings go on. A failed ping disconnects and forgets the peer (`peer.removed`).
    pub async fn cleanse_peers(&self, options: CleanseOptions) {
        let (mut peers, verify) = (self.all(), self.p.options.verify_timeout);
        let mut max = peers.len();
        let delay = if options.fast { FAST_PING } else { verify };
        if let Some(count) = options.peer_count.filter(|&count| count > 0) {
            peers = self.sample(peers, count);
            max = peers.len().min(count);
        }
        if options.log {
            info!("Checking {} :telescope:", plural("peer", max));
        }
        let tally = Arc::new(Mutex::new(Tally::default()));
        lock(&tally).pinging.clone_from(&peers);
        let pings: Vec<_> = peers
            .into_iter()
            .map(|peer| {
                let (client, events) = (Arc::clone(&self.p.client), Arc::clone(&self.p.events));
                let tally = Arc::clone(&tally);
                self.p.runtime.spawn(async move {
                    let (force, skip) = (options.force_ping, options.skip_common_blocks);
                    let pinged = client.ping(&peer, delay, true, force, skip).await;
                    let mut counts = lock(&tally);
                    counts.pinging.retain(|other| !other.same(&peer));
                    if let Err(failure) = pinged {
                        bump(&mut counts.failed);
                        bump(counts.errors.entry(failure.to_string()).or_default());
                        drop(counts);
                        client.remove_peer(peer.ip());
                        events.emit(Event::PeerRemoved);
                    }
                })
            })
            .collect();
        // Past the delay the pings run on in the background.
        let _ = timeout(delay, join_all(pings)).await;
        let now = self.p.clock.now_ms();
        let counts = lock(&tally);
        for peer in &counts.pinging {
            peer.with(|p| p.add_infraction(now));
        }
        for (failure, count) in &counts.errors {
            let count = plural("peer", *count);
            debug!("Removed {count} because of \"{failure}\" :wastebasket:");
        }
        let responsive = max.saturating_sub(counts.failed);
        drop(counts);
        if self.state(|s| s.initialising) {
            let (max, height) = (plural("peer", max), grouped(self.network_height().into()));
            info!("{responsive} of {max} on the network are responsive");
            info!("Median Network Height: {height}");
        }
    }

    /// Discovery: getPeers from 8 sampled peers (those that would be throttled left out when
    /// `add_all`), 50 sampled entries of each reply, merged by IP. When pinging all, adding all,
    /// below the minimum, or knowing fewer than 3/4 as many peers as they sent, every candidate
    /// goes through acceptance (less verbose) and the ports of all peers (or half, sampled) are
    /// probed in the background; whether it did so.
    pub async fn discover_peers(&self, ping_all: bool, add_all: bool, silent: bool) -> bool {
        let own = self.all();
        let mut to_try = self.sources(own.clone());
        if add_all {
            to_try.retain(|peer| !self.throttled(peer, RouteId::GetPeers));
        }
        let client = Arc::clone(&self.p.client);
        let asked = self.sample(to_try, DISCOVERY_SOURCES);
        let replies = join_all(asked.iter().map(|peer| client.get_peers(peer, silent))).await;
        let mut candidates: IndexMap<String, PeerAddress> = IndexMap::new();
        for reply in replies {
            let sampled = self.p.sampler.shuffle(reply.unwrap_or_default());
            let mut one: IndexMap<String, PeerAddress> = IndexMap::new();
            for peer in sampled.into_iter().take(PEERS_PER_REPLY) {
                one.insert(peer.ip.clone(), peer);
            }
            candidates.extend(one);
        }
        let too_few = own.len().saturating_mul(4) < candidates.len().saturating_mul(3);
        let accept = ping_all || add_all || !self.has_minimum_peers(false) || too_few;
        if accept {
            let accept_one = |c| self.p.acceptor.validate_and_accept(c, CANDIDATE);
            join_all(candidates.into_values().map(accept_one)).await;
        }
        let mut peers = self.all();
        if !(accept && ping_all) {
            peers = self.sample(peers.clone(), peers.len() / 2);
        }
        if !silent {
            debug!("Checking ports of {}", plural("peer", peers.len()));
        }
        let probes = async move { join_all(peers.iter().map(|p| client.ping_ports(p))).await };
        self.p.runtime.spawn(probes);
        accept
    }

    /// The median of the peers' heights ([`median_height`]).
    pub fn network_height(&self) -> u32 {
        let peers = self.all();
        median_height(peers.iter().map(|peer| peer.with(|p| p.height())))
    }

    /// The fork health check: a discovery that pings all first unless `fast`, a forced cleanse,
    /// then [`fork_decision`] over the peers.
    pub async fn check_network_health(&self, fast: bool) -> NetworkStatus {
        if !fast {
            self.discover_peers(true, false, false).await;
        }
        let mut forced = CleanseOptions::default();
        (forced.fast, forced.force_ping) = (fast, true);
        self.cleanse_peers(forced).await;
        let chain = &self.p.chain;
        let height = chain.last_block().height();
        let active = chain.milestones().at(height).active_delegates();
        let rank_of = |key: &PublicKeyBytes| chain.delegate_by_key(key).and_then(|d| d.rank);
        let fork_peer = |p: &mut Peer| ForkPeer {
            active_delegates: active_count(&p.public_keys, rank_of, active),
            height: p.height(),
            verification: p.verification,
        };
        let peers: Vec<ForkPeer> = self.all().iter().map(|p| p.with(fork_peer)).collect();
        fork_decision(&peers, height, active)
    }

    /// The fork check of the blockchain: 0 without peers; else, after 1 s, a fast health check and
    /// a full one when the fast one saw no fork; the blocks to roll back.
    pub async fn check_for_fork(&self) -> u32 {
        if self.p.repository.is_empty() {
            return 0;
        }
        sleep(SETTLE_DELAY).await;
        let mut status = self.check_network_health(true).await;
        if !status.forked {
            status = self.check_network_health(false).await;
        }
        let rollback = status.blocks_to_rollback;
        rollback.filter(|_| status.forked).unwrap_or(0)
    }

    /// The block at `height` from the peer at `ip`, if it is a peer and a source (one block, 2 s,
    /// silent).
    pub async fn download_block_at_height(
        &self,
        ip: IpAddr,
        height: u32,
    ) -> Result<Option<DownloadedBlock>, DownloadError> {
        let peer = self.p.repository.get(ip).into_iter().collect();
        let Some(peer) = self.sources(peer).pop() else {
            return Ok(None);
        };
        let request = BlocksRequest {
            block_limit: 1,
            silent: true,
            timeout: Some(SINGLE_BLOCK_TIMEOUT),
            ..BlocksRequest::after(height.saturating_sub(1))
        };
        let blocks = self.p.client.get_peer_blocks(&peer, request).await?;
        Ok(blocks.into_iter().next())
    }

    /// The blocks after `from`, downloaded in parallel chunks: an unbroken run of heights from
    /// `from + 1`, up to the first chunk no peer gave.
    ///
    /// `min(ceil((network height - from) / size) or 1, peers, max_parallel)` chunks of the current
    /// size (400, or a tenth of the last size after a download that returned nothing); chunk `i`
    /// is asked of the repository's peer `i` first, then of the peers after the first chunks'
    /// peers in a sampled order (of every other peer when there are none). A cached chunk is used
    /// and removed; chunks above a missing one are cached. The last chunk may be empty. Each
    /// getBlocks has `timeout`, or the configured `getBlocksTimeout` when `None`.
    pub async fn download_blocks_from_height(
        &self,
        from: u32,
        max_parallel: usize,
        silent: bool,
        timeout: Option<Duration>,
        check_throttle: bool,
    ) -> Vec<DownloadedBlock> {
        let all = self.all();
        if all.is_empty() {
            if !silent {
                error!("Could not download blocks: we have 0 peers :bangbang:");
            }
            return Vec::new();
        }
        let mut peers = self.sources(all);
        if check_throttle {
            peers.retain(|peer| !self.throttled(peer, RouteId::GetBlocks));
        }
        let (network, size, from) = (self.network_height(), self.chunk_size(), u64::from(from));
        let behind = u64::from(network).saturating_sub(from);
        let missing = usize::try_from(behind.div_ceil(size.max(1).into())).unwrap_or(usize::MAX);
        let chunks = missing.max(1).min(peers.len()).min(max_parallel);
        let start = |i: usize| from.saturating_add(u64::from(size).saturating_mul(i as u64));
        let end = |i: usize| start(i).saturating_add(size.into());
        let (failed, mut jobs, mut names) = (AtomicBool::new(false), Vec::new(), Vec::new());
        for (i, first) in peers.iter().take(chunks).enumerate() {
            let last = i.saturating_add(1) == chunks;
            let key = range_key(start(i).saturating_add(1), (!last).then(|| end(i)));
            names.push(key.clone());
            let cached = self.state(|s| s.cache.take(&key));
            let mut to_try = Vec::new();
            if cached.is_none() {
                let backups = peers.get(chunks..).unwrap_or_default().to_vec();
                to_try = [vec![first.clone()], self.sample(backups, peers.len())].concat();
                if to_try.len() == 1 {
                    to_try.extend(peers.iter().filter(|p| p.ip() != first.ip()).cloned());
                }
            }
            let request = BlocksRequest {
                block_limit: size,
                silent,
                timeout,
                ..BlocksRequest::after(u32::try_from(start(i)).unwrap_or(u32::MAX))
            };
            jobs.push(self.download_chunk(cached, key, to_try, request, last, &failed));
        }
        if !silent {
            debug!("Downloading blocks in chunks: {}", names.join(", "));
        }
        let (mut downloaded, mut results) = (Vec::new(), join_all(jobs).await.into_iter());
        for result in results.by_ref() {
            match result {
                Ok(blocks) => downloaded.extend(blocks),
                Err(message) => {
                    if !silent {
                        error!("{message} :exclamation:");
                    }
                    break;
                }
            }
        }
        let skipped = chunks.saturating_sub(results.len());
        self.state(|s| {
            for (i, blocks) in (skipped..)
                .zip(results)
                .filter_map(|(i, r)| Some((i, r.ok()?)))
            {
                let key = range_key(start(i).saturating_add(1), Some(end(i)));
                s.cache.set(key, blocks);
            }
            let (tenth, full) = (s.chunk_size.div_ceil(10), DEFAULT_CHUNK_SIZE);
            s.chunk_size = if downloaded.is_empty() { tenth } else { full };
        });
        downloaded
    }

    /// One chunk: the cached blocks, or asked of `peers` in turn until one gives blocks (or, for
    /// the last chunk, any answer); the blocks, or the reference's failure message.
    async fn download_chunk(
        &self,
        cached: Option<Vec<DownloadedBlock>>,
        key: String,
        peers: Vec<SharedPeer>,
        request: BlocksRequest,
        last: bool,
        failed: &AtomicBool,
    ) -> Result<Vec<DownloadedBlock>, String> {
        if let Some(blocks) = cached {
            return Ok(blocks);
        }
        let (mut returned, mut printed, log) = (false, String::new(), !request.silent);
        for peer in &peers {
            printed = format!("{}:{}", peer.ip(), peer.port());
            let why = match self.p.client.get_peer_blocks(peer, request).await {
                Ok(blocks) if !blocks.is_empty() || last => {
                    if log {
                        debug!("Downloaded blocks {key} ({}) from {printed}", blocks.len());
                    }
                    return Ok(blocks);
                }
                Ok(_) => {
                    returned = true;
                    "Peer did not return any block".to_owned()
                }
                Err(fault) => fault.to_string(),
            };
            if log {
                info!("Failed to download blocks {key} from {printed}: {why} :bangbang:");
                if failed.load(SeqCst) {
                    let why = "another download job failed";
                    info!("Giving up on trying to download blocks {key}: {why} :bangbang:");
                }
            }
        }
        failed.store(true, SeqCst);
        if !returned {
            // The reference reads the length of a reply it never had.
            return Err("Cannot read properties of undefined (reading 'length')".to_owned());
        }
        let tried = peers.len();
        Err(format!(
            "Could not download blocks {key} from any of {tried} peer(s). Last attempt returned 0 \
             block(s) from peer {printed}"
        ))
    }

    /// The pool transactions of up to 5 sampled peers, in turn, without `exclude`; the ids seen in
    /// the last 30 s are dropped, and the rest remembered. The throttle's answers are matched to
    /// the sampled order by position, not by peer (reproduced).
    pub async fn download_transactions(&self, exclude: &[TransactionId]) -> Vec<Vec<u8>> {
        let peers = self.all();
        let route = RouteId::GetUnconfirmedTransactions;
        let throttled: Vec<bool> = peers.iter().map(|p| self.throttled(p, route)).collect();
        let shuffled = self.p.sampler.shuffle(peers).into_iter().zip(throttled);
        let open = shuffled.filter(|(_, t)| !t).map(|(peer, _)| peer).collect();
        let asked = round_robin(self.sources(open), TRANSACTION_SOURCES, SharedPeer::ip);
        let (client, mut downloaded) = (&self.p.client, IndexSet::new());
        for peer in asked {
            downloaded.extend(client.get_unconfirmed_transactions(&peer, exclude).await);
        }
        let now_s = self.p.clock.now_ms().saturating_add(999).div_euclid(1_000);
        let fresh = |seen: &mut i64| now_s.saturating_sub(*seen) <= TRANSACTION_MEMORY_S;
        self.state(|s| s.seen.retain(|_, seen| fresh(seen)));
        let height = self.p.chain.last_block().height();
        let mut accepted = Vec::new();
        for bytes in downloaded {
            let params = self.p.chain.milestones().at(height);
            if let Ok(transaction) = Transaction::decode_trusted(&bytes, params) {
                let id = *transaction.id();
                if self.state(|s| !s.seen.contains_key(&id) && s.seen.insert(id, now_s).is_none()) {
                    accepted.push(transaction.bytes().to_vec());
                }
            }
            sleep(Duration::from_millis(1)).await;
        }
        accepted
    }

    /// Broadcast `block` to every peer. A block whose ping came from a peer less than 500 ms
    /// before its last one waits for the rest of the 500 ms, and is not broadcast when the ping
    /// has moved to another height by then.
    pub async fn broadcast_block(&self, block: &Block) {
        let (peers, chain, at) = (self.all(), &self.p.chain, block.height());
        let own = |ping: &BlockPing| ping.id == *block.id() && !ping.from_forger;
        if let Some(ping) = chain.block_ping().filter(own) {
            let seen = ping.last_ms.saturating_sub(ping.first_ms);
            if seen < BLOCK_PING_WAIT_MS {
                let wait = u64::try_from(BLOCK_PING_WAIT_MS.saturating_sub(seen)).unwrap_or(0);
                sleep(Duration::from_millis(wait)).await;
                if chain.block_ping().is_none_or(|ping| ping.height != at) {
                    return;
                }
            }
        }
        let (height, count) = (grouped(at.into()), plural("peer", peers.len()));
        info!("Broadcasting block {height} to {count} :satellite_antenna:");
        let client = &self.p.client;
        join_all(peers.iter().map(|peer| client.post_block(peer, block))).await;
    }

    /// Broadcast `transactions` to every peer, in batches of `maxTransactionsPerRequest`.
    pub fn broadcast_transactions(&self, transactions: &[Transaction]) {
        let (peers, batch) = (self.all(), self.p.options.max_transactions_per_request);
        broadcaster::broadcast_transactions(self.p.client.as_ref(), &peers, transactions, batch);
    }

    /// Whether the node knows `minimumNetworkReach` peers (always with
    /// `ignoreMinimumNetworkReach`, with a warning unless `silent`).
    pub fn has_minimum_peers(&self, silent: bool) -> bool {
        if self.p.options.ignore_minimum_network_reach {
            if !silent {
                warning!(
                    "Ignored the minimum network reach because the relay is in seed mode :exclamation:"
                );
            }
            return true;
        }
        self.p.repository.len() >= self.p.options.minimum_network_reach
    }

    /// Forget each seed and accept it again (as a seed, less verbose): `peers.json`'s list, grown
    /// for good by the peers of its first source that it lacks. A source that cannot be read adds
    /// nothing; [`SeedError::NoSeeds`] stops the node.
    pub async fn populate_seed_peers(&self) -> Result<(), SeedError> {
        let from_sources = seeds::load_sources(&self.p.options.sources)
            .await
            .unwrap_or_default();
        let list = self.state(|s| {
            s.seeds = seeds::merge_seeds(&s.seeds, &from_sources);
            s.seeds.clone()
        });
        if list.is_empty() {
            return Err(SeedError::NoSeeds);
        }
        let accepts: Vec<_> = (list.into_iter())
            .map(|seed| {
                if let Ok(ip) = seed.ip.parse() {
                    self.p.repository.forget(ip);
                }
                self.p.acceptor.validate_and_accept(seed, SEED)
            })
            .collect();
        join_all(accepts).await;
        Ok(())
    }

    /// A fast forced cleanse without common blocks, at most every 10 s, in the background. The
    /// node calls it after each applied round while synced, and through [`Self::synced`].
    pub fn ping_all(self: &Arc<Self>) {
        let now = self.p.clock.now_ms();
        let due = self.state(|s| {
            let due = now.saturating_sub(s.last_pinged_ms) > PING_ALL_INTERVAL_MS;
            if due {
                s.last_pinged_ms = now;
            }
            due
        });
        if due {
            let monitor = Arc::clone(self);
            let cleanse = async move { monitor.cleanse_peers(CleanseOptions::PING_ALL).await };
            self.p.runtime.spawn(cleanse);
        }
    }

    /// The `blockchain.synced` listener: [`Self::ping_all`] after 1 s, in the background.
    pub fn synced(self: &Arc<Self>) {
        let monitor = Arc::clone(self);
        self.p.runtime.spawn(async move {
            sleep(SETTLE_DELAY).await;
            monitor.ping_all();
        });
    }

    /// A forced cleanse after a fork recovery.
    pub async fn refresh_peers_after_fork(&self) {
        let count = plural("peer", self.p.repository.len());
        info!("Refreshing {count} after fork :fork_and_knife:");
        self.cleanse_peers(CleanseOptions {
            force_ping: true,
            ..CleanseOptions::default()
        })
        .await;
    }

    /// Disconnect the peers whose version no longer passes the version gate (a milestone change).
    pub fn disconnect_invalid_peers(&self) {
        let (chain, options) = (&self.p.chain, &self.p.options);
        let milestone = chain.milestones().at(chain.last_block().height());
        for peer in self.all() {
            let version = peer.with(|p| p.version.clone());
            let (ranges, name) = (&options.minimum_versions, &chain.network().name);
            if !self
                .p
                .rules
                .is_valid_version(version.as_deref(), milestone, ranges, name)
            {
                self.p.client.remove_peer(peer.ip());
            }
        }
    }

    /// How many peers the node knows: the repository's size, which the reference's `hasPeers`
    /// reads.
    pub fn peer_count(&self) -> usize {
        self.p.repository.len()
    }

    /// The peers, as the API lists them.
    pub fn peers(&self) -> Vec<PeerInfo> {
        let info = |p: &mut crate::peer::Peer| PeerInfo {
            ip: p.ip,
            port: p.port,
            ports: p.ports.clone(),
            version: p.version.clone(),
            height: p.height(),
            latency: p.latency,
            plugins: p.plugins.clone(),
        };
        self.p
            .repository
            .peers()
            .iter()
            .map(|peer| peer.with(info))
            .collect()
    }
}

/// The median of `heights`: the element at `floor(n / 2)` of the known, nonzero heights sorted;
/// 0 without one.
pub fn median_height(heights: impl IntoIterator<Item = Option<u32>>) -> u32 {
    let mut known: Vec<u32> = heights.into_iter().flatten().filter(|&h| h != 0).collect();
    known.sort_unstable();
    known.get(known.len() / 2).copied().unwrap_or(0)
}

/// How many of `keys` name an active delegate: a key whose delegate has a rank of at most
/// `active_delegates`.
pub fn active_count(
    keys: &[PublicKeyBytes],
    rank_of: impl Fn(&PublicKeyBytes) -> Option<u32>,
    active_delegates: u64,
) -> usize {
    let active = |key: &&PublicKeyBytes| {
        rank_of(key).is_some_and(|rank| u64::from(rank) <= active_delegates)
    };
    keys.iter().filter(active).count()
}

/// The fork decision of `checkNetworkHealth` over `peers`, with our height and the milestone's
/// active delegates.
///
/// Each peer counts once per active delegate it attests; when fewer than half the delegates plus
/// one are counted, the peers above our height that attest none are added, and with still too
/// few, or no verified peer, there is no fork. Otherwise the distinct common heights of the forked
/// results are sorted as text and reversed (reproduced), and the first height where more results
/// are forked there than above it is the fork: a rollback to it, of at most 5,000 blocks, and no
/// fork when it is not below us.
pub fn fork_decision(peers: &[ForkPeer], our_height: u32, active_delegates: u64) -> NetworkStatus {
    let copies = |p: &ForkPeer| std::iter::repeat_n(*p, p.active_delegates);
    let mut included: Vec<ForkPeer> = peers.iter().flat_map(copies).collect();
    let relay = |p: &&ForkPeer| p.active_delegates == 0 && p.height.is_some_and(|h| h > our_height);
    let half_plus_one =
        usize::try_from(active_delegates / 2).map_or(usize::MAX, |n| n.saturating_add(1));
    if included.len() < half_plus_one {
        included.extend(peers.iter().filter(relay));
    }
    if included.len() < half_plus_one {
        info!("Not enough peers available to check network health :no_entry_sign:");
        return NetworkStatus::default();
    }
    let results: Vec<Verification> = included.iter().filter_map(|p| p.verification).collect();
    if results.is_empty() {
        info!("No verified peers available to check network health :no_entry_sign:");
        return NetworkStatus::default();
    }
    let forked: Vec<u32> = results
        .iter()
        .filter(|r| r.forked())
        .map(|r| r.highest_common_height)
        .collect();
    let mut heights = forked.clone();
    heights.sort_by_key(u32::to_string);
    heights.dedup();
    for height in heights.into_iter().rev() {
        let fork_count = forked.iter().filter(|&&common| common == height).count() as u64;
        let above = results
            .iter()
            .filter(|r| r.highest_common_height > height)
            .count() as u64;
        let (at, counts) = (
            grouped(height.into()),
            format!("{} vs {}", grouped(above), grouped(fork_count)),
        );
        if fork_count <= above {
            debug!("Ignoring fork at height {at} ({counts}) :fork_and_knife:");
            continue;
        }
        let rollback = i64::from(our_height).saturating_sub(i64::from(height));
        let blocks = u32::try_from(rollback).unwrap_or(0).min(MAX_ROLLBACK);
        let rolled = match rollback > i64::from(MAX_ROLLBACK) {
            true => format!(
                "{}/{} blocks",
                grouped(MAX_ROLLBACK.into()),
                grouped(rollback.unsigned_abs())
            ),
            false => plural("block", blocks as usize),
        };
        if rollback > 0 {
            warning!(
                "Fork detected - rolling back {rolled} to fork at height {at} ({counts}) :repeat:"
            );
        }
        return NetworkStatus {
            forked: rollback > 0,
            blocks_to_rollback: (rollback > 0).then_some(blocks),
        };
    }
    NetworkStatus::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verified(common: u32, mine: u32, theirs: u32) -> Option<Verification> {
        Some(Verification {
            my_height: mine,
            their_height: theirs,
            highest_common_height: common,
        })
    }

    fn peer(active: usize, height: u32, verification: Option<Verification>) -> ForkPeer {
        ForkPeer {
            active_delegates: active,
            height: Some(height),
            verification,
        }
    }

    #[test]
    fn the_median_is_at_half_the_known_heights() {
        assert_eq!(median_height([]), 0);
        assert_eq!(median_height([None, Some(0)]), 0);
        assert_eq!(median_height([Some(5)]), 5);
        assert_eq!(median_height([Some(9), Some(3)]), 9);
        assert_eq!(median_height([Some(9), None, Some(3), Some(7)]), 7);
        assert_eq!(median_height([Some(4), Some(1), Some(3), Some(2)]), 3);
    }

    #[test]
    fn fork_heights_are_sorted_as_text() {
        // Common heights 9 and 100 below our 120. As text "9" sorts after "100", so reversed it
        // is tried first: 20 results forked there against 10 above it, a rollback of 111 blocks.
        // Sorted as numbers, 100 would be tried first and win 10 to 0, a rollback of 20.
        let peers = [
            peer(20, 130, verified(9, 120, 130)),
            peer(10, 130, verified(100, 120, 130)),
        ];
        assert_eq!(
            fork_decision(&peers, 120, 53),
            NetworkStatus {
                forked: true,
                blocks_to_rollback: Some(111)
            }
        );
    }

    #[test]
    fn too_few_attestations_or_verifications_are_no_fork() {
        let few = [peer(26, 130, verified(100, 120, 130))];
        assert_eq!(fork_decision(&few, 120, 53), NetworkStatus::default());
        // Relay peers above us fill the count.
        let filled = [
            peer(26, 130, verified(100, 120, 130)),
            peer(0, 121, verified(100, 120, 121)),
        ];
        assert!(fork_decision(&filled, 120, 53).forked);
        // A relay at our height does not.
        let level = [peer(26, 130, None), peer(0, 120, verified(100, 120, 120))];
        assert_eq!(fork_decision(&level, 120, 53), NetworkStatus::default());
        let unverified = [peer(27, 130, None)];
        assert_eq!(
            fork_decision(&unverified, 120, 53),
            NetworkStatus::default()
        );
    }

    #[test]
    fn rollbacks_are_capped_and_never_upward() {
        let deep = [peer(30, 9_000, verified(10, 9_000, 9_100))];
        assert_eq!(
            fork_decision(&deep, 9_000, 53).blocks_to_rollback,
            Some(MAX_ROLLBACK)
        );
        // A fork above our height: no rollback, no fork.
        let above = [peer(30, 200, verified(150, 140, 200))];
        assert_eq!(fork_decision(&above, 140, 53), NetworkStatus::default());
        // More peers above the fork height than on it: ignored.
        let ignored = [
            peer(10, 130, verified(100, 120, 130)),
            peer(20, 130, verified(120, 120, 130)),
        ];
        assert_eq!(fork_decision(&ignored, 120, 53), NetworkStatus::default());
    }

    #[test]
    fn active_delegates_are_ranked_at_most_the_milestones_count() {
        let key = |n: u8| PublicKeyBytes::from_array([n; 33]);
        let keys = [key(1), key(2), key(3), key(4)];
        let rank_of = |k: &PublicKeyBytes| match k.to_hex().get(..2) {
            Some("01") => Some(1),
            Some("02") => Some(53),
            Some("03") => Some(54),
            _ => None,
        };
        assert_eq!(active_count(&keys, rank_of, 53), 2);
    }

    #[test]
    fn chunk_keys_are_written_as_the_reference_writes_them() {
        assert_eq!(range_key(1_201, Some(1_600)), "[1,201, 1,600]");
        assert_eq!(range_key(5, None), "[5, ..]");
    }
}
