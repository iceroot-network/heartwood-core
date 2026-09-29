//! The committed one-node devnet chain as the API reads it: the crypto files, the blocks, the state
//! at the tip published as a snapshot, the reference's REST state at the tip (its checkpoint), and
//! a pool view of some of the chain's transactions.

#![allow(dead_code, reason = "each test file uses a part of the fixture")]

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use heartwood_api::context::{NodeInfo, NodeStatus, PeerList, SubmitFuture, SubmitTransactions};
use heartwood_api::server::{ApiConfig, ServerHandle, serve};
use heartwood_api::{ApiContext, ApiError, ChainTotals, Productivity, ProductivityTable};
use heartwood_crypto::blocks::factory;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockId, Milestones, Network, Transaction};
use heartwood_database::BlockRow;
use heartwood_database::{Database, Readers, Settings};
use heartwood_kernel::config::CryptoFiles;
use heartwood_kernel::config::{Env, Options as _};
use heartwood_p2p::monitor::PeerInfo;
use heartwood_pool::{Entry, OnlineEntry, Options, PoolView, PoolViewOptions, ProcessorResult};
use heartwood_state::{State, StateView};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

/// The chain, its state at the tip and the reference's checkpoint there.
pub struct Chain {
    pub files: CryptoFiles,
    pub blocks: Vec<Block>,
    pub state: StateView,
    pub checkpoint: Value,
}

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/difftest/corpus/txgen-one-node")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(corpus().join(name)).unwrap()
}

/// The chain, replayed once per test binary.
pub fn chain() -> &'static Chain {
    static CHAIN: OnceLock<Chain> = OnceLock::new();
    CHAIN.get_or_init(|| {
        let network = Network::load(&read("crypto/network.json")).unwrap();
        let milestones_text = read("crypto/milestones.json");
        let milestones = Milestones::load(&milestones_text, &network).unwrap();
        let files = CryptoFiles {
            network,
            milestones,
            milestones_text,
            genesis: serde_json::from_str(&read("crypto/genesisBlock.json")).unwrap(),
        };
        let blocks: Vec<Block> = read("blocks.jsonl")
            .lines()
            .map(|line| {
                let line: Value = serde_json::from_str(line).unwrap();
                let bytes = hex::decode(line["hex"].as_str().unwrap()).unwrap();
                Block::receive(&bytes, &files.milestones).unwrap()
            })
            .collect();
        let genesis = factory::from_json(&files.genesis, &files.milestones).unwrap();
        let mut state = State::from_genesis(&genesis, &files.milestones, &Handlers).unwrap();
        for block in &blocks {
            state
                .apply_block(block, &files.milestones, &Handlers)
                .unwrap();
        }
        let checkpoint = read("reference/checkpoints/h000000265.json");
        Chain {
            state: state.view(),
            blocks,
            checkpoint: serde_json::from_str(&checkpoint).unwrap(),
            files,
        }
    })
}

impl Chain {
    pub fn milestones(&self) -> &Milestones {
        &self.files.milestones
    }

    /// The block at `height` (from 2).
    pub fn block(&self, height: u32) -> &Block {
        &self.blocks[height as usize - 2]
    }

    /// The reference's record of the block at `height` (its generator's name, burned fee and
    /// donations).
    pub fn meta(&self, height: u32) -> Value {
        let meta = read("reference/meta.jsonl");
        let line = meta
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap());
        line.into_iter().find(|meta| meta["h"] == height).unwrap()
    }

    /// The stored row of the block at `height` (from 2), as the store holds it.
    pub fn row(&self, height: u32) -> BlockRow {
        let block = self.block(height);
        let header = block.header();
        let meta = self.meta(height);
        BlockRow {
            id: *block.id(),
            height,
            version: header.version,
            timestamp: header.timestamp,
            previous_block: Some(BlockId::from_hex(&hex::encode(&header.previous_block)).unwrap()),
            number_of_transactions: header.number_of_transactions,
            total_amount: header.total_amount,
            total_fee: header.total_fee,
            burned_fee: meta["burnedFee"].as_str().unwrap().parse().unwrap(),
            reward: header.reward,
            donations: meta["donations"].to_string(),
            payload_length: header.payload_length,
            payload_hash: hex::encode(&header.payload_hash),
            generator_public_key: header.generator_public_key.to_hex(),
            username: meta["username"].as_str().map(str::to_owned),
            block_signature: header.block_signature.to_hex(),
        }
    }

    /// Every transaction of the chain after the genesis block, in chain order.
    pub fn transactions(&self) -> Vec<Arc<Transaction>> {
        let all = self.blocks.iter().flat_map(|block| block.transactions());
        all.cloned().map(Arc::new).collect()
    }

    /// The reference's wallet at `address` in its checkpoint.
    pub fn reference_wallet(&self, address: &str) -> &Value {
        let wallets = self.checkpoint["wallets"].as_array().unwrap();
        wallets
            .iter()
            .find(|wallet| wallet["address"] == address)
            .unwrap()
    }

    /// The online record the reference's delegates show in its checkpoint: each delegate's
    /// version, in the round of the tip.
    pub fn online(&self) -> Vec<OnlineEntry> {
        let delegates = self.checkpoint["delegates"].as_array().unwrap();
        let mut entries: Vec<OnlineEntry> = delegates
            .iter()
            .filter_map(|delegate| {
                Some(OnlineEntry {
                    public_key: heartwood_crypto::PublicKeyBytes::from_hex(
                        delegate["publicKey"].as_str()?,
                    )
                    .ok()?,
                    round: 5,
                    version: delegate["version"].as_str()?.to_owned(),
                })
            })
            .collect();
        entries.sort_by_key(|entry| entry.public_key);
        entries
    }

    /// A pool of `transactions`, in that order, with the chain's online record and the
    /// reference's default options.
    pub fn pool(&self, transactions: Vec<Arc<Transaction>>) -> PoolView {
        let options = Options::defaults(&|_| None, std::path::Path::new("/nonexistent"), 4);
        PoolView {
            by_priority: transactions,
            online: self.online(),
            options: PoolViewOptions::of(&options),
        }
    }
}

/// Peers as the node lists them.
pub struct Peers(pub Vec<PeerInfo>);

impl PeerList for Peers {
    fn peers(&self) -> Vec<PeerInfo> {
        self.0.clone()
    }
}

/// A pool processor that admits every entry of a submission, by index.
/// A node without productivity: the routes that read it are not served in these tests, so each
/// call is a 500.
pub struct NoProductivity;

impl Productivity for NoProductivity {
    fn table(&self, _tip: &Block) -> Result<Arc<ProductivityTable>, ApiError> {
        Err(ApiError::internal())
    }

    fn totals(&self, _tip: &Block) -> Result<ChainTotals, ApiError> {
        Err(ApiError::internal())
    }
}

pub struct AdmitAll;

impl SubmitTransactions for AdmitAll {
    fn submit(&self, entries: Vec<Entry>) -> SubmitFuture {
        let ids: Vec<String> = (0..entries.len()).map(|index| index.to_string()).collect();
        Box::pin(async move {
            Ok(ProcessorResult {
                accept: ids.clone(),
                broadcast: ids,
                ..ProcessorResult::default()
            })
        })
    }
}

/// The API's default options on a free local port, the rate limit's whitelist naming the tests'
/// address.
pub fn local_config() -> ApiConfig {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let options = json!({
        "server": { "http": { "host": "127.0.0.1", "port": port } },
        "plugins": { "rateLimit": { "whitelist": ["127.0.0.1"] } },
    });
    ApiConfig::from_options(&options, &Env::default()).unwrap()
}

/// The context of a node at the chain's tip: `pool` in its pool, `peers` as its peers, a network
/// height of 270, an empty store in a directory of its own, and `config`.
pub fn context(name: &str, pool: PoolView, peers: Vec<PeerInfo>, config: ApiConfig) -> ApiContext {
    let chain = chain();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("api-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let settings = Settings::in_data_dir(&dir);
    drop(Database::open(&settings).unwrap());
    let status = NodeStatus {
        synced: true,
        network_height: 270,
    };
    ApiContext {
        state: watch::channel(Arc::new(chain.state.clone())).1,
        pool: watch::channel(Arc::new(pool)).1,
        status: watch::channel(status).1,
        peers: Arc::new(Peers(peers)),
        history: Arc::new(Readers::open(&settings, 2).unwrap()),
        submit: Arc::new(AdmitAll),
        productivity: Arc::new(NoProductivity),
        node: Arc::new(NodeInfo {
            crypto: chain.files.clone(),
            version: "4.3.1-heartwood.0.1.0".into(),
        }),
        config: Arc::new(config),
    }
}

/// The chain served by a node (see [`context`]) on a free local port.
pub async fn serve_chain(
    name: &str,
    pool: PoolView,
    peers: Vec<PeerInfo>,
) -> (SocketAddr, ServerHandle) {
    let context = context(name, pool, peers, local_config());
    let address = context.config.address;
    let server = serve(Arc::new(context), &tokio::runtime::Handle::current()).unwrap();
    (address, server)
}

/// An answer over HTTP: its status, its headers (names in lowercase) and its JSON body.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        let found = self.headers.iter().find(|(key, _)| key == name);
        found.map(|(_, value)| value.as_str())
    }
}

/// `method target` with `headers` and `body` to `address`, over one connection.
pub async fn http(
    address: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut request = format!("{method} {target} HTTP/1.1\r\nhost: node\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("content-length: {}\r\n\r\n", body.len()));
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    let text = String::from_utf8(answer).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap()[9..12].parse().unwrap();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(": ").unwrap();
            (name.to_lowercase(), value.to_owned())
        })
        .collect();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body).unwrap()
    };
    Answer {
        status,
        headers,
        body,
    }
}

/// `GET target`.
pub async fn get(address: SocketAddr, target: &str) -> Answer {
    http(address, "GET", target, &[], b"").await
}
