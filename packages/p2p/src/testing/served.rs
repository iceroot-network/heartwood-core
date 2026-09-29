//! The public controllers over a scripted node, for the oracle's `p2p.controller` and its vector
//! test: [`controller`] answers a route's request with the server's own controllers, over a chain
//! at a height, stored blocks, a peer repository, a pool and delegates that the input describes.
//!
//! The node here is scripted, as the reference's collaborators are in the oracle: what a case
//! checks is the controller's steps (the filters, the order, the cut, the lines, the answers it
//! writes), not the pool's or the store's. A synthetic store of full blocks is built from its
//! parameters alone, block by block, as a listing is asked for, so a store of hundreds of blocks
//! of a megabyte each costs nothing until a route reads it.

// Test support: its arithmetic is over the sizes of a scripted store and its indexes over the
// JSON of a scripted case, both written by the test that runs it, never over a peer's bytes.
#![allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::future::BoxFuture;
use heartwood_crypto::blocks::deserialiser::deserialise_header;
use heartwood_crypto::blocks::serialiser::block_id;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::bytes::Reader;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{
    Block, BlockHeader, BlockId, Milestones, Network, PublicKeyBytes, TransactionId,
};
use heartwood_kernel::{ChainLink, Clock};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::{BlockListing, get_blocks_decode_response};
use heartwood_p2p_wire::internal::CurrentRound;
use heartwood_p2p_wire::status::{NetworkConfig, PeerConfig, PluginConfig, TokenConfig};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::server::FakeChain;
use crate::attestation_session::AttestationSession;
use crate::errors::LookupError;
use crate::node_services::{
    Collation, ForgerEvent, NodeServices, NodeUnavailable, PeerSide, QueueState,
};
use crate::options::P2pOptions;
use crate::peer::Peer;
use crate::server::ServerOptions;
use crate::server::controllers::blocks::MAX_ESTIMATE;
use crate::server::controllers::{ControllerParts, Controllers, remote_access_warnings};
use crate::server::seams::{RouteHandlers, ServerRefusal, ServerRequest, SessionId};
use crate::status_server::StatusServer;
use crate::view::{BlockPing, ChainView, DelegateInfo};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a route answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Served {
    /// The oracle's output shape: `{hex}` (or `{digest}`), `handled` for postBlock and `collated`
    /// for getUnconfirmedTransactions; `{thrown: true}` for a 500, which the reference's server
    /// gives to an error that is its own controller's throw.
    pub output: Value,
    /// The refusal, when the route refused.
    pub refusal: Option<ServerRefusal>,
    /// How many of the log lines written during the call came before the route ran, when the
    /// controllers were set up (the warnings of a `remoteAccess` that admits no forger).
    pub setup_lines: usize,
}

/// The header of a stored block and what it holds of its transactions.
struct Row {
    height: u32,
    id: String,
    kind: Kind,
}

enum Kind {
    /// A block given as bytes: its header and its transactions (none for a header alone).
    Real {
        header: Box<BlockHeader>,
        transactions: Vec<Vec<u8>>,
    },
    /// A block of the synthetic store: all `per_block` transactions are the one `transaction`.
    Synthetic {
        transaction: Arc<Vec<u8>>,
        per_block: u32,
    },
}

/// The amount of a store's `per_block` transfers of 256 recipients at 1,000 each, and their fee.
const RECIPIENTS: u64 = 256;
const AMOUNT_EACH: u64 = 1_000;
const FEE_EACH: u64 = 100_000_000;

fn padded(value: u64, width: usize) -> String {
    format!("{value:0width$x}")
}

impl Row {
    /// The block `bytes` as the store keeps it.
    fn of_bytes(bytes: &[u8]) -> Result<Row, String> {
        let mut reader = Reader::new(bytes);
        let header = deserialise_header(&mut reader).map_err(|e| e.to_string())?;
        let mut transactions = Vec::new();
        if !reader.is_empty() {
            let mut lengths = Vec::new();
            for _ in 0..header.number_of_transactions {
                lengths.push(reader.u32_le().map_err(|e| e.to_string())?);
            }
            for length in lengths {
                let length = usize::try_from(length).map_err(|e| e.to_string())?;
                let bytes = reader.bytes(length).map_err(|e| e.to_string())?;
                transactions.push(bytes.to_vec());
            }
        }
        Ok(Row {
            height: header.height,
            id: block_id(&header).to_hex(),
            kind: Kind::Real {
                header: Box::new(header),
                transactions,
            },
        })
    }

    /// The listing of the row, with its transactions unless `headers_only`.
    fn listing(&self, headers_only: bool) -> BlockListing {
        match &self.kind {
            Kind::Real {
                header,
                transactions,
            } => BlockListing {
                id: self.id.clone(),
                version: header.version,
                timestamp: header.timestamp,
                previous_block: (header.height > 1).then(|| hex::encode(&header.previous_block)),
                height: header.height,
                number_of_transactions: header.number_of_transactions,
                total_amount: Amount::new(header.total_amount.into()),
                total_fee: Amount::new(header.total_fee.into()),
                reward: Amount::new(header.reward.into()),
                payload_length: header.payload_length,
                payload_hash: hex::encode(&header.payload_hash),
                generator_public_key: header.generator_public_key.to_hex(),
                block_signature: header.block_signature.to_hex(),
                transactions: if headers_only {
                    Vec::new()
                } else {
                    transactions.clone()
                },
            },
            Kind::Synthetic {
                transaction,
                per_block,
            } => {
                let (height, count) = (u64::from(self.height), u64::from(*per_block));
                let bytes = u64::try_from(transaction.len()).unwrap_or(u64::MAX);
                BlockListing {
                    id: self.id.clone(),
                    version: 0,
                    timestamp: self.height.saturating_mul(8),
                    previous_block: Some(padded(height.saturating_sub(1), 64)),
                    height: self.height,
                    number_of_transactions: *per_block,
                    total_amount: Amount::new((count * RECIPIENTS * AMOUNT_EACH).into()),
                    total_fee: Amount::new((count * FEE_EACH).into()),
                    reward: Amount::new(200_000_000),
                    payload_length: u32::try_from(count * (bytes + 4)).unwrap_or(u32::MAX),
                    payload_hash: padded(height + 1_000, 64),
                    generator_public_key: format!("02{}", padded(height, 64)),
                    block_signature: padded(height, 128),
                    transactions: if headers_only {
                        Vec::new()
                    } else {
                        vec![transaction.as_ref().clone(); *per_block as usize]
                    },
                }
            }
        }
    }

    /// The bytes of transactions the row lists, for the estimate the getBlocks cut reads.
    fn transaction_bytes(&self) -> usize {
        match &self.kind {
            Kind::Real { transactions, .. } => transactions.iter().map(Vec::len).sum(),
            Kind::Synthetic {
                transaction,
                per_block,
            } => transaction.len().saturating_mul(*per_block as usize),
        }
    }
}

/// The chain the controllers read: the last block, the milestones and the delegates.
struct ServedChain {
    last: Arc<Block>,
    milestones: Milestones,
    network: Network,
    delegates: Vec<(PublicKeyBytes, DelegateInfo)>,
    registered: Vec<PublicKeyBytes>,
}

impl ChainView for ServedChain {
    fn last_block(&self) -> Arc<Block> {
        Arc::clone(&self.last)
    }

    fn last_blocks(&self) -> Vec<Arc<Block>> {
        vec![self.last_block()]
    }

    fn milestones(&self) -> &Milestones {
        &self.milestones
    }

    fn network(&self) -> &Network {
        &self.network
    }

    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
        let found = self.delegates.iter().find(|(known, _)| known == key);
        found.map(|(_, delegate)| delegate.clone())
    }

    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        self.registered.clone()
    }

    fn is_synced(&self) -> bool {
        true
    }

    fn is_idle(&self) -> bool {
        true
    }

    fn block_ping(&self) -> Option<BlockPing> {
        None
    }
}

/// What the node was asked to do with a posted block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Handled {
    id: String,
    from_forger: bool,
    ip: String,
}

/// The pool the controllers read: its size, its candidates, and the ids its processor refuses.
#[derive(Default)]
struct Pool {
    size: usize,
    candidates: Vec<Vec<u8>>,
    refuse: HashSet<[u8; 32]>,
    known: HashSet<[u8; 32]>,
}

/// The scripted node.
struct ServedNode {
    chain: Arc<ServedChain>,
    rows: Vec<Row>,
    memory: u32,
    pinged: bool,
    downloaded: ChainLink,
    already_forged: bool,
    pool: Pool,
    config: PeerConfig,
    queue: watch::Sender<QueueState>,
    handled: Mutex<Option<Handled>>,
    collated: Mutex<Option<(bool, Vec<TransactionId>)>>,
}

impl ServedNode {
    fn in_memory(&self, row: &Row) -> bool {
        let tip = self.chain.last.height();
        row.height <= tip && u64::from(row.height) + u64::from(self.memory) > u64::from(tip)
    }
}

impl NodeServices for ServedNode {
    fn is_booted(&self) -> bool {
        true
    }

    fn first_synced_ms(&self) -> Option<i64> {
        None
    }

    fn queue_state(&self) -> watch::Receiver<QueueState> {
        self.queue.subscribe()
    }

    fn chain(&self) -> Arc<dyn ChainView> {
        Arc::clone(&self.chain) as Arc<dyn ChainView>
    }

    fn last_downloaded_block(&self) -> ChainLink {
        self.downloaded
    }

    fn ping_block(&self, _header: &BlockHeader) -> bool {
        self.pinged
    }

    fn handle_incoming_block(&self, block: Block, from_forger: bool, ip: IpAddr) {
        *lock(&self.handled) = Some(Handled {
            id: block.id().to_hex(),
            from_forger,
            ip: ip.to_string(),
        });
    }

    fn delegate_rank(&self, username: &str) -> Option<u32> {
        let named = self.chain.delegates.iter().map(|(_, delegate)| delegate);
        named.into_iter().find(|d| d.username == username)?.rank
    }

    fn already_forged(&self, _height: u32, _username: &str) -> Result<bool, LookupError> {
        Ok(self.already_forged)
    }

    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError> {
        Ok(Vec::new())
    }

    fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<BlockListing>, LookupError> {
        let last = u64::from(from) + u64::from(limit);
        let wanted = self
            .rows
            .iter()
            .filter(|row| row.height >= from && u64::from(row.height) < last);
        // The controller stops at its estimate; listings past twice that are never read, and a
        // store of full blocks is too large to build in full for every request.
        let (mut listed, mut estimate) = (Vec::new(), 0_usize);
        for row in wanted {
            if estimate >= MAX_ESTIMATE.saturating_mul(2) {
                break;
            }
            if !headers_only {
                estimate = estimate.saturating_add(row.transaction_bytes().saturating_mul(2));
            }
            listed.push(row.listing(headers_only));
        }
        Ok(listed)
    }

    fn common_blocks(
        &self,
        ids: &[BlockId],
        asked: usize,
    ) -> Result<Vec<BlockHeader>, LookupError> {
        let names: HashSet<String> = ids.iter().map(BlockId::to_hex).collect();
        let headers = |memory: bool| -> Vec<BlockHeader> {
            let named = self.rows.iter().filter(|row| names.contains(&row.id));
            let kept = named.filter(|row| !memory || self.in_memory(row));
            let real = kept.filter_map(|row| match &row.kind {
                Kind::Real { header, .. } => Some(header.as_ref().clone()),
                Kind::Synthetic { .. } => None,
            });
            real.collect()
        };
        let memory = headers(true);
        Ok(if memory.len() >= asked {
            memory
        } else {
            headers(false)
        })
    }

    fn pool_size(&self) -> usize {
        self.pool.size
    }

    fn collate(
        &self,
        validate: bool,
        exclude: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>> {
        *lock(&self.collated) = Some((validate, exclude));
        Box::pin(async {
            Ok(Collation {
                pool_size: self.pool.size,
                transactions: self.pool.candidates.clone(),
            })
        })
    }

    fn post_transactions(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>> {
        let mut held = self.pool.known.clone();
        let mut accepted = Vec::new();
        for transaction in &transactions {
            let id = sha256(transaction);
            if !self.pool.refuse.contains(&id) && held.insert(id) {
                accepted.push(TransactionId::from_array(id));
            }
        }
        Box::pin(async { Ok(accepted) })
    }

    fn current_round(&self, _now_ms: i64) -> Result<CurrentRound, LookupError> {
        Err(LookupError("no round in the scripted node".to_owned()))
    }

    fn slot_number(&self, _timestamp: Option<i64>, _now_ms: i64) -> i64 {
        0
    }

    fn force_wakeup(&self) {}

    fn forger_event(&self, _event: ForgerEvent) {}

    fn peer_config(&self) -> PeerConfig {
        self.config.clone()
    }
}

/// The peer repository of a case, in order.
struct Repository {
    peers: Vec<Peer>,
}

impl PeerSide for Repository {
    fn accept(&self, _ip: IpAddr, _port: u16) {}

    fn is_peer(&self, ip: IpAddr) -> bool {
        self.peers.iter().any(|peer| peer.ip == ip)
    }

    fn record_version(&self, _ip: IpAddr, _version: &str) {}

    fn mark_stale(&self, _ip: IpAddr) {}

    fn record_violation(&self, _ip: IpAddr, _violation: crate::scoring::Violation) {}

    fn peers(&self) -> Vec<Peer> {
        self.peers.clone()
    }

    fn remove_keys(&self, _keys: &[PublicKeyBytes]) {}

    fn is_cold_start(&self) -> bool {
        false
    }

    fn complete_cold_start(&self) {}

    fn cleanse(&self, _options: crate::monitor::CleanseOptions) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// A clock stopped at one moment.
struct Stopped(i64);

impl Clock for Stopped {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value.as_str().ok_or(format!("{name} is not text"))
}

fn items<'a>(value: &'a Value, name: &str) -> Result<&'a [Value], String> {
    match value {
        Value::Null => Ok(&[]),
        Value::Array(list) => Ok(list),
        _ => Err(format!("{name} is not a list")),
    }
}

fn bytes(value: &Value, name: &str) -> Result<Vec<u8>, String> {
    hex::decode(text(value, name)?).map_err(|e| format!("{name}: {e}"))
}

fn count<T: TryFrom<u64>>(value: &Value, name: &str) -> Result<T, String> {
    let number = value.as_u64().and_then(|n| T::try_from(n).ok());
    number.ok_or(format!("{name} is not a count in range"))
}

fn id_set(value: &Value, name: &str) -> Result<HashSet<[u8; 32]>, String> {
    let ids = items(value, name)?.iter();
    ids.map(|id| hex::decode_array(text(id, name)?).map_err(|e| format!("{name}: {e}")))
        .collect()
}

/// The stored rows of a case: the blocks it gives, its last block, and the synthetic store. A
/// case with no last block of its own has none stored.
fn rows_of(store: &Value) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for block in items(&store["blocks"], "store.blocks")? {
        rows.push(Row::of_bytes(&bytes(block, "store.blocks")?)?);
    }
    if !store["tip"].is_null() {
        rows.push(Row::of_bytes(&bytes(&store["tip"], "store.tip")?)?);
    }
    let synthetic = &store["synthetic"];
    if !synthetic.is_null() {
        let (from, blocks): (u32, u32) = (
            count(&synthetic["from"], "synthetic.from")?,
            count(&synthetic["count"], "synthetic.count")?,
        );
        let per_block = count(&synthetic["perBlock"], "synthetic.perBlock")?;
        let transaction = Arc::new(bytes(&synthetic["transaction"], "synthetic.transaction")?);
        for height in from..from.saturating_add(blocks) {
            rows.push(Row {
                height,
                id: padded(u64::from(height), 64),
                kind: Kind::Synthetic {
                    transaction: Arc::clone(&transaction),
                    per_block,
                },
            });
        }
    }
    rows.sort_by_key(|row| row.height);
    Ok(rows)
}

fn keys_of(value: &Value) -> Result<Vec<PublicKeyBytes>, String> {
    let keys = items(value, "status.registered")?.iter();
    let key = |key: &Value| {
        PublicKeyBytes::from_hex(text(key, "a key")?).map_err(|e| format!("a key: {e}"))
    };
    keys.map(key).collect()
}

fn peers_of(value: &Value) -> Result<Vec<Peer>, String> {
    let mut peers = Vec::new();
    for entry in items(value, "peers")? {
        // The repository holds a peer under its plain address, as the caller's is read.
        let ip: IpAddr = (text(&entry["ip"], "peer.ip")?)
            .parse()
            .map_err(|_| "a peer's ip is not an address")?;
        let ip = ip.to_canonical();
        // A port of -1 cannot be held: the reference leaves such a peer out of its answer.
        let Some(port) = entry["port"].as_i64().filter(|&port| port >= 0) else {
            continue;
        };
        let mut peer = Peer::new(ip, u16::try_from(port).map_err(|e| e.to_string())?);
        peer.latency = match &entry["latency"] {
            Value::Null => None,
            latency => Some(count(latency, "peer.latency")?),
        };
        peers.push(peer);
    }
    Ok(peers)
}

fn delegates_of(value: &Value) -> Result<Vec<(PublicKeyBytes, DelegateInfo)>, String> {
    let mut delegates = Vec::new();
    for entry in items(value, "delegates")? {
        let key = PublicKeyBytes::from_hex(text(&entry["publicKey"], "publicKey")?)
            .map_err(|e| e.to_string())?;
        let username = text(&entry["username"], "username")?.to_owned();
        let rank = match &entry["rank"] {
            Value::Null => None,
            rank => Some(count(rank, "rank")?),
        };
        delegates.push((key, DelegateInfo { username, rank }));
    }
    Ok(delegates)
}

/// The peer configuration of a status input.
fn peer_config_of(config: &Value) -> Result<PeerConfig, String> {
    let network = &config["network"];
    let mut plugins = Vec::new();
    for (name, plugin) in config["plugins"].as_object().into_iter().flatten() {
        plugins.push((
            name.clone(),
            PluginConfig {
                enabled: plugin["enabled"].as_bool() == Some(true),
                port: count(&plugin["port"], "the port of a plugin")?,
                estimate_total_count: plugin["estimateTotalCount"].as_bool(),
            },
        ));
    }
    let name = |value: &Value, name: &str| text(value, name).map(str::to_owned);
    Ok(PeerConfig {
        version: name(&config["version"], "peerConfig.version")?,
        network: NetworkConfig {
            version: count(&network["version"], "network.version")?,
            name: name(&network["name"], "network.name")?,
            nethash: name(&network["nethash"], "network.nethash")?,
            explorer: name(&network["explorer"], "network.explorer")?,
            token: TokenConfig {
                name: name(&network["token"]["name"], "token.name")?,
                symbol: name(&network["token"]["symbol"], "token.symbol")?,
            },
        },
        plugins,
    })
}

/// A getBlocks answer's number of blocks.
fn listed_blocks(answer: &[u8]) -> Value {
    get_blocks_decode_response(answer).map_or(Value::Null, |listing| json!(listing.len()))
}

/// The oracle's `p2p.controller`, answered with the server's controllers: `input` is `{route,
/// request, ip, remoteAccess, store?, peers?, pool?, delegates?, pinged?, lastDownloaded?,
/// alreadyForged?, status?, digest?}` (`request` the payload's hex), the node's last block at
/// `height` under `milestones`. A status input needs no forger: the status is served unsigned.
pub async fn controller(
    input: &Value,
    height: u32,
    milestones: Milestones,
    network: Network,
) -> Result<Served, String> {
    let route = text(&input["route"], "input.route")?;
    let route = RouteId::ALL.into_iter().find(|known| known.id() == route);
    let route = route
        .filter(|route| !route.is_internal())
        .ok_or("no public route")?;
    let ip: IpAddr = (text(&input["ip"], "input.ip")?)
        .parse()
        .map_err(|_| "input.ip is not an address")?;
    let ip = ip.to_canonical();
    let payload = bytes(&input["request"], "input.request")?;
    let store = &input["store"];
    let tip = match (&store["tip"], &input["status"]["tip"]["hex"]) {
        (Value::Null, Value::Null) => None,
        (Value::Null, status) => Some(bytes(status, "status.tip.hex")?),
        (tip, _) => Some(bytes(tip, "store.tip")?),
    };
    let last = match tip {
        None => FakeChain::at(height, milestones.clone(), network.clone())?.last_block(),
        Some(tip) => {
            Arc::new(Block::decode(&tip, &milestones).map_err(|e| format!("the tip: {e}"))?)
        }
    };
    if last.height() != height {
        return Err(format!(
            "store.tip is at height {}, not {height}",
            last.height()
        ));
    }
    let downloaded = match &input["lastDownloaded"] {
        Value::Null => ChainLink::of(&last),
        hex => {
            let block = Block::decode(&bytes(hex, "lastDownloaded")?, &milestones);
            ChainLink::of(&block.map_err(|e| format!("lastDownloaded: {e}"))?)
        }
    };
    let status = &input["status"];
    let config = match status.get("peerConfig") {
        Some(config) => peer_config_of(config)?,
        None => PeerConfig {
            version: String::new(),
            network: NetworkConfig {
                version: network.pub_key_hash,
                name: network.name.clone(),
                nethash: network.nethash.clone(),
                explorer: network.explorer.clone(),
                token: TokenConfig {
                    name: network.token.clone(),
                    symbol: network.symbol.clone(),
                },
            },
            plugins: Vec::new(),
        },
    };
    let pool = &input["pool"];
    let mut candidates = Vec::new();
    for candidate in items(&pool["candidates"], "pool.candidates")? {
        candidates.push(bytes(candidate, "pool.candidates")?);
    }
    let pool = Pool {
        size: if pool.is_null() {
            0
        } else {
            count(&pool["size"], "pool.size")?
        },
        candidates,
        refuse: id_set(&pool["refuse"], "pool.refuse")?,
        known: id_set(&pool["known"], "pool.known")?,
    };
    let rows = rows_of(store)?;
    let chain = Arc::new(ServedChain {
        last,
        milestones,
        network,
        delegates: delegates_of(&input["delegates"])?,
        registered: keys_of(&status["registered"])?,
    });
    let node = Arc::new(ServedNode {
        chain,
        rows,
        memory: match &store["memory"] {
            Value::Null => 100,
            memory => count(memory, "store.memory")?,
        },
        pinged: input["pinged"].as_bool() == Some(true),
        downloaded,
        already_forged: input["alreadyForged"].as_bool() == Some(true),
        pool,
        config,
        queue: watch::Sender::new(QueueState::default()),
        handled: Mutex::new(None),
        collated: Mutex::new(None),
    });
    let peers = Arc::new(Repository {
        peers: peers_of(&input["peers"])?,
    });
    let clock: Arc<dyn Clock> = Arc::new(Stopped(status["now"].as_i64().unwrap_or_default()));
    let attestation = Arc::new(AttestationSession::new(
        Arc::clone(&node) as Arc<dyn NodeServices>,
        Arc::clone(&clock),
    ));
    if !items(&status["forgerKeys"], "status.forgerKeys")?.is_empty() {
        return Err("a status with forger keys needs a signing forger".to_owned());
    }
    let status_server = Arc::new(StatusServer::new(
        Arc::clone(&node) as Arc<dyn NodeServices>,
        Arc::clone(&attestation),
        Arc::clone(&clock),
    ));
    let options = P2pOptions {
        remote_access: (items(&input["remoteAccess"], "input.remoteAccess")?.iter())
            .map(|entry| text(entry, "remoteAccess").map(str::to_owned))
            .collect::<Result<_, _>>()?,
        ..P2pOptions::default()
    };
    let from_forger = ServerOptions::from_p2p(&options)
        .remote_access
        .contains(&ip);
    let setup_lines = remote_access_warnings(&options).len();
    let controllers = Controllers::new(ControllerParts {
        node: Arc::clone(&node) as Arc<dyn NodeServices>,
        peers: peers as Arc<dyn PeerSide>,
        attestation,
        status: status_server,
        clock,
        options: Arc::new(options),
    });
    let request = ServerRequest {
        payload,
        ip,
        session: SessionId(1),
        from_forger,
    };
    let answered = controllers.handle(route, request).await;
    let answer = match answered {
        Ok(answer) => answer,
        Err(refusal) if refusal.status == 500 => {
            return Ok(Served {
                output: json!({"thrown": true}),
                refusal: Some(refusal),
                setup_lines,
            });
        }
        Err(refusal) => {
            return Ok(Served {
                output: Value::Null,
                refusal: Some(refusal),
                setup_lines,
            });
        }
    };
    let mut output = if input["digest"].as_bool() == Some(true) {
        let blocks = if route == RouteId::GetBlocks {
            listed_blocks(&answer)
        } else {
            Value::Null
        };
        let digest = json!({"sha256": hex::encode(&sha256(&answer)), "length": answer.len(), "blocks": blocks});
        json!({ "digest": digest })
    } else {
        json!({ "hex": hex::encode(&answer) })
    };
    if route == RouteId::PostBlock {
        output["handled"] = match lock(&node.handled).as_ref() {
            Some(handled) => {
                json!({"id": handled.id, "fromForger": handled.from_forger, "ip": handled.ip})
            }
            None => Value::Null,
        };
    }
    if route == RouteId::GetUnconfirmedTransactions {
        output["collated"] = match lock(&node.collated).as_ref() {
            Some((validate, exclude)) => {
                let ids: Vec<String> = exclude.iter().map(TransactionId::to_hex).collect();
                json!({"validate": validate, "exclude": ids})
            }
            None => Value::Null,
        };
    }
    Ok(Served {
        output,
        refusal: None,
        setup_lines,
    })
}
