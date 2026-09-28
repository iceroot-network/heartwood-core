//! The committed one-node chain in a store, and the API's context over it: what the history routes'
//! tests read. The store is built once per test binary, one chunk per 50 blocks, and its readers
//! and the tip's state view are the context's.

#![allow(dead_code, reason = "each test file uses a part of the helpers")]

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use heartwood_api::context::SubmitError;
use heartwood_api::context::SubmitFuture;
use heartwood_api::server::{RateLimit, Semaphore, SemaphoreLevel};
use heartwood_api::{
    ApiConfig, ApiContext, ApiRequest, NodeInfo, NodeStatus, PeerList, RouteId, SubmitTransactions,
};
use heartwood_blockchain::{ChainStore, RestoreMode, restore};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, Milestones, Network, Transaction};
use heartwood_database::{Readers, Settings};
use heartwood_kernel::config::CryptoFiles;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_pool::{Entry, Options, PoolView, PoolViewOptions};
use heartwood_transactions::Handlers;
use serde_json::{Map, Value};
use tokio::sync::watch;

/// The committed chain and the API's context over its store.
pub struct Fixture {
    /// The context the routes read.
    pub ctx: Arc<ApiContext>,
    /// The blocks after the genesis block.
    pub blocks: Vec<Block>,
    /// The genesis block.
    pub genesis: Block,
    /// The block columns the reference stored, by height from 2: `username`, `burnedFee` and
    /// `donations`.
    pub meta: Vec<Value>,
    /// The store's settings, for readers of its own.
    pub settings: Settings,
}

impl Fixture {
    /// The milestones of the chain.
    pub fn milestones(&self) -> &Milestones {
        &self.ctx.node.crypto.milestones
    }

    /// The tip's height.
    pub fn tip(&self) -> u32 {
        self.blocks.last().map_or(1, Block::height)
    }

    /// The block at `height`, the genesis block at 1.
    pub fn block(&self, height: u32) -> &Block {
        if height == 1 {
            &self.genesis
        } else {
            &self.blocks[height as usize - 2]
        }
    }

    /// Every transaction of the chain with its block, in chain order.
    pub fn transactions(&self) -> Vec<(&Block, u32, &Transaction)> {
        std::iter::once(&self.genesis)
            .chain(&self.blocks)
            .flat_map(|block| {
                (0..)
                    .zip(block.transactions())
                    .map(move |(sequence, transaction)| (block, sequence, transaction))
            })
            .collect()
    }
}

/// The fixture of this test binary.
pub fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(build)
}

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/difftest/corpus/txgen-one-node")
}

fn read(name: &str) -> String {
    fs::read_to_string(corpus().join(name)).unwrap()
}

fn lines(name: &str) -> Vec<Value> {
    read(name)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The chain's crypto files.
pub fn files() -> CryptoFiles {
    let network = Network::load(&read("crypto/network.json")).unwrap();
    let text = read("crypto/milestones.json");
    let milestones = Milestones::load(&text, &network).unwrap();
    CryptoFiles {
        network,
        milestones,
        milestones_text: text,
        genesis: serde_json::from_str(&read("crypto/genesisBlock.json")).unwrap(),
    }
}

/// A fresh directory for this test binary's store.
fn scratch() -> PathBuf {
    let binary = std::env::current_exe().unwrap();
    let name = binary.file_stem().unwrap().to_string_lossy().into_owned();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("api-history-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn build() -> Fixture {
    let files = files();
    let milestones = files.milestones.clone();
    let blocks: Vec<Block> = lines("blocks.jsonl")
        .iter()
        .map(|line| {
            let bytes = hex::decode(line["hex"].as_str().unwrap()).unwrap();
            Block::receive(&bytes, &milestones).unwrap()
        })
        .collect();
    let settings = Settings::in_data_dir(&scratch());
    let mut store = ChainStore::open(&settings).unwrap();
    let mut state = restore(&mut store, &files, &Handlers, RestoreMode::Auto)
        .unwrap()
        .state;
    let genesis = state.genesis_block().clone();
    for chunk in blocks.chunks(50) {
        let mut writer = store.chunk().unwrap();
        for block in chunk {
            let applied = state.apply_block(block, &milestones, &Handlers).unwrap();
            writer
                .applied(&state, block, &applied, &milestones)
                .unwrap();
        }
        writer.commit().unwrap();
    }
    let options = Options::defaults(&|_| None, &settings.path, 1);
    let pool = PoolView {
        by_priority: Vec::new(),
        online: Vec::new(),
        options: PoolViewOptions::of(&options),
    };
    let (_, state) = watch::channel(Arc::new(state.view()));
    let (_, pool) = watch::channel(Arc::new(pool));
    let (_, status) = watch::channel(NodeStatus::default());
    let ctx = ApiContext {
        state,
        pool,
        status,
        peers: Arc::new(NoPeers),
        history: Arc::new(Readers::open(&settings, 2).unwrap()),
        submit: Arc::new(NoSubmissions),
        node: Arc::new(NodeInfo {
            crypto: files,
            version: "4.3.1-heartwood.0.1.0".into(),
        }),
        config: Arc::new(config()),
    };
    Fixture {
        ctx: Arc::new(ctx),
        blocks,
        genesis,
        meta: lines("reference/meta.jsonl"),
        settings,
    }
}

/// The API's options with the reference's defaults, the rate limit and the semaphore off.
fn config() -> ApiConfig {
    let level = |concurrency, queue_limit, max_offset| SemaphoreLevel {
        concurrency,
        queue_limit,
        max_offset,
    };
    ApiConfig {
        enabled: true,
        address: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4003),
        https: false,
        rate_limit: RateLimit {
            enabled: false,
            points: 100,
            duration_s: 60,
            whitelist: Vec::new(),
            blacklist: Vec::new(),
        },
        semaphore: Semaphore {
            enabled: false,
            database: [level(10, 100, Some(10_000)), level(1, 5, None)],
            memory: [level(3, 30, Some(1_000)), level(1, 5, None)],
        },
        socket_timeout_ms: 5_000,
        whitelist: vec!["*".into()],
        trust_proxy: false,
        base_path: "/api".into(),
        estimate_total_count: false,
    }
}

/// A node that knows no peer.
struct NoPeers;

impl PeerList for NoPeers {
    fn peers(&self) -> Vec<PeerInfo> {
        Vec::new()
    }
}

/// A node whose pool takes nothing.
struct NoSubmissions;

impl SubmitTransactions for NoSubmissions {
    fn submit(&self, _entries: Vec<Entry>) -> SubmitFuture {
        Box::pin(async { Err(SubmitError) })
    }
}

/// A request of `route` with the path parameters `params` and the validated query `query`.
pub fn request(route: RouteId, params: &[(&str, &str)], query: Value) -> ApiRequest {
    let params: Map<String, Value> = params
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::from(*value)))
        .collect();
    let mut path = route.path().to_owned();
    for (name, value) in &params {
        path = path.replace(&format!("{{{name}}}"), value.as_str().unwrap_or_default());
    }
    ApiRequest {
        route,
        params,
        path,
        query,
        payload: None,
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
    }
}
