//! Test support for the blockchain service: a clock on tokio's time (paused in the tests), a fake
//! network with scripted answers, a job queue with the reference's memory queue rules, and a node
//! loop that answers the service's work from the committed chain as the processor would.

#![allow(dead_code, reason = "each test file uses a part of the support")]

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use heartwood_blockchain::blockchain::Queue;
use heartwood_blockchain::chain_state::lock;
use heartwood_blockchain::queue::QueueEvents;
use heartwood_blockchain::{
    BlockProcessorResult, Blockchain, BlockchainOptions, BlockchainParts, ChainState,
    IncomingBlock, Job, JobEnd, JobOutcome, MachineStatus, Network, NodeError, NodeHandle,
    OnlineWork, RollbackDepth, SharedChainState, Work,
};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Block, BlockId, Milestones, Transaction, TransactionId};
use heartwood_kernel::{ChainLink, Clock, is_block_chained};
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::errors::{DownloadError, SeedError};
use heartwood_p2p::monitor::{CleanseOptions, NetworkStatus};
use heartwood_pool::{Options, PoolView, PoolViewOptions, ProcessorResult};
use heartwood_state::{State, StateView};
use heartwood_transactions::Handlers;
use tokio::sync::{Notify, mpsc, watch};

use crate::common::Chain;

/// The peer every test block comes from.
pub const PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));

/// The committed chain, loaded once per test binary.
pub fn c1() -> &'static Chain {
    static CHAIN: OnceLock<Chain> = OnceLock::new();
    CHAIN.get_or_init(Chain::load)
}

/// The chain's block at `height`, the genesis block at 1.
pub fn block(height: u32) -> Block {
    match height {
        1 => c1().genesis(),
        _ => c1().block(height).clone(),
    }
}

/// `block` as a peer's getBlocks reply gives it, from the peer [`PEER`].
pub fn downloaded(block: &Block) -> DownloadedBlock {
    let header = block.header();
    let mut downloaded = DownloadedBlock {
        header: Default::default(),
        transactions: Some(block.transactions().to_vec()),
        ip: PEER,
    };
    let fields = &mut downloaded.header.header;
    fields.id = Some(block.id().to_hex());
    fields.version = Some(header.version);
    fields.timestamp = Some(header.timestamp);
    fields.previous_block = Some(BlockId::from_array(header.previous_block).to_hex());
    fields.height = Some(header.height);
    fields.number_of_transactions = Some(header.number_of_transactions);
    fields.payload_length = Some(header.payload_length);
    fields.payload_hash = Some(hex::encode(header.payload_hash));
    fields.generator_public_key = Some(header.generator_public_key.to_hex());
    fields.block_signature = Some(header.block_signature.to_hex());
    downloaded.header.total_amount = header.total_amount.into();
    downloaded.header.total_fee = header.total_fee.into();
    downloaded.header.reward = header.reward.into();
    downloaded
}

/// The chain's blocks from `from` to `to` as a peer gives them.
pub fn range(from: u32, to: u32) -> Vec<DownloadedBlock> {
    (from..=to)
        .map(|height| downloaded(&block(height)))
        .collect()
}

/// `blocks` as incoming blocks.
pub fn incoming(blocks: Vec<DownloadedBlock>) -> Vec<IncomingBlock> {
    blocks.into_iter().map(IncomingBlock::downloaded).collect()
}

/// The heights of a job's blocks.
pub fn heights(job: &Job) -> Vec<u32> {
    job.blocks.iter().map(IncomingBlock::height).collect()
}

/// Milliseconds since the Unix epoch of the chain's epoch.
pub fn epoch_ms() -> i64 {
    slots::epoch_unix_ms(c1().milestones().at(1).epoch()).unwrap()
}

/// The time `seconds` after the network epoch, in milliseconds since the Unix epoch.
pub fn at(seconds: i64) -> i64 {
    epoch_ms() + seconds * 1000
}

/// The time of the block at `height`, in milliseconds since the Unix epoch.
pub fn time_of(height: u32) -> i64 {
    at(block(height).header().timestamp.into())
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A clock that reads tokio's time from `base_ms` on, so that the paused time of a test moves
/// it.
pub struct TokioClock {
    base_ms: i64,
    start: tokio::time::Instant,
}

impl TokioClock {
    pub fn new(base_ms: i64) -> TokioClock {
        TokioClock {
            base_ms,
            start: tokio::time::Instant::now(),
        }
    }
}

impl Clock for TokioClock {
    fn now_ms(&self) -> i64 {
        self.base_ms + i64::try_from(self.start.elapsed().as_millis()).unwrap()
    }
}

/// How the fake network serves blocks.
#[derive(Clone)]
pub struct Served {
    /// The blocks, by height.
    pub blocks: BTreeMap<u32, DownloadedBlock>,
    /// The chunk size.
    pub chunk: u32,
    /// How many peers serve a chunk each.
    pub peers: u32,
}

/// What the fake network answers and what it was asked.
#[derive(Default)]
pub struct NetState {
    /// The calls, in order, by name (with their main argument).
    pub calls: Vec<String>,
    /// The number of peers.
    pub peers: usize,
    /// The next answers of `download_blocks_from_height`, before [`NetState::served`].
    pub downloads: VecDeque<Vec<DownloadedBlock>>,
    /// The chain the downloads serve once the scripted answers are used up.
    pub served: Option<Served>,
    /// The next answers of `check_for_fork` (0 once used up).
    pub forks: VecDeque<u32>,
    /// The next answers of `check_network_health` (not forked once used up).
    pub health: VecDeque<NetworkStatus>,
    /// The answer of `download_block_at_height`.
    pub at_height: Option<DownloadedBlock>,
    /// The heights of the broadcast blocks.
    pub broadcasts: Vec<u32>,
    /// Whether the boot fails.
    pub boot_fails: bool,
    /// Whether the node knows the minimum number of peers.
    pub minimum: bool,
    /// What `download_transactions` answers.
    pub transactions: Vec<Vec<u8>>,
}

/// A network with scripted answers.
#[derive(Clone, Default)]
pub struct FakeNetwork(pub Arc<Mutex<NetState>>);

impl FakeNetwork {
    pub fn state(&self) -> MutexGuard<'_, NetState> {
        locked(&self.0)
    }

    fn call(&self, name: String) {
        self.state().calls.push(name);
    }

    /// The calls made, by name.
    pub fn calls(&self) -> Vec<String> {
        self.state().calls.clone()
    }

    /// How many calls start with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }
}

impl Network for FakeNetwork {
    async fn boot(&self) -> Result<(), SeedError> {
        self.call("boot".into());
        match self.state().boot_fails {
            true => Err(SeedError::NoSeeds),
            false => Ok(()),
        }
    }

    async fn update_network_status(&self, initial: bool) {
        self.call(format!("update_network_status {initial}"));
    }

    async fn cleanse_peers(&self, options: CleanseOptions) {
        let CleanseOptions {
            fast,
            force_ping,
            peer_count,
            skip_common_blocks,
            ..
        } = options;
        let count = peer_count.map_or("all".into(), |n| n.to_string());
        self.call(format!(
            "cleanse fast={fast} force={force_ping} count={count} skip={skip_common_blocks}"
        ));
    }

    async fn discover_peers(&self, ping_all: bool, add_all: bool, silent: bool) -> bool {
        self.call(format!("discover {ping_all} {add_all} {silent}"));
        false
    }

    fn network_height(&self) -> u32 {
        0
    }

    fn peer_count(&self) -> usize {
        self.state().peers
    }

    async fn check_network_health(&self, fast: bool) -> NetworkStatus {
        self.call(format!("health {fast}"));
        let answer = self.state().health.pop_front();
        answer.unwrap_or(NetworkStatus {
            forked: false,
            blocks_to_rollback: None,
        })
    }

    async fn check_for_fork(&self) -> u32 {
        self.call("check_for_fork".into());
        let answer = self.state().forks.pop_front();
        answer.unwrap_or(0)
    }

    async fn download_block_at_height(
        &self,
        ip: IpAddr,
        height: u32,
    ) -> Result<Option<DownloadedBlock>, DownloadError> {
        self.call(format!("download_block_at_height {ip} {height}"));
        Ok(self.state().at_height.clone())
    }

    async fn download_blocks_from_height(
        &self,
        from: u32,
        max_parallel: usize,
        silent: bool,
        timeout: Option<Duration>,
        check_throttle: bool,
    ) -> Vec<DownloadedBlock> {
        let ms = timeout.map_or("configured".to_owned(), |t| t.as_millis().to_string());
        self.call(format!(
            "download {from} parallel={max_parallel} silent={silent} timeout={ms} throttle={check_throttle}"
        ));
        let mut state = self.state();
        if let Some(answer) = state.downloads.pop_front() {
            return answer;
        }
        let Some(served) = &state.served else {
            return Vec::new();
        };
        let most = served.chunk * served.peers.min(u32::try_from(max_parallel).unwrap());
        let to = from.saturating_add(most);
        served
            .blocks
            .range(from + 1..=to)
            .map(|(_, b)| b.clone())
            .collect()
    }

    async fn download_transactions(&self, exclude: &[TransactionId]) -> Vec<Vec<u8>> {
        self.call(format!("download_transactions {}", exclude.len()));
        self.state().transactions.clone()
    }

    async fn broadcast_block(&self, block: &Block) {
        self.call(format!("broadcast_block {}", block.height()));
        self.state().broadcasts.push(block.height());
    }

    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        self.call(format!("broadcast_transactions {}", transactions.len()));
    }

    fn has_minimum_peers(&self, silent: bool) -> bool {
        self.call(format!("has_minimum_peers {silent}"));
        self.state().minimum
    }

    async fn populate_seed_peers(&self) -> Result<(), SeedError> {
        self.call("populate_seed_peers".into());
        Ok(())
    }

    fn ping_all(&self) {
        self.call("ping_all".into());
    }

    fn synced(&self) {
        self.call("synced".into());
    }

    async fn refresh_peers_after_fork(&self) {
        self.call("refresh_peers_after_fork".into());
    }

    fn disconnect_invalid_peers(&self) {
        self.call("disconnect_invalid_peers".into());
    }

    fn is_cold_start(&self) -> bool {
        false
    }

    fn complete_cold_start(&self) {}
}

/// The queue's state.
#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Job>,
    paused: bool,
    running: bool,
    processing: bool,
}

struct QueueShared {
    node: NodeHandle,
    events: Arc<dyn QueueEvents>,
    runtime: tokio::runtime::Handle,
    state: Mutex<QueueState>,
    idle: Notify,
    /// Every pushed job, in order.
    pushed: Mutex<Vec<Job>>,
}

/// A job queue with the rules of the node's queue and the reference's memory queue: `push` and
/// `resume` start the processing unless it runs or the queue is paused; it runs the jobs one at a
/// time, reports each outcome before the next job, and when no job waits at its end (even when
/// there was none to run) has the node drain, then reports `drained`; `pause` and `stop` hold the
/// jobs not started.
#[derive(Clone)]
pub struct ScriptQueue(Arc<QueueShared>);

impl ScriptQueue {
    pub fn new(
        node: NodeHandle,
        events: Arc<dyn QueueEvents>,
        runtime: tokio::runtime::Handle,
    ) -> ScriptQueue {
        ScriptQueue(Arc::new(QueueShared {
            node,
            events,
            runtime,
            state: Mutex::default(),
            idle: Notify::new(),
            pushed: Mutex::default(),
        }))
    }

    /// Every job pushed so far.
    pub fn pushed(&self) -> Vec<Job> {
        locked(&self.0.pushed).clone()
    }

    fn process(&self) {
        let mut state = locked(&self.0.state);
        if !state.processing && !state.paused {
            (state.processing, state.running) = (true, !state.jobs.is_empty());
            let shared = Arc::clone(&self.0);
            self.0.runtime.spawn(async move { run_jobs(shared).await });
        }
    }
}

async fn run_jobs(shared: Arc<QueueShared>) {
    loop {
        let job = {
            let mut state = locked(&shared.state);
            let job = (!state.paused).then(|| state.jobs.pop_front()).flatten();
            state.running |= job.is_some();
            job
        };
        let Some(job) = job else {
            break;
        };
        if let Ok(outcome) = shared.node.job(job).await {
            shared.events.finished(outcome);
        }
    }
    let drain = {
        let mut state = locked(&shared.state);
        (state.running, state.processing) = (false, false);
        state.jobs.is_empty()
    };
    shared.idle.notify_waiters();
    if drain && shared.node.drain().await.is_ok() {
        shared.events.drained();
    }
}

impl Queue for ScriptQueue {
    fn push(&self, job: Job) {
        locked(&self.0.pushed).push(job.clone());
        locked(&self.0.state).jobs.push_back(job);
        self.process();
    }

    fn pause(&self) {
        locked(&self.0.state).paused = true;
    }

    fn resume(&self) {
        locked(&self.0.state).paused = false;
        self.process();
    }

    fn clear(&self) {
        locked(&self.0.state).jobs.clear();
    }

    async fn stop(&self) {
        let idle = {
            let mut state = locked(&self.0.state);
            (state.paused, state.jobs) = (true, VecDeque::new());
            state.running.then(|| self.0.idle.notified())
        };
        if let Some(idle) = idle {
            idle.await;
        }
    }

    fn size(&self) -> usize {
        locked(&self.0.state).jobs.len()
    }

    fn is_running(&self) -> bool {
        locked(&self.0.state).running
    }
}

/// What the fake node thread does and was asked.
#[derive(Default)]
pub struct NodeScript {
    /// The work received, by name.
    pub log: Vec<String>,
    /// The result of the block at a height, instead of accepting it.
    pub results: BTreeMap<u32, BlockProcessorResult>,
    /// Whether a removal fails.
    pub removal_fails: bool,
    /// How long each job takes.
    pub job_time: Duration,
}

/// A node loop that answers the service's work from the committed chain: a job accepts each
/// block that is the chain's block at its height (or gives the scripted result), and moves the
/// chain state's tip as the processor would; a removal moves the tip back.
#[derive(Clone)]
pub struct FakeNode {
    pub script: Arc<Mutex<NodeScript>>,
    pub chain: SharedChainState,
    pub clock: Arc<TokioClock>,
}

impl FakeNode {
    pub fn script(&self) -> MutexGuard<'_, NodeScript> {
        locked(&self.script)
    }

    /// The work received, by name.
    pub fn log(&self) -> Vec<String> {
        self.script().log.clone()
    }

    fn record(&self, entry: String) {
        self.script().log.push(entry);
    }

    fn job(&self, job: &Job) -> JobOutcome {
        let blocks = &job.blocks;
        self.record(format!("job {:?}", heights(job)));
        let outcome = |end, results, committed| JobOutcome {
            results,
            committed,
            clear_queue: false,
            end,
        };
        let mut chain = lock(&self.chain);
        let (Some(first), Some(tip)) = (blocks.first(), chain.last_block().cloned()) else {
            return outcome(JobEnd::Dropped, Vec::new(), 0);
        };
        if first.height() <= tip.height() {
            return outcome(JobEnd::Dropped, Vec::new(), 0);
        }
        let link = link(first);
        if is_block_chained(&ChainLink::of(&tip), &link, c1().milestones()).is_err() {
            return outcome(JobEnd::ForkCheck(blocks.clone()), Vec::new(), 0);
        }
        let (mut results, mut committed, mut end) = (Vec::new(), 0, JobEnd::Clear);
        for incoming in blocks {
            let height = incoming.height();
            let known = Arc::new(block(height));
            let scripted = self.script().results.get(&height).copied();
            let result = scripted.unwrap_or(match link_id(incoming) == *known.id() {
                true => BlockProcessorResult::Accepted,
                false => BlockProcessorResult::Rejected,
            });
            results.push((link_id(incoming), result));
            match result {
                BlockProcessorResult::Accepted => {
                    committed += 1;
                    chain.set_last_block(Arc::clone(&known));
                    if chain
                        .last_downloaded
                        .is_none_or(|last| last.height < height)
                    {
                        chain.last_downloaded = Some(ChainLink::of(&known));
                    }
                    if chain.started {
                        chain.last_accepted_ms = Some(self.clock.now_ms());
                    }
                    chain.last_stored_height = height;
                    end = JobEnd::Broadcast(known);
                }
                BlockProcessorResult::DiscardedButCanBeBroadcasted => {
                    end = JobEnd::Broadcast(known);
                    break;
                }
                BlockProcessorResult::Rollback => {
                    chain.last_downloaded = Some(ChainLink::of(&known));
                    end = JobEnd::Fork(known);
                    break;
                }
                _ => {
                    chain.reset_last_downloaded();
                    end = JobEnd::Clear;
                    break;
                }
            }
        }
        outcome(end, results, committed)
    }

    fn remove(&self, count: u32) -> Result<u32, NodeError> {
        self.record(format!("remove {count}"));
        if self.script().removal_fails {
            return Err(NodeError::Revert {
                height: lock(&self.chain).last_height(),
                reason: "scripted".into(),
            });
        }
        let mut chain = lock(&self.chain);
        let tip = chain.last_height();
        let count = count.min(tip - 1);
        let height = tip - count;
        chain.set_last_block(Arc::new(block(height)));
        chain.last_stored_height = height;
        Ok(height)
    }

    /// Answer the work of `work` until the service's handle is dropped.
    pub fn spawn(self, mut work: mpsc::UnboundedReceiver<Work>) {
        tokio::spawn(async move {
            while let Some(item) = work.recv().await {
                match item {
                    Work::Job(job, answer) => {
                        let time = self.script().job_time;
                        if !time.is_zero() {
                            tokio::time::sleep(time).await;
                        }
                        let _ = answer.send(self.job(&job));
                    }
                    Work::PoolEntries(channel, entries, answer) => {
                        self.record(format!("pool {channel:?} {}", entries.len()));
                        let _ = answer.send(ProcessorResult::default());
                    }
                    Work::Drain(answer, _) => {
                        self.record("drain".into());
                        let _ = answer.send(Ok(()));
                    }
                    Work::RemoveBlocks(count, answer) => {
                        let _ = answer.send(self.remove(count));
                    }
                    Work::Online(OnlineWork::Expire { round }) => {
                        self.record(format!("expire {round}"));
                    }
                    Work::Online(OnlineWork::Set { round, .. }) => {
                        self.record(format!("set {round}"));
                    }
                    Work::Stop(answer) => {
                        self.record("stop".into());
                        let _ = answer.send(());
                    }
                }
            }
        });
    }
}

/// The chain link of a downloaded block.
pub fn link(block: &IncomingBlock) -> ChainLink {
    let fields = &block.block.header.header;
    ChainLink {
        id: link_id(block),
        previous_block: BlockId::from_hex(fields.previous_block.as_deref().unwrap()).unwrap(),
        height: fields.height.unwrap(),
        timestamp: fields.timestamp.unwrap(),
    }
}

fn link_id(block: &IncomingBlock) -> BlockId {
    BlockId::from_hex(block.block.header.header.id.as_deref().unwrap()).unwrap()
}

/// The genesis state's view, built once per test binary.
pub fn genesis_view() -> Arc<StateView> {
    static VIEW: OnceLock<Arc<StateView>> = OnceLock::new();
    let view = VIEW.get_or_init(|| {
        let state = State::from_genesis(&c1().genesis(), c1().milestones(), &Handlers).unwrap();
        Arc::new(state.view())
    });
    Arc::clone(view)
}

/// How a test's service starts.
pub struct Setup {
    /// The height of the tip.
    pub tip: u32,
    /// The time, in milliseconds since the Unix epoch.
    pub now_ms: i64,
    /// The number of peers.
    pub peers: usize,
    /// The options.
    pub options: BlockchainOptions,
    /// The milestones, when not the chain's.
    pub milestones: Option<Milestones>,
    /// The rollback depth.
    pub depth: RollbackDepth,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            tip: 1,
            now_ms: time_of(1) + 1_000,
            peers: 3,
            options: BlockchainOptions::default(),
            milestones: None,
            depth: RollbackDepth::Random,
        }
    }
}

/// The senders of the node's snapshots.
type Snapshots = (
    watch::Sender<Arc<StateView>>,
    watch::Sender<Arc<PoolView>>,
    watch::Sender<Arc<heartwood_blockchain::ChainStatus>>,
);

/// A service over the fakes.
pub struct Harness {
    pub service: Arc<Blockchain<FakeNetwork, ScriptQueue>>,
    pub network: FakeNetwork,
    pub node: FakeNode,
    pub queue: ScriptQueue,
    pub chain: SharedChainState,
    pub clock: Arc<TokioClock>,
    pub status: watch::Receiver<MachineStatus>,
    /// Keeps the snapshots' channels open.
    _snapshots: Snapshots,
}

impl Harness {
    pub fn new(setup: Setup) -> Harness {
        let clock = Arc::new(TokioClock::new(setup.now_ms));
        let chain: SharedChainState =
            Arc::new(Mutex::new(ChainState::at(Arc::new(block(setup.tip)))));
        let (work, receiver) = mpsc::unbounded_channel();
        let options = Options::defaults(&|_| None, std::path::Path::new("pool"), 1);
        let pool = PoolView {
            by_priority: Vec::new(),
            online: Vec::new(),
            options: PoolViewOptions::of(&options),
        };
        let (state_tx, state_rx) = watch::channel(genesis_view());
        let (pool_tx, pool_rx) = watch::channel(Arc::new(pool));
        let (chain_tx, chain_rx) = watch::channel(Arc::default());
        let handle = NodeHandle::from_channels(work, state_rx, pool_rx, chain_rx);
        let node = FakeNode {
            script: Arc::default(),
            chain: Arc::clone(&chain),
            clock: Arc::clone(&clock),
        };
        node.clone().spawn(receiver);
        let network = FakeNetwork::default();
        network.state().peers = setup.peers;
        let milestones = setup
            .milestones
            .unwrap_or_else(|| c1().milestones().clone());
        let parts = BlockchainParts {
            node: handle,
            chain: Arc::clone(&chain),
            network: network.clone(),
            options: Arc::new(setup.options),
            milestones: Arc::new(milestones),
            clock: Arc::clone(&clock) as Arc<dyn Clock>,
            rollback: setup.depth,
            runtime: tokio::runtime::Handle::current(),
        };
        let slot = Arc::new(Mutex::new(None));
        let made = Arc::clone(&slot);
        let service = Blockchain::with_queue(parts, move |node, events, runtime| {
            let queue = ScriptQueue::new(node, events, runtime);
            *locked(&made) = Some(queue.clone());
            queue
        });
        let queue = locked(&slot).take().unwrap();
        let status = service.status();
        Harness {
            service,
            network,
            node,
            queue,
            chain,
            clock,
            status,
            _snapshots: (state_tx, pool_tx, chain_tx),
        }
    }

    /// The chain state.
    pub fn chain(&self) -> MutexGuard<'_, ChainState> {
        lock(&self.chain)
    }

    /// The machine's state's name.
    pub fn state(&self) -> &'static str {
        self.status.borrow().state.name()
    }

    /// Wait until the machine is in the state named `name`, at most `limit` of tokio's time.
    pub async fn until(&mut self, name: &str, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        while self.state() != name {
            let changed = tokio::time::timeout_at(deadline, self.status.changed()).await;
            assert!(
                changed.is_ok(),
                "still {} after {limit:?}, not {name}",
                self.state()
            );
        }
    }

    /// Let the background tasks run for `time` of tokio's time.
    pub async fn settle(&self, time: Duration) {
        tokio::time::sleep(time).await;
    }

    /// Wait until `done` holds, looking every 10 ms of tokio's time, at most `limit`; how long it
    /// took.
    pub async fn wait_for(
        &self,
        limit: Duration,
        mut done: impl FnMut(&Harness) -> bool,
    ) -> Duration {
        let start = tokio::time::Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < limit,
                "not done after {limit:?}: {}",
                self.state()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        start.elapsed()
    }

    /// The clock's time.
    pub fn clock_now(&self) -> i64 {
        self.clock.now_ms()
    }
}
