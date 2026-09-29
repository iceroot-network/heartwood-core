//! The node's services for the peer-to-peer server over a node of the committed one-node corpus
//! (its store written by the node thread's jobs): the blocks of a download and the common blocks
//! from memory and from the store, the P2P channel's processor with its broadcast, the collation
//! on the node thread, the round and the slots as the forger reads them, the delegates' ranks, the
//! boot and the first sync, the forger's events on the bus, and, in a harness build, the
//! network-state record.

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Setup, corpus, run};
use heartwood_blockchain::pool_task::TransactionBroadcast;
use heartwood_blockchain::{
    BlockProcessor, Blockchain, BlockchainOptions, BlockchainParts, ChainState, IncomingBlock, Job,
    Network, Node, NodeHandle, NodeParts, OnlineWork, PoolTask, RollbackDepth,
};
use heartwood_core::app::restored;
use heartwood_core::config::RelayConfig;
use heartwood_core::node_services::{RelayServices, queue_state};
use heartwood_core::views::{Links, NodeView};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockId, Milestones, Transaction, TransactionId};
use heartwood_database::{Readers, Settings};
use heartwood_kernel::bus::EventBus;
use heartwood_kernel::{Clock, Event, SystemClock, calculate_round};
use heartwood_p2p::errors::{DownloadError, SeedError};
use heartwood_p2p::monitor::{CleanseOptions, NetworkStatus};
use heartwood_p2p::node_services::ForgerEvent;
use heartwood_p2p::{DownloadedBlock, NodeServices};
use heartwood_p2p_wire::status::{NetworkConfig, PeerConfig, TokenConfig};
use heartwood_pool::Pool;
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

/// A network without peers.
struct NoPeers;

impl Network for NoPeers {
    async fn boot(&self) -> Result<(), SeedError> {
        Ok(())
    }
    async fn update_network_status(&self, _: bool) {}
    async fn cleanse_peers(&self, _: CleanseOptions) {}
    async fn discover_peers(&self, _: bool, _: bool, _: bool) -> bool {
        false
    }
    fn network_height(&self) -> u32 {
        0
    }
    fn peer_count(&self) -> usize {
        0
    }
    async fn check_network_health(&self, _: bool) -> NetworkStatus {
        NetworkStatus::default()
    }
    async fn check_for_fork(&self) -> u32 {
        0
    }
    async fn download_block_at_height(
        &self,
        _: IpAddr,
        _: u32,
    ) -> Result<Option<DownloadedBlock>, DownloadError> {
        Ok(None)
    }
    async fn download_blocks_from_height(
        &self,
        _: u32,
        _: usize,
        _: bool,
        _: Option<Duration>,
        _: bool,
    ) -> Vec<DownloadedBlock> {
        Vec::new()
    }
    async fn download_transactions(&self, _: &[TransactionId]) -> Vec<Vec<u8>> {
        Vec::new()
    }
    async fn broadcast_block(&self, _: &Block) {}
    fn broadcast_transactions(&self, _: &[Transaction]) {}
    fn has_minimum_peers(&self, _: bool) -> bool {
        true
    }
    async fn populate_seed_peers(&self) -> Result<(), SeedError> {
        Ok(())
    }
    fn ping_all(&self) {}
    fn synced(&self) {}
    async fn refresh_peers_after_fork(&self) {}
    fn disconnect_invalid_peers(&self) {}
    fn is_cold_start(&self) -> bool {
        false
    }
    fn complete_cold_start(&self) {}
}

/// The transactions the node broadcast, as it admitted them.
#[derive(Default)]
struct Broadcasts(Mutex<Vec<TransactionId>>);

impl TransactionBroadcast for Broadcasts {
    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        let ids = transactions.iter().map(|transaction| *transaction.id());
        self.0.lock().unwrap().extend(ids);
    }
}

/// The corpus's blocks after the genesis block.
fn blocks(milestones: &Milestones) -> Vec<Block> {
    let text = std::fs::read_to_string(corpus().join("blocks.jsonl")).unwrap();
    let block = |line: &str| {
        let record: Value = serde_json::from_str(line).unwrap();
        let bytes = hex::decode(record["hex"].as_str().unwrap()).unwrap();
        Block::decode_trusted(&bytes, milestones).unwrap()
    };
    text.lines().map(block).collect()
}

/// `blocks` as one job, named from the state `handle` published.
fn job(handle: &NodeHandle, blocks: &[Block]) -> Job {
    let state = handle.state().borrow().clone();
    let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    let incoming = blocks.iter().map(|block| {
        let mut incoming = IncomingBlock::downloaded(DownloadedBlock::from_block(block, ip));
        incoming.set_username(&state.wallets);
        incoming
    });
    Job {
        blocks: incoming.collect(),
    }
}

/// The peer configuration the services give.
fn peer_config() -> PeerConfig {
    PeerConfig {
        version: heartwood_core::VERSION.into(),
        network: NetworkConfig {
            version: 90,
            name: "devnet".into(),
            nethash: "0".repeat(64),
            explorer: String::new(),
            token: TokenConfig {
                name: "dROOT".into(),
                symbol: "dRT".into(),
            },
        },
        plugins: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_servers_reads_and_work_go_to_the_node_and_its_store() {
    let setup = Setup::new("node-services", "");
    let config = RelayConfig::load(
        &run("--network testnet"),
        &setup.process(&[]),
        &setup.home(),
        &setup.root,
    )
    .unwrap();
    let (store, restored) = restored(&config).unwrap();
    let milestones = Arc::new(config.crypto.milestones.clone());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let chain = Arc::new(Mutex::new(ChainState::at(Arc::new(
        restored.state.last_block().clone(),
    ))));
    let pool_options = heartwood_pool::Options::defaults(&|_| None, &setup.root.join("data"), 2);
    let (links, broadcasts, bus) = (
        Arc::new(Links::default()),
        Arc::new(Broadcasts::default()),
        EventBus::new(),
    );
    let (node, handle) = Node::spawn(NodeParts {
        state: restored.state,
        store,
        pool: PoolTask::new(Pool::open(pool_options, None).unwrap(), restored.online),
        chain: Arc::clone(&chain),
        processor: Arc::new(BlockProcessor),
        handlers: Handlers,
        milestones: Arc::clone(&milestones),
        network: Arc::new(config.crypto.network.clone()),
        options: Arc::new(BlockchainOptions::default()),
        clock: Arc::clone(&clock),
        listener: Arc::new(heartwood_blockchain::events::NoListener),
        observer: None,
        broadcast: broadcasts.clone(),
        own_keys: links.clone(),
        version: heartwood_core::VERSION.into(),
        #[cfg(feature = "difftest")]
        stall: None,
    })
    .unwrap();
    let corpus = blocks(&milestones);
    let before = |height: u32| &corpus[..usize::try_from(height - 2).unwrap()];
    handle.job(job(&handle, before(59))).await.unwrap();
    assert_eq!(handle.state().borrow().height(), 58);
    let settings = Settings::in_data_dir(&config.paths.data);
    let view = Arc::new(NodeView {
        handle: handle.clone(),
        chain: Arc::clone(&chain),
        milestones: Arc::clone(&milestones),
        network: Arc::new(config.crypto.network.clone()),
        history: Arc::new(Readers::open(&settings, 2).unwrap()),
        links: Arc::clone(&links),
    });
    let blockchain = Blockchain::new(BlockchainParts {
        node: handle.clone(),
        chain: Arc::clone(&chain),
        network: NoPeers,
        options: Arc::new(BlockchainOptions::default()),
        milestones: Arc::clone(&milestones),
        clock: Arc::clone(&clock),
        rollback: RollbackDepth::Random,
        runtime: tokio::runtime::Handle::current(),
    });
    let record = setup.root.join("harness");
    let services = RelayServices {
        handle: handle.clone(),
        view,
        history: Arc::new(Readers::open(&settings, 1).unwrap()),
        links: Arc::clone(&links),
        queue: queue_state(&blockchain),
        blockchain,
        peer_config: peer_config(),
        bus: bus.clone(),
        clock,
        #[cfg(feature = "difftest")]
        record: Some(heartwood_core::harness::NetworkStateRecord::open(&record).unwrap()),
        #[cfg(feature = "difftest")]
        await_record: None,
    };

    // The boot and the first sync, as the server reads them.
    assert!(!services.is_booted() && services.first_synced_ms().is_none());
    links.booted.store(true, Ordering::Release);
    assert!(services.is_booted());
    assert_eq!(*services.queue_state().borrow(), Default::default());

    // Block 59's transactions through the P2P channel, then collated for the forger.
    for wallet in handle.state().borrow().wallets.delegates() {
        let key = wallet.public_key().unwrap().to_compressed();
        let set = OnlineWork::Set {
            key,
            round: 1,
            version: "4.3.1".into(),
        };
        handle.online(set).unwrap();
    }
    let transactions = corpus[57].transactions();
    let bytes: Vec<Vec<u8>> = transactions.iter().map(|t| t.bytes().to_vec()).collect();
    let mut accepted = services.post_transactions(bytes.clone()).await.unwrap();
    let mut ids: Vec<TransactionId> = transactions.iter().map(|t| *t.id()).collect();
    accepted.sort();
    ids.sort();
    assert_eq!(accepted, ids);
    let mut broadcast = broadcasts.0.lock().unwrap().clone();
    broadcast.sort();
    assert_eq!(broadcast, ids, "the P2P channel broadcasts what it admits");
    assert_eq!(services.pool_size(), 4);
    let collation = services.collate(true, vec![ids[0]]).await.unwrap();
    assert_eq!((collation.pool_size, collation.transactions.len()), (4, 3));

    // The node forges block 59 and more; the store holds them.
    handle.job(job(&handle, &corpus[57..69])).await.unwrap();
    let tip = handle.state().borrow().height();
    assert_eq!(tip, 70);
    let listed = services.blocks_for_download(58, 3, false).unwrap();
    let heights: Vec<u32> = listed.iter().map(|block| block.height).collect();
    assert_eq!(heights, [58, 59, 60]);
    assert_eq!(listed[1].transactions, bytes);
    assert_eq!(listed[1].id, corpus[57].id().to_hex());
    let headers = services.blocks_for_download(58, 3, true).unwrap();
    assert!(headers.iter().all(|block| block.transactions.is_empty()));
    assert!(
        services
            .blocks_for_download(tip + 1, 400, false)
            .unwrap()
            .is_empty()
    );
    // The common blocks: from memory when every id is there, else from the store.
    let id = |height: u32| *corpus[usize::try_from(height - 2).unwrap()].id();
    let common = services.common_blocks(&[id(70), id(65)], 2).unwrap();
    assert_eq!(
        common.iter().map(|h| h.height).collect::<Vec<_>>(),
        [65, 70]
    );
    let unknown = BlockId::from_array([0x42; 32]);
    let common = services
        .common_blocks(&[id(3), unknown, id(60)], 3)
        .unwrap();
    assert_eq!(common.iter().map(|h| h.height).collect::<Vec<_>>(), [3, 60]);
    // A block in memory and not yet stored (applied in a running job) answers from memory only
    // when every id asked for is there: an id in capitals, dropped before, still counts.
    let applied = Arc::new(corpus[69].clone());
    chain.lock().unwrap().last_blocks.push_back(applied);
    let common = services.common_blocks(&[id(71)], 1).unwrap();
    assert_eq!(common.iter().map(|h| h.height).collect::<Vec<_>>(), [71]);
    assert!(services.common_blocks(&[id(71)], 2).unwrap().is_empty());
    chain.lock().unwrap().last_blocks.pop_back();
    assert_eq!(services.last_downloaded_block().height, tip);

    // The round and the slots, as the forger reads them.
    let block_ms = i64::try_from(milestones.at(1).block_time()).unwrap() * 1000;
    let epoch = heartwood_crypto::crypto::slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap();
    let last = corpus[68].header().timestamp;
    let now = epoch + i64::from(last) * 1000 + block_ms + 500;
    let round = services.current_round(now).unwrap();
    assert_eq!(round.current, calculate_round(tip + 1, &milestones).round);
    assert_eq!(round.last_block.height, tip);
    assert_eq!(round.delegates.len(), 53);
    assert_eq!(round.timestamp, i64::from(last) + block_ms / 1000);
    assert!(round.current_forger.is_some() && round.next_forger.is_some());
    assert!(round.all_delegates.len() >= 53, "every registered delegate");
    assert_eq!(
        services.slot_number(Some(i64::from(last)), now),
        i64::from(last) * 1000 / block_ms
    );
    assert_eq!(
        services.slot_number(None, now),
        i64::from(last) * 1000 / block_ms + 1
    );
    // The round's delegates and the two forgers in forging order, as getActiveDelegates gives
    // them, and the tip's data with its forger's name, as the reference's enqueueing names it.
    let state = handle.state().borrow().clone();
    let forging = state.round.forging_delegates().iter();
    let forging: Vec<String> = forging.map(|d| d.public_key.to_hex()).collect();
    let listed = round
        .delegates
        .iter()
        .map(|wallet| wallet.public_key.clone());
    assert_eq!(listed.collect::<Vec<_>>(), forging);
    let slot = |at: i64| forging[usize::try_from(at).unwrap() % forging.len()].clone();
    let info = heartwood_kernel::calculate_forging_info(
        heartwood_crypto::crypto::slots::epoch_time(now, epoch),
        tip + 1,
        &milestones,
    );
    let forger = round.current_forger.as_ref().map(|w| w.public_key.clone());
    assert_eq!(forger, Some(slot(info.current_forger)));
    let generator = corpus[68].header().generator_public_key;
    let name = state.wallets.find_by_public_key(&generator).unwrap();
    let name = name.delegate().unwrap().username();
    assert_eq!(round.last_block.fields["username"], name);
    let active = services.active_keys_next_round().unwrap();
    assert_eq!(active.len(), 53);
    let active_hex: Vec<String> = active.iter().map(|key| key.to_hex()).collect();
    assert_eq!(active_hex, forging, "getActiveDelegates' order");
    let name = round.delegates[0].username.clone();
    assert!(services.delegate_rank(&name).is_some());
    assert!(services.delegate_rank("nobody").is_none());
    assert!(services.already_forged(tip + 1, &name).is_ok());
    assert!(services.already_forged(tip + 1, "nobody").is_err());

    // The forger's events go to the bus.
    let mut events = bus.subscribe();
    services.forger_event(ForgerEvent::BlockForged(json!({})));
    assert_eq!(events.try_recv().unwrap(), Event::BlockForged);
    assert_eq!(services.peer_config(), peer_config());

    #[cfg(feature = "difftest")]
    {
        let state = heartwood_p2p_wire::internal::NetworkState::unknown();
        services.network_state_served(IpAddr::V4(Ipv4Addr::LOCALHOST), &state);
        let text = std::fs::read_to_string(record.join("network-state.jsonl")).unwrap();
        let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(
            (line["caller"].as_str(), line["status"].as_u64()),
            (Some("127.0.0.1"), Some(4))
        );
        assert!(line["t"].as_str().unwrap().ends_with('Z'));
    }
    #[cfg(not(feature = "difftest"))]
    let _ = record;
    drop(services);
    handle.stop().await;
    node.join().unwrap();
}
