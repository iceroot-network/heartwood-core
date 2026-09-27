//! The node as the peer-to-peer client and the API read it, over a fake node loop: the chain view
//! of the shared chain state and the published state, the block lookup of the in-memory blocks and
//! the store's history, the online record's updates, the submission of pool entries, and the node
//! thread's listener with its ordered calls.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{Setup, corpus, run};
use heartwood_api::{PeerList, SubmitTransactions};
use heartwood_blockchain::events::{NodeEvent, NodeListener};
use heartwood_blockchain::pool_task::TransactionBroadcast;
use heartwood_blockchain::rows::block_row;
use heartwood_blockchain::{ChainState, ChainStatus, Channel, NodeHandle, OnlineWork, Work};
use heartwood_core::app::restored;
use heartwood_core::config::RelayConfig;
use heartwood_core::views::{
    BusEvents, History, Links, Listener, Machine, NodeView, Peers, Submit,
};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, Milestones, PublicKeyBytes, Transaction};
use heartwood_database::{BlockRow, RoundRow};
use heartwood_kernel::bus::EventBus;
use heartwood_kernel::{Event, RoundInfo};
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_p2p::{BlockLookup, ChainView, OnlineSink, PeerEvents};
use heartwood_pool::view::PoolViewOptions;
use heartwood_pool::{Entry, Options as PoolOptions, PoolView, ProcessorResult};
use heartwood_state::StateView;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

/// The corpus's blocks after the genesis block.
fn blocks(milestones: &Milestones) -> Vec<Block> {
    let text = std::fs::read_to_string(corpus().join("blocks.jsonl")).unwrap();
    let block = |line: &str| {
        let record: Value = serde_json::from_str(line).unwrap();
        Block::decode_trusted(
            &hex::decode(record["hex"].as_str().unwrap()).unwrap(),
            milestones,
        )
        .unwrap()
    };
    text.lines().map(block).collect()
}

/// A fake history: rows of stored blocks and rounds.
#[derive(Default)]
struct Stored {
    blocks: Vec<BlockRow>,
    rounds: Vec<RoundRow>,
}

impl History for Stored {
    fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockRow>, LookupError> {
        Ok(self
            .blocks
            .iter()
            .filter(|row| heights.contains(&row.height))
            .cloned()
            .collect())
    }
    fn round(&self, round: u64) -> Result<Vec<RoundRow>, LookupError> {
        Ok(self
            .rounds
            .iter()
            .filter(|row| row.round == round)
            .cloned()
            .collect())
    }
}

/// A fake machine.
struct FakeMachine(bool);

impl Machine for FakeMachine {
    fn is_synced(&self) -> bool {
        self.0
    }
    fn is_idle(&self) -> bool {
        self.0
    }
}

/// A fake monitor that counts its calls.
#[derive(Default)]
struct FakePeers {
    pings: AtomicUsize,
    disconnects: AtomicUsize,
    broadcasts: AtomicUsize,
}

impl Peers for FakePeers {
    fn peers(&self) -> Vec<PeerInfo> {
        Vec::new()
    }
    fn network_height(&self) -> u32 {
        7
    }
    fn ping_all(&self) {
        self.pings.fetch_add(1, Ordering::SeqCst);
    }
    fn disconnect_invalid_peers(&self) {
        self.disconnects.fetch_add(1, Ordering::SeqCst);
    }
    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        self.broadcasts
            .fetch_add(transactions.len(), Ordering::SeqCst);
    }
}

/// The fake node's publishers, kept so its receivers stay open.
type Senders = (
    watch::Sender<Arc<StateView>>,
    watch::Sender<Arc<PoolView>>,
    watch::Sender<Arc<ChainStatus>>,
);

/// A node at the corpus's genesis block: its view, its work queue and its milestones.
struct Fixture {
    _setup: Setup,
    view: NodeView,
    work: mpsc::UnboundedReceiver<Work>,
    milestones: Arc<Milestones>,
    _senders: Senders,
}

fn fixture(name: &str, stored: Stored) -> Fixture {
    let setup = Setup::new(name, "");
    let config = RelayConfig::load(
        &run("--network testnet"),
        &setup.process(&[]),
        &setup.home(),
        &setup.root,
    )
    .unwrap();
    let (_, restored) = restored(&config).unwrap();
    let milestones = Arc::new(config.crypto.milestones.clone());
    let (work, queue) = mpsc::unbounded_channel();
    let (state, state_view) = watch::channel(Arc::new(restored.state.view()));
    let pool_view = PoolView {
        by_priority: Vec::new(),
        online: Vec::new(),
        options: PoolViewOptions::of(&PoolOptions::defaults(&|_| None, &setup.root, 2)),
    };
    let (pool, pool_view) = watch::channel(Arc::new(pool_view));
    let (chain, chain_status) = watch::channel(Arc::new(ChainStatus::default()));
    let tip = Arc::new(restored.state.last_block().clone());
    let view = NodeView {
        handle: NodeHandle::from_channels(work, state_view, pool_view, chain_status),
        chain: Arc::new(Mutex::new(ChainState::at(tip))),
        milestones: Arc::clone(&milestones),
        network: Arc::new(config.crypto.network.clone()),
        history: Arc::new(stored),
        links: Arc::new(Links::default()),
    };
    Fixture {
        _setup: setup,
        view,
        work: queue,
        milestones,
        _senders: (state, pool, chain),
    }
}

#[test]
fn the_chain_view_reads_the_shared_chain_state_and_the_published_state() {
    let mut node = fixture("chain-view", Stored::default());
    let view = &node.view;
    assert_eq!(view.last_block().height(), 1);
    assert_eq!(view.last_blocks().len(), 1);
    let keys = view.delegate_keys();
    assert_eq!(keys.len(), 53);
    let first = view.delegate_by_key(&keys[0]).unwrap();
    assert!(
        first.rank.is_some() && !first.username.is_empty(),
        "{first:?}"
    );
    assert_eq!(
        view.delegate_by_key(&PublicKeyBytes::from_hex(&format!("02{}", "11".repeat(32))).unwrap()),
        None
    );
    assert_eq!(view.network().name, "devnet");
    assert!(view.block_ping().is_none());
    // The blocks a job accepts are read at once, before the state is published.
    let second = Arc::new(blocks(&node.milestones).remove(0));
    {
        let mut chain = view.chain.lock().unwrap();
        chain.set_last_block(Arc::clone(&second));
        chain.push_ping_block(*second.id(), 2, false, 1_000);
    }
    assert_eq!(view.last_block().height(), 2);
    assert_eq!(
        view.last_blocks()
            .iter()
            .map(|b| b.height())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(view.block_ping().map(|ping| ping.height), Some(2));
    // Not synced and not idle until the machine is linked.
    assert!(!view.is_synced() && !view.is_idle());
    let _ = view.links.machine.set(Arc::new(FakeMachine(true)));
    assert!(view.is_synced() && view.is_idle());
    // The attested versions go to the node thread's online record.
    view.set_version(keys[0], 3, "4.3.1".into());
    match node.work.try_recv() {
        Ok(Work::Online(OnlineWork::Set {
            key,
            round,
            version,
        })) => {
            assert_eq!((key, round, version.as_str()), (keys[0], 3, "4.3.1"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_block_lookup_reads_memory_first_then_the_store_in_the_order_asked() {
    let setup = Setup::new("lookup-rows", "");
    let config = RelayConfig::load(
        &run("--network testnet"),
        &setup.process(&[]),
        &setup.home(),
        &setup.root,
    )
    .unwrap();
    let milestones = config.crypto.milestones.clone();
    let chain = blocks(&milestones);
    let rows = chain
        .iter()
        .take(5)
        .map(|block| block_row(block, Some("x"), &milestones).unwrap())
        .collect();
    let keys: Vec<PublicKeyBytes> = (1..=3)
        .map(|n| PublicKeyBytes::from_hex(&format!("02{}", format!("{n:02x}").repeat(32))).unwrap())
        .collect();
    let rounds = keys
        .iter()
        .map(|key| RoundRow {
            round: 2,
            public_key: key.to_hex(),
            balance: 1,
        })
        .collect();
    let node = fixture(
        "lookup",
        Stored {
            blocks: rows,
            rounds,
        },
    );
    let view = &node.view;
    let headers = view.blocks_by_height(&[4, 1, 99, 3]).unwrap();
    let heights: Vec<u32> = headers.iter().map(|header| header.height).collect();
    assert_eq!(heights, [4, 1, 3]);
    assert_eq!(headers[0], *chain[2].header());
    let round = |round: u64| RoundInfo {
        round,
        round_height: 1,
        next_round: round,
        max_delegates: 53,
    };
    assert_eq!(view.round_delegate_keys(round(2)).unwrap(), keys);
    // A round that is not stored: the current round's delegates.
    assert_eq!(view.round_delegate_keys(round(9)).unwrap().len(), 53);
}

#[test]
fn the_listener_emits_on_the_bus_and_calls_the_monitor_in_order() {
    let bus = EventBus::new();
    let mut heard = bus.subscribe();
    let links = Arc::new(Links::default());
    let listener = Listener {
        bus: bus.clone(),
        links: Arc::clone(&links),
    };
    // Before the monitor is linked, nothing is called and nothing fails.
    listener.committed(&[NodeEvent::MilestoneChanged { height: 5 }]);
    let peers = Arc::new(FakePeers::default());
    let _ = links.peers.set(peers.clone());
    let round = NodeEvent::BlockApplied {
        height: 54,
        round_started: Some(2),
    };
    listener.committed(&[
        NodeEvent::BlockApplied {
            height: 53,
            round_started: None,
        },
        round.clone(),
    ]);
    // Not synced: no pingAll at the round.
    assert_eq!(peers.pings.load(Ordering::SeqCst), 0);
    let _ = links.machine.set(Arc::new(FakeMachine(true)));
    listener.committed(&[
        round,
        NodeEvent::BlockReverted { height: 54 },
        NodeEvent::MilestoneChanged { height: 54 },
    ]);
    assert_eq!(peers.pings.load(Ordering::SeqCst), 1);
    assert_eq!(peers.disconnects.load(Ordering::SeqCst), 1);
    let mut events = Vec::new();
    while let Ok(event) = heard.try_recv() {
        events.push(event);
    }
    use Event::*;
    assert_eq!(
        events,
        [
            MilestoneChanged,
            BlockApplied,
            BlockApplied,
            RoundApplied,
            BlockApplied,
            RoundApplied,
            BlockReverted,
            MilestoneChanged
        ]
    );
    // The broadcast and the peer list go to the monitor.
    links.broadcast_transactions(&[]);
    assert_eq!(peers.broadcasts.load(Ordering::SeqCst), 0);
    assert!(PeerList::peers(links.as_ref()).is_empty());
    let mut bus_events = bus.subscribe();
    BusEvents(bus).emit(Event::PeerAdded);
    assert_eq!(bus_events.try_recv(), Ok(Event::PeerAdded));
}

#[tokio::test]
async fn a_submission_goes_to_the_api_channel_and_answers_the_processors_result() {
    let mut node = fixture("submit", Stored::default());
    let submit = Submit(node.view.handle.clone());
    let answer = tokio::spawn(submit.submit(vec![Entry::Data(json!({"id": "a"}))]));
    match node.work.recv().await {
        Some(Work::PoolEntries(Channel::Api, entries, reply)) => {
            assert_eq!(entries, [Entry::Data(json!({"id": "a"}))]);
            let result = ProcessorResult {
                accept: vec!["a".into()],
                ..ProcessorResult::default()
            };
            reply.send(result).unwrap();
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(answer.await.unwrap().unwrap().accept, ["a"]);
    // A node that has stopped takes no submission.
    drop(node.work);
    assert!(submit.submit(Vec::new()).await.is_err());
}
