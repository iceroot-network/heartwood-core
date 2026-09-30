//! The blockchain service on a corpus chain, as a node runs it: [`ServiceRig`].
//!
//! A [`heartwood_blockchain::Blockchain`] over a real node thread (the store, the state, the pool
//! and the block processor with a recorder around it), a manual clock, a job queue that holds its
//! jobs until the rig lets them run, and a network that records what it is asked to broadcast.
//! The rig posts a block as the node's own forger does ([`ServiceRig::post_forged`]) and reports
//! what the service did with it, in the shape of the reference's `chain.job` answer: the lateness
//! rules at the receipt and at the job's end, the pings, the broadcast, the fork block and the
//! rollback count, and the log lines of the receipt and the job. The service's own recovery
//! removes the block of a fork; [`ServiceRig::settle`] waits for it.
//!
//! The rig runs the service on a runtime of its own, on tokio's paused clock, so its timers (the
//! boot's wait, the idle loops) cost no time; every time the service reads is the manual clock.
//! The log lines come from a `tracing` subscriber that is global to the process, since the node
//! thread logs too; it keeps nothing while no rig runs.

use std::borrow::Cow;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use heartwood_blockchain::blockchain::Queue;
use heartwood_blockchain::events::{CommitObserver, NoListener};
use heartwood_blockchain::pool_task::TransactionBroadcast;
use heartwood_blockchain::queue::QueueEvents;
use heartwood_blockchain::{
    BlockchainOptions, BlockchainParts, ChainState, JobOutcome, JobQueue, Network, Node,
    NodeHandle, NodeParts, RollbackDepth, SharedChainState,
};
use heartwood_crypto::{Block, BlockId, Transaction, TransactionId};
use heartwood_kernel::{Clock, ManualClock};
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::errors::{DownloadError, SeedError};
use heartwood_p2p::monitor::{CleanseOptions, NetworkStatus};
use heartwood_state::State;
use heartwood_state::digest::{DigestLine, Step};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

use crate::levels::l2_processor::{
    JobBlock, JobInputs, JobParts, Recorder, RigParts, Seen, download, job_value,
};

/// Where the node's own forger posts from.
const FORGER: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// How long the rig waits for the node and the service, in real time.
const PATIENCE: Duration = Duration::from_secs(120);
/// The node's version.
const VERSION: &str = "4.3.1-heartwood.0.1.0";

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One rig runs at a time in a process: they share the subscriber's lines.
static EXCLUSIVE: Mutex<()> = Mutex::new(());

/// The log lines the process's subscriber keeps while a rig is capturing.
fn captured() -> &'static Mutex<Option<Vec<String>>> {
    static LINES: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();
    LINES.get_or_init(Mutex::default)
}

/// The messages of the events the reference logs too, while a rig captures.
struct Capture;

#[derive(Default)]
struct Fields {
    message: String,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        // The machine's own trace of its events is not a line the reference logs.
        if fields.message.starts_with("event '") {
            return;
        }
        if let Some(lines) = locked(captured()).as_mut() {
            lines.push(fields.message);
        }
    }
}

/// Install the subscriber, once per process.
fn install_capture() -> Result<(), String> {
    static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            let subscriber = tracing_subscriber::registry().with(Capture);
            tracing::subscriber::set_global_default(subscriber)
                .map_err(|e| format!("the process already logs elsewhere: {e}"))
        })
        .clone()
}

/// What the network was asked.
#[derive(Default)]
struct Trace {
    /// The heights of the broadcast blocks, in order.
    broadcasts: Vec<u32>,
    /// How many forced cleanses after a fork recovery ran.
    refreshes: usize,
}

/// A network that records the blocks it is asked to broadcast, has no peers and answers nothing.
#[derive(Clone, Default)]
struct Recording(Arc<Mutex<Trace>>);

impl Network for Recording {
    async fn boot(&self) -> Result<(), SeedError> {
        Ok(())
    }

    async fn update_network_status(&self, _initial: bool) {}

    async fn cleanse_peers(&self, _options: CleanseOptions) {}

    async fn discover_peers(&self, _ping_all: bool, _add_all: bool, _silent: bool) -> bool {
        false
    }

    fn network_height(&self) -> u32 {
        0
    }

    fn peer_count(&self) -> usize {
        0
    }

    async fn check_network_health(&self, _fast: bool) -> NetworkStatus {
        NetworkStatus {
            forked: false,
            blocks_to_rollback: None,
        }
    }

    async fn check_for_fork(&self) -> u32 {
        0
    }

    async fn download_block_at_height(
        &self,
        _ip: IpAddr,
        _height: u32,
    ) -> Result<Option<DownloadedBlock>, DownloadError> {
        Ok(None)
    }

    async fn download_blocks_from_height(
        &self,
        _from: u32,
        _max_parallel: usize,
        _silent: bool,
        _timeout: Option<Duration>,
        _check_throttle: bool,
    ) -> Vec<DownloadedBlock> {
        Vec::new()
    }

    async fn download_transactions(&self, _exclude: &[TransactionId]) -> Vec<Vec<u8>> {
        Vec::new()
    }

    async fn broadcast_block(&self, block: &Block) {
        locked(&self.0).broadcasts.push(block.height());
    }

    fn broadcast_transactions(&self, _transactions: &[Transaction]) {}

    fn has_minimum_peers(&self, _silent: bool) -> bool {
        false
    }

    async fn populate_seed_peers(&self) -> Result<(), SeedError> {
        Ok(())
    }

    fn ping_all(&self) {}

    fn synced(&self) {}

    async fn refresh_peers_after_fork(&self) {
        locked(&self.0).refreshes += 1;
    }

    fn disconnect_invalid_peers(&self) {}

    fn is_cold_start(&self) -> bool {
        false
    }

    fn complete_cold_start(&self) {}
}

/// The transactions the pool admits are not broadcast.
struct Silent;

impl TransactionBroadcast for Silent {
    fn broadcast_transactions(&self, _transactions: &[Transaction]) {}
}

/// The node's job queue with its jobs held: a resume that the service asks for runs nothing until
/// the rig releases the queue.
struct Held {
    inner: Arc<JobQueue>,
    held: Arc<AtomicBool>,
    pushed: Arc<AtomicUsize>,
}

impl Queue for Held {
    fn push(&self, job: heartwood_blockchain::Job) {
        self.pushed.fetch_add(1, Ordering::SeqCst);
        self.inner.push(job);
    }

    fn pause(&self) {
        self.inner.pause();
    }

    fn resume(&self) {
        if !self.held.load(Ordering::SeqCst) {
            self.inner.resume();
        }
    }

    fn clear(&self) {
        self.inner.clear();
    }

    fn stop(&self) -> impl Future<Output = ()> + Send {
        self.inner.stop()
    }

    fn size(&self) -> usize {
        self.inner.size()
    }

    fn is_running(&self) -> bool {
        self.inner.is_running()
    }
}

/// A job's end as the service saw it: the outcome, the chain state and the log lines right after
/// the service answered it, before its own reaction (a fork recovery) ran.
struct Finished {
    outcome: JobOutcome,
    chain: ChainState,
    lines: Vec<String>,
}

/// The queue's listener: the service, after the rig has noted each job's end.
struct Tap {
    service: Arc<dyn QueueEvents>,
    chain: SharedChainState,
    log: Arc<Mutex<Vec<Finished>>>,
}

impl QueueEvents for Tap {
    fn finished(&self, outcome: JobOutcome) {
        self.service.finished(outcome.clone());
        // The service's answer ran on this call, and its reaction (a task) has not started: the
        // runtime has one thread, and this call is on it.
        let chain = heartwood_blockchain::chain_state::lock(&self.chain).clone();
        let lines = locked(captured()).clone().unwrap_or_default();
        locked(&self.log).push(Finished {
            outcome,
            chain,
            lines,
        });
    }

    fn drained(&self) {
        self.service.drained();
    }
}

/// The node's digest writer: the line of each block the node reverts.
struct Reverts(Arc<Recorder<'static>>);

impl CommitObserver for Reverts {
    fn applied(&mut self, _state: &State, _block: &Block) {}

    fn reverted(&mut self, state: &State) {
        self.0.reverted(state);
    }

    fn restarted(&mut self, _state: &State) {}
}

/// What the service did with a block posted by the node's forger.
#[derive(Debug, Clone)]
pub struct Posted {
    /// The answer in the shape of the reference's `chain.job` answer: `{received: false, lines}`
    /// for a block the service discarded at the receipt, else the job's answer with the log lines.
    pub value: Value,
    /// Each block the processor answered.
    pub seen: Vec<Seen>,
    /// The heights of the blocks the service broadcast.
    pub broadcasts: Vec<u32>,
    /// Whether the service pinged the block.
    pub pinged: bool,
}

/// The blockchain service on a corpus chain (see the module documentation).
pub struct ServiceRig {
    runtime: tokio::runtime::Runtime,
    service: Arc<heartwood_blockchain::Blockchain<Recording, Held>>,
    node: Option<Node>,
    handle: NodeHandle,
    shared: SharedChainState,
    recorder: Arc<Recorder<'static>>,
    network: Recording,
    clock: Arc<ManualClock>,
    held: Arc<AtomicBool>,
    pushed: Arc<AtomicUsize>,
    finished: Arc<Mutex<Vec<Finished>>>,
    queue: Arc<JobQueue>,
    milestones: Arc<heartwood_crypto::Milestones>,
    /// The store's and the pool's directory.
    _scratch: crate::store::ScratchDir,
    /// Held while the rig runs.
    _exclusive: MutexGuard<'static, ()>,
}

impl ServiceRig {
    /// The service over the node `parts` stand for, booted and idle at `now_ms`.
    pub fn start(parts: RigParts, now_ms: i64) -> Result<ServiceRig, String> {
        let exclusive = locked(&EXCLUSIVE);
        install_capture()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .map_err(|e| format!("no runtime: {e}"))?;
        let milestones = Arc::new(parts.milestones.clone());
        let milestones_shared = Arc::clone(&milestones);
        let clock = Arc::new(ManualClock::new(now_ms));
        let recorder = Arc::new(Recorder::new(
            Cow::Owned(parts.milestones),
            parts.chain_digest,
            &[],
        ));
        let shared: SharedChainState = Arc::new(Mutex::new(parts.chain));
        let dynamic: Arc<dyn Clock> = clock.clone();
        let (node, handle) = Node::spawn(NodeParts {
            state: parts.state,
            store: parts.store,
            pool: parts.pool,
            chain: Arc::clone(&shared),
            processor: recorder.clone(),
            handlers: Handlers,
            milestones: Arc::clone(&milestones),
            network: Arc::new(parts.network),
            options: Arc::default(),
            clock: Arc::clone(&dynamic),
            listener: Arc::new(NoListener),
            observer: Some(Box::new(Reverts(Arc::clone(&recorder)))),
            broadcast: Arc::new(Silent),
            own_keys: Arc::new(heartwood_p2p::NoAttestedKeys),
            version: VERSION.to_owned(),
            stall: None,
        })
        .map_err(|e| format!("the node does not start: {e}"))?;
        let network = Recording::default();
        let held = Arc::new(AtomicBool::new(false));
        let pushed = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(Mutex::new(Vec::new()));
        let inner: Arc<Mutex<Option<Arc<JobQueue>>>> = Arc::default();
        let queue = {
            let (held, pushed, finished, chain, slot) = (
                Arc::clone(&held),
                Arc::clone(&pushed),
                Arc::clone(&finished),
                Arc::clone(&shared),
                Arc::clone(&inner),
            );
            move |node: NodeHandle, events: Arc<dyn QueueEvents>, runtime| {
                let tap = Arc::new(Tap {
                    service: events,
                    chain,
                    log: finished,
                });
                let inner = Arc::new(JobQueue::new(node, tap, runtime));
                *locked(&slot) = Some(Arc::clone(&inner));
                Held {
                    inner,
                    held,
                    pushed,
                }
            }
        };
        let service = {
            let _entered = runtime.enter();
            heartwood_blockchain::Blockchain::with_queue(
                BlockchainParts {
                    node: handle.clone(),
                    chain: Arc::clone(&shared),
                    network: network.clone(),
                    options: Arc::new(BlockchainOptions::default()),
                    milestones,
                    clock: dynamic,
                    rollback: RollbackDepth::Random,
                    runtime: runtime.handle().clone(),
                },
                queue,
            )
        };
        let queue = locked(&inner).clone().ok_or("the service built no queue")?;
        let mut rig = ServiceRig {
            runtime,
            service,
            node: Some(node),
            handle,
            shared,
            recorder,
            network,
            clock,
            held,
            pushed,
            finished,
            queue,
            milestones: Arc::clone(&milestones_shared),
            _scratch: parts.scratch,
            _exclusive: exclusive,
        };
        rig.boot()?;
        Ok(rig)
    }

    fn boot(&mut self) -> Result<(), String> {
        let service = Arc::clone(&self.service);
        let booting = self.runtime.spawn(async move { service.boot().await });
        self.wait("the boot", |_| booting.is_finished())?;
        match self.runtime.block_on(booting) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("the service does not boot: {e}")),
            Err(e) => return Err(format!("the service's boot ended badly: {e}")),
        }
        self.wait("the machine at idle", |rig| rig.state() == "idle")
    }

    /// The machine's state, by its name.
    pub fn state(&self) -> &'static str {
        self.service.status().borrow().state.name()
    }

    /// The tip the node published: its height and id.
    pub fn tip(&self) -> (u32, BlockId) {
        let chain = heartwood_blockchain::chain_state::lock(&self.shared);
        chain
            .last_block()
            .map_or((0, BlockId::from_array([0; 32])), |b| (b.height(), *b.id()))
    }

    /// Run the runtime until `ready`, for at most [`PATIENCE`] of real time.
    fn wait(&self, what: &str, ready: impl Fn(&ServiceRig) -> bool) -> Result<(), String> {
        let start = Instant::now();
        loop {
            if ready(self) {
                return Ok(());
            }
            if start.elapsed() > PATIENCE {
                return Err(format!("timed out waiting for {what}"));
            }
            // The service's timers run on the runtime's paused clock, which moves a second at a
            // time here while the node thread works in real time.
            self.runtime.block_on(async {
                tokio::time::advance(Duration::from_secs(1)).await;
                tokio::task::yield_now().await;
            });
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Let the service's tasks that are ready run.
    fn turn(&self) {
        self.runtime.block_on(async {
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        });
    }

    /// Wait until the service has answered a fork it began (a recovery ended with a cleanse of
    /// the peers) or is idle again.
    pub fn settle(&self, refreshes: usize) -> Result<(), String> {
        self.wait("the recovery", |rig| {
            locked(&rig.network.0).refreshes >= refreshes && rig.state() == "idle"
        })?;
        self.turn();
        Ok(())
    }

    /// How many recoveries have cleansed the peers.
    pub fn refreshes(&self) -> usize {
        locked(&self.network.0).refreshes
    }

    /// The lines of the blocks the node reverted since the last call.
    pub fn take_reverts(&self) -> Vec<DigestLine> {
        self.recorder.take_reverts()
    }

    /// Post `block` as the node's forger does: received at `received_at`, and processed at `now`
    /// (both in milliseconds since the Unix epoch), its digest step `step`.
    pub fn post_forged(
        &mut self,
        block: &JobBlock,
        received_at: i64,
        now: i64,
        step: Step,
    ) -> Result<Posted, String> {
        let params = self.service_params(block)?;
        let mut incoming = download(std::slice::from_ref(block), &params)?;
        for incoming in &mut incoming {
            incoming.block.ip = FORGER;
        }
        let mut incoming = incoming.into_iter();
        let (Some(mut posted), None) = (incoming.next(), incoming.next()) else {
            return Err("the download did not give one block".into());
        };
        posted.from_forger = true;
        *locked(captured()) = Some(Vec::new());
        locked(&self.finished).clear();
        locked(&self.network.0).broadcasts.clear();
        self.recorder.take_seen();
        self.recorder.expect_steps(&[step]);
        let before = heartwood_blockchain::chain_state::lock(&self.shared)
            .ping
            .clone();
        let pushed = self.pushed.load(Ordering::SeqCst);
        // The queue holds the job until the time of its processing.
        self.held.store(true, Ordering::SeqCst);
        self.queue.pause();
        self.clock.set(received_at);
        let service = Arc::clone(&self.service);
        self.runtime
            .block_on(async move { service.handle_incoming_block(posted, true).await });
        let pinged = heartwood_blockchain::chain_state::lock(&self.shared).ping != before;
        if self.pushed.load(Ordering::SeqCst) == pushed {
            self.held.store(false, Ordering::SeqCst);
            let lines = locked(captured()).take().unwrap_or_default();
            return Ok(Posted {
                value: json!({"received": false, "lines": lines}),
                seen: Vec::new(),
                broadcasts: Vec::new(),
                pinged,
            });
        }
        self.clock.set(now);
        self.held.store(false, Ordering::SeqCst);
        self.queue.resume();
        self.wait("the job's end", |rig| !locked(&rig.finished).is_empty())?;
        self.turn();
        let seen = self.recorder.take_seen();
        let broadcasts = locked(&self.network.0).broadcasts.clone();
        let finished = locked(&self.finished).remove(0);
        let value = self.value(block, &finished, &seen, &broadcasts, now)?;
        Ok(Posted {
            value,
            seen,
            broadcasts,
            pinged,
        })
    }

    /// The job's answer in the reference's shape, from what the service did: the reference's job
    /// ends `fork` when it forks the block of a forger that was late, where Heartwood's job ends
    /// with the block to broadcast and the service forks it, so the end is the service's.
    fn value(
        &self,
        block: &JobBlock,
        finished: &Finished,
        seen: &[Seen],
        broadcasts: &[u32],
        now: i64,
    ) -> Result<Value, String> {
        let milestones = &self.milestones;
        let tip = finished
            .chain
            .last_block()
            .map(|b| (b.height(), *b.id()))
            .ok_or("the chain state has no tip")?;
        let parts = JobParts {
            blocks: std::slice::from_ref(block),
            inputs: &JobInputs::at(now),
            outcome: &finished.outcome,
            seen,
            chain: &finished.chain,
            tip,
            milestones,
        };
        let mut value = job_value(&parts);
        let height = block.link()?.height;
        value["broadcast"] = json!(broadcasts.contains(&height));
        if let Some(forked) = &finished.chain.forked_block {
            value["end"] = json!("fork");
            value["block"] = json!({"height": forked.height(), "id": forked.id().to_hex()});
        }
        value["lines"] = json!(finished.lines);
        Ok(value)
    }

    fn service_params(&self, block: &JobBlock) -> Result<heartwood_crypto::Params, String> {
        let height = block.link()?.height;
        Ok(self.milestones.at(height).clone())
    }

    /// End the service and the node.
    pub fn stop(mut self) {
        let service = Arc::clone(&self.service);
        let handle = self.handle.clone();
        self.runtime.block_on(async {
            service.dispose().await;
            handle.stop().await;
        });
        if let Some(node) = self.node.take() {
            let _ = node.join();
        }
        *locked(captured()) = None;
    }
}

impl Drop for ServiceRig {
    fn drop(&mut self) {
        *locked(captured()) = None;
    }
}

/// The lines of a `chain.job` answer's `lines`, as text.
pub fn lines_of(value: &Value) -> Vec<String> {
    (value["lines"].as_array())
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
