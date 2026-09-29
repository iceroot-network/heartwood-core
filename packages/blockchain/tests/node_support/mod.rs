//! The node thread on the committed one-node chain, with a scripted block processor, a manual
//! clock, and recorders of what the node broadcasts, publishes and tells its listener and
//! observer.

#![allow(dead_code, reason = "each test file uses a part of the helpers")]

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use heartwood_blockchain::chain_state::{ChainState, SharedChainState};
use heartwood_blockchain::events::{CommitObserver, NodeEvent, NodeListener};
use heartwood_blockchain::pool_task::TransactionBroadcast;
use heartwood_blockchain::processor::{BlockProcess, ProcessContext, Processed, UnchainedStatus};
use heartwood_blockchain::{
    BlockProcessorResult, BlockchainOptions, ChainStore, IncomingBlock, Job, Node, NodeHandle,
    NodeParts, PoolTask, RestoreMode, Restored,
};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Block, Transaction, TransactionId};
use heartwood_database::{Database, Settings};
use heartwood_kernel::{ChainLink, ManualClock};
use heartwood_p2p::DownloadedBlock;
use heartwood_pool::{Options, Pool};
use heartwood_state::{State, StateView};
use heartwood_transactions::Handlers;
use tokio::sync::watch;

use crate::common::{self, Chain};

/// Lock `mutex`, a poisoned one included (a failed test poisons it).
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `block` as a peer's getBlocks reply lists it, after the reply check.
pub fn incoming(block: &Block) -> IncomingBlock {
    let header = block.header();
    let mut downloaded = DownloadedBlock {
        header: Default::default(),
        transactions: Some(block.transactions().to_vec()),
        ip: IpAddr::from([192, 0, 2, 1]),
    };
    let fields = &mut downloaded.header.header;
    fields.id = Some(block.id().to_hex());
    fields.version = Some(header.version);
    fields.timestamp = Some(header.timestamp);
    fields.previous_block = Some(hex::encode(header.previous_block));
    fields.height = Some(header.height);
    fields.number_of_transactions = Some(header.number_of_transactions);
    fields.payload_length = Some(header.payload_length);
    fields.payload_hash = Some(hex::encode(header.payload_hash));
    fields.generator_public_key = Some(header.generator_public_key.to_hex());
    fields.block_signature = Some(header.block_signature.to_hex());
    downloaded.header.total_amount = header.total_amount.into();
    downloaded.header.total_fee = header.total_fee.into();
    downloaded.header.reward = header.reward.into();
    IncomingBlock::downloaded(downloaded)
}

/// A job of the chain's blocks from `from` to `to`.
pub fn job(chain: &Chain, from: u32, to: u32) -> Job {
    let blocks = (from..=to)
        .map(|height| incoming(chain.block(height)))
        .collect();
    Job { blocks }
}

/// The time, in milliseconds since the Unix epoch, of `seconds` after the chain's epoch.
pub fn at_epoch_seconds(chain: &Chain, seconds: u32) -> i64 {
    let epoch = slots::epoch_unix_ms(chain.milestones().at(1).epoch()).unwrap();
    epoch + i64::from(seconds) * 1_000
}

/// What the scripted processor does with a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// Apply it, remove its transactions from the pool, move the last downloaded block up.
    Accept,
    /// Apply it, but leave the pool as it is (a processor that forgets the pool step).
    AcceptWithoutPool,
    /// Refuse it, as a refusing handler does.
    Reject,
    /// Discard it as already in the chain.
    Discard,
    /// A double-forged block of a round delegate.
    Rollback,
    /// Report a corrupted state after applying it.
    Corrupt,
}

/// What the scripted processor saw at one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The block's height.
    pub height: u32,
    /// How many blocks of the job were accepted and not committed before it.
    pub pending: usize,
    /// The height of the tip in the store, read through a connection of its own.
    pub stored: Option<u32>,
    /// The height of the published state.
    pub published: Option<u32>,
}

/// A block processor that follows a script by height (accepting by default) and records what it
/// saw.
#[derive(Default)]
pub struct Scripted {
    pub script: Mutex<BTreeMap<u32, Script>>,
    pub seen: Mutex<Vec<Seen>>,
    /// A second connection to the node's store.
    pub reader: Mutex<Option<Database>>,
    /// The node's published state.
    pub published: Mutex<Option<watch::Receiver<Arc<StateView>>>>,
    /// The processor that processes the blocks instead of the script, if any.
    pub inner: Option<Arc<dyn BlockProcess>>,
}

impl Scripted {
    pub fn with(script: &[(u32, Script)]) -> Arc<Scripted> {
        let scripted = Scripted::default();
        lock(&scripted.script).extend(script.iter().copied());
        Arc::new(scripted)
    }

    /// A recorder in front of `inner`.
    pub fn through(inner: Arc<dyn BlockProcess>) -> Arc<Scripted> {
        let inner = Some(inner);
        Arc::new(Scripted {
            inner,
            ..Scripted::default()
        })
    }

    pub fn heights(&self) -> Vec<u32> {
        lock(&self.seen).iter().map(|seen| seen.height).collect()
    }
}

impl BlockProcess for Scripted {
    fn process(
        &self,
        ctx: &mut ProcessContext<'_>,
        block: &Block,
        name: Option<&str>,
    ) -> Processed {
        let height = block.height();
        let stored = lock(&self.reader).as_ref().map(|reader| {
            let last = reader.last_block().unwrap();
            last.map_or(0, |last| last.row.height)
        });
        let published = lock(&self.published)
            .as_ref()
            .map(|view| view.borrow().height());
        let pending = ctx.pending.len();
        let seen = Seen {
            height,
            pending,
            stored,
            published,
        };
        lock(&self.seen).push(seen);
        if let Some(inner) = &self.inner {
            return inner.process(ctx, block, name);
        }
        let script = lock(&self.script).get(&height).copied();
        let processed = |result, unchained| Processed {
            result,
            applied: None,
            unchained,
            fault: false,
        };
        match script.unwrap_or(Script::Accept) {
            Script::Accept | Script::AcceptWithoutPool | Script::Corrupt => {
                let applied = ctx.state.apply_block(block, ctx.milestones, ctx.handlers);
                let applied = applied.unwrap();
                if script != Some(Script::AcceptWithoutPool) {
                    let params = ctx.milestones.at(height);
                    for transaction in block.transactions() {
                        ctx.pool.remove_forged(transaction, params);
                    }
                }
                ctx.chain.set_last_block(Arc::new(block.clone()));
                ctx.chain.last_downloaded = Some(ChainLink::of(block));
                ctx.chain.last_accepted_ms = Some(ctx.clock.now_ms());
                if script == Some(Script::Corrupt) {
                    return processed(BlockProcessorResult::Corrupted, None);
                }
                Processed {
                    result: BlockProcessorResult::Accepted,
                    applied: Some(applied),
                    unchained: None,
                    fault: false,
                }
            }
            Script::Reject => {
                ctx.chain.reset_last_downloaded();
                processed(BlockProcessorResult::Rejected, None)
            }
            Script::Discard => {
                ctx.chain.reset_last_downloaded();
                let status = Some(UnchainedStatus::AlreadyInBlockchain);
                processed(BlockProcessorResult::DiscardedButCanBeBroadcasted, status)
            }
            Script::Rollback => {
                ctx.chain.reset_last_downloaded();
                let status = Some(UnchainedStatus::DoubleForging);
                processed(BlockProcessorResult::Rollback, status)
            }
        }
    }
}

/// The transactions broadcast, batch by batch.
#[derive(Default)]
pub struct Broadcasts(pub Mutex<Vec<Vec<TransactionId>>>);

impl TransactionBroadcast for Broadcasts {
    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        let ids = transactions.iter().map(|transaction| *transaction.id());
        lock(&self.0).push(ids.collect());
    }
}

/// The events of each commit.
#[derive(Default)]
pub struct Events(pub Mutex<Vec<Vec<NodeEvent>>>);

impl NodeListener for Events {
    fn committed(&self, events: &[NodeEvent]) {
        lock(&self.0).push(events.to_vec());
    }
}

/// What the observer was told, as `applied <h> missed <n>`, `reverted <h>` and `restarted <h>`;
/// and, at each applied block, the tip it read from the shared chain state, when that was not
/// locked (a job keeps it locked only while it processes a block).
#[derive(Clone, Default)]
pub struct Observed(
    pub Arc<Mutex<Vec<String>>>,
    pub Arc<Mutex<Vec<Option<u32>>>>,
    pub Arc<OnceLock<SharedChainState>>,
);

impl CommitObserver for Observed {
    fn applied(&mut self, _state: &State, block: &Block) {
        panic!(
            "the node calls applied_with, not applied (block {})",
            block.height()
        );
    }

    fn applied_with(
        &mut self,
        state: &State,
        block: &Block,
        applied: &heartwood_state::machine::Applied,
    ) {
        assert_eq!(state.last_block(), block);
        let line = format!("applied {} missed {}", block.height(), applied.missed.len());
        lock(&self.0).push(line);
        if let Some(chain) = self.2.get() {
            let tip = chain.try_lock().ok().map(|chain| chain.last_height());
            lock(&self.1).push(tip);
        }
    }

    fn reverted(&mut self, state: &State) {
        lock(&self.0).push(format!("reverted {}", state.height()));
    }

    fn restarted(&mut self, state: &State) {
        lock(&self.0).push(format!("restarted {}", state.height()));
    }
}

/// A node on the committed chain, in its own directory.
pub struct TestNode {
    pub node: Option<Node>,
    pub handle: NodeHandle,
    pub dir: PathBuf,
    pub chain: SharedChainState,
    pub clock: Arc<ManualClock>,
    pub processor: Arc<Scripted>,
    pub broadcasts: Arc<Broadcasts>,
    pub events: Arc<Events>,
    pub observed: Observed,
}

/// The pool options of a node in `dir`, with `max_transaction_age`.
pub fn pool_options(dir: &Path, max_transaction_age: u32) -> Options {
    let mut options = Options::defaults(&|_| None, dir, 2);
    options.max_transaction_age = max_transaction_age;
    options
}

impl TestNode {
    /// Start a node from the store in `dir` (a first start when it is empty), with the clock a
    /// minute after the chain's last block.
    pub fn start(chain: &Chain, dir: &Path, processor: Arc<Scripted>, options: Options) -> Self {
        let clock = Arc::new(ManualClock::new(at_epoch_seconds(
            chain,
            chain.blocks.last().unwrap().header().timestamp + 60,
        )));
        let (store, restored) = common::start(dir, &chain.files, RestoreMode::Auto).unwrap();
        TestNode::spawn(chain, dir, processor, options, clock, store, restored)
    }

    pub fn spawn(
        chain: &Chain,
        dir: &Path,
        processor: Arc<Scripted>,
        options: Options,
        clock: Arc<ManualClock>,
        store: ChainStore,
        restored: Restored,
    ) -> Self {
        let own = Arc::new(heartwood_p2p::NoAttestedKeys);
        TestNode::spawn_with(
            chain,
            dir,
            processor,
            options,
            (clock, store, restored),
            own,
        )
    }

    /// [`TestNode::spawn`] with the delegates on this node `own_keys`.
    pub fn spawn_with(
        chain: &Chain,
        dir: &Path,
        processor: Arc<Scripted>,
        options: Options,
        (clock, store, restored): (Arc<ManualClock>, ChainStore, Restored),
        own_keys: Arc<dyn heartwood_blockchain::OwnKeys>,
    ) -> Self {
        *lock(&processor.reader) = Some(Database::open(&Settings::in_data_dir(dir)).unwrap());
        let pool = Pool::open(options, None).unwrap();
        let tip = Arc::new(restored.state.last_block().clone());
        let shared: SharedChainState = Arc::new(Mutex::new(ChainState::at(tip)));
        let broadcasts = Arc::new(Broadcasts::default());
        let events = Arc::new(Events::default());
        let observed = Observed::default();
        let _ = observed.2.set(Arc::clone(&shared));
        let parts = NodeParts {
            state: restored.state,
            store,
            pool: PoolTask::new(pool, restored.online),
            chain: Arc::clone(&shared),
            processor: processor.clone(),
            handlers: Handlers,
            milestones: Arc::new(chain.milestones().clone()),
            network: Arc::new(chain.files.network.clone()),
            options: Arc::new(BlockchainOptions::default()),
            clock: clock.clone(),
            listener: events.clone(),
            observer: Some(Box::new(observed.clone())),
            broadcast: broadcasts.clone(),
            own_keys,
            version: "4.3.1-heartwood.0.1.0".into(),
        };
        let (node, handle) = Node::spawn(parts).unwrap();
        *lock(&processor.published) = Some(handle.state());
        TestNode {
            node: Some(node),
            handle,
            dir: dir.to_path_buf(),
            chain: shared,
            clock,
            processor,
            broadcasts,
            events,
            observed,
        }
    }

    /// Run the chain's blocks from `from` to `to` as one job, enqueued now.
    pub async fn run(&self, chain: &Chain, from: u32, to: u32) -> heartwood_blockchain::JobOutcome {
        let job = self.enqueued(job(chain, from, to));
        self.handle.job(job).await.unwrap()
    }

    /// `job` with its blocks named from the published state, as the enqueueing names them.
    pub fn enqueued(&self, mut job: Job) -> Job {
        let state = self.handle.state().borrow().clone();
        for block in &mut job.blocks {
            block.set_username(&state.wallets);
        }
        job
    }

    /// The published state's height.
    pub fn height(&self) -> u32 {
        self.handle.state().borrow().height()
    }

    /// Stop the node and wait for its thread.
    pub async fn stop(mut self) -> Result<(), heartwood_blockchain::NodeError> {
        self.handle.stop().await;
        self.node.take().unwrap().join()
    }
}

/// The state after the chain's blocks up to `height`, applied one by one.
pub fn replayed(chain: &Chain, height: u32) -> State {
    let mut state = State::from_genesis(&chain.genesis(), chain.milestones(), &Handlers).unwrap();
    for height in 2..=height {
        state
            .apply_block(chain.block(height), chain.milestones(), &Handlers)
            .unwrap();
    }
    state
}
