//! The AwaitBlock record of the plugin chain on tokio's paused clock: each way a wait ends (no
//! wait, the drain, the bound, a socket that closes during the wait, with the handler running
//! after the drain or the bound all the same), no record for an internal route, and a job held
//! for 12 s: waits that end by the bound with the tip before the job, waits that see the drain
//! with the tip after it, and requests after the commit.

#![cfg(all(feature = "node", feature = "difftest"))]

mod server_support;

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use heartwood_crypto::{Block, BlockHeader, BlockId, PublicKeyBytes, TransactionId};
use heartwood_kernel::ChainLink;
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::node_services::{
    AwaitEnd, AwaitRecord, Collation, ForgerEvent, NodeServices, NodeUnavailable, QueueState,
};
use heartwood_p2p::server::ban_hammer::BanHammer;
use heartwood_p2p::server::plugins::{PluginParts, Plugins};
use heartwood_p2p::server::seams::{SessionId, SocketClosed};
use heartwood_p2p::testing::server::{FakeChain, FakeNode};
use heartwood_p2p::view::ChainView;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::BlockListing;
use heartwood_p2p_wire::internal::CurrentRound;
use heartwood_p2p_wire::status::PeerConfig;
use server_support::*;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

const BEFORE: u32 = 10;
const AFTER: u32 = 11;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// A node at [`BEFORE`] until it is told the job committed, then at [`AFTER`], with its own queue
/// and a record of every AwaitBlock step; everything else is the fake node's.
struct Recording {
    before: FakeNode,
    after: FakeNode,
    committed: AtomicBool,
    queue: watch::Sender<QueueState>,
    records: Mutex<Vec<AwaitRecord>>,
}

impl Recording {
    fn new() -> Arc<Recording> {
        let node = |height| {
            let (milestones, network) = devnet();
            FakeNode::new(FakeChain::at(height, milestones, network).unwrap())
        };
        Arc::new(Recording {
            before: node(BEFORE),
            after: node(AFTER),
            committed: AtomicBool::new(false),
            queue: watch::channel(QueueState::default()).0,
            records: Mutex::default(),
        })
    }

    /// The machine in `newBlock`, the queue running or not.
    fn queue(&self, running: bool) {
        let state = QueueState {
            new_block: true,
            running,
        };
        self.queue.send_replace(state);
    }

    /// The job commits: the tip moves, and the queue drains.
    fn commit(&self) {
        self.committed.store(true, Ordering::SeqCst);
        self.queue(false);
    }

    fn records(&self) -> Vec<AwaitRecord> {
        self.records.lock().unwrap().clone()
    }
}

impl NodeServices for Recording {
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
        match self.committed.load(Ordering::SeqCst) {
            true => self.after.chain(),
            false => self.before.chain(),
        }
    }
    fn last_downloaded_block(&self) -> ChainLink {
        self.before.last_downloaded_block()
    }
    fn ping_block(&self, header: &BlockHeader) -> bool {
        self.before.ping_block(header)
    }
    fn handle_incoming_block(&self, block: Block, from_forger: bool, ip: IpAddr) {
        self.before.handle_incoming_block(block, from_forger, ip);
    }
    fn delegate_rank(&self, username: &str) -> Option<u32> {
        self.before.delegate_rank(username)
    }
    fn already_forged(&self, height: u32, username: &str) -> Result<bool, LookupError> {
        self.before.already_forged(height, username)
    }
    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError> {
        self.before.active_keys_next_round()
    }
    fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<BlockListing>, LookupError> {
        self.before.blocks_for_download(from, limit, headers_only)
    }
    fn common_blocks(
        &self,
        ids: &[BlockId],
        asked: usize,
    ) -> Result<Vec<BlockHeader>, LookupError> {
        self.before.common_blocks(ids, asked)
    }
    fn pool_size(&self) -> usize {
        0
    }
    fn collate(
        &self,
        validate: bool,
        exclude: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>> {
        self.before.collate(validate, exclude)
    }
    fn post_transactions(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>> {
        self.before.post_transactions(transactions)
    }
    fn current_round(&self, now_ms: i64) -> Result<CurrentRound, LookupError> {
        self.before.current_round(now_ms)
    }
    fn slot_number(&self, timestamp: Option<i64>, now_ms: i64) -> i64 {
        self.before.slot_number(timestamp, now_ms)
    }
    fn force_wakeup(&self) {}
    fn forger_event(&self, event: ForgerEvent) {
        self.before.forger_event(event);
    }
    fn peer_config(&self) -> PeerConfig {
        self.before.peer_config()
    }
    fn await_block_step(&self, record: &AwaitRecord) {
        self.records.lock().unwrap().push(record.clone());
    }
}

/// The plugin chain over `node`.
fn plugins(node: &Arc<Recording>) -> Arc<Plugins> {
    let fakes = Fakes::new();
    Arc::new(Plugins::new(PluginParts {
        options: Arc::new(options()),
        node: node.clone(),
        peers: fakes.peers.clone(),
        handlers: fakes.handlers.clone(),
        ban_hammer: Arc::new(BanHammer::new(10, vec![FORGER])),
        clock: fakes.clock.clone(),
    }))
}

/// A getStatus request from `ip` that ends with the time at which its answer came, counted from
/// `start`, and the answer's success.
fn status(
    plugins: &Arc<Plugins>,
    ip: IpAddr,
    closed: SocketClosed,
    start: Instant,
) -> JoinHandle<(Duration, bool)> {
    let plugins = Arc::clone(plugins);
    tokio::spawn(async move {
        let answer = plugins
            .handle(
                Some(RouteId::GetStatus),
                status_request(VERSION),
                ip,
                SessionId(1),
                closed,
            )
            .await;
        (start.elapsed(), answer.result.is_ok())
    })
}

#[tokio::test(start_paused = true)]
async fn each_end_is_recorded_and_a_closed_socket_does_not_hasten_the_handler() {
    let node = Recording::new();
    let plugins = plugins(&node);
    let start = Instant::now();

    // No wait: the queue is idle.
    let idle = status(&plugins, peer(2), SocketClosed::never(), start);
    assert_eq!(idle.await.unwrap(), (ms(0), true));

    // Drained: the wait lasts until the drain.
    node.queue(true);
    let drained = status(&plugins, peer(3), SocketClosed::never(), start);
    sleep(ms(300)).await;
    node.queue(false);
    assert_eq!(drained.await.unwrap(), (ms(300), true));

    // Bound: the queue runs on.
    node.queue(true);
    let begun = start.elapsed();
    let bound = status(&plugins, peer(4), SocketClosed::never(), Instant::now());
    assert_eq!(bound.await.unwrap(), (ms(8_000), true));
    assert_eq!(start.elapsed() - begun, ms(8_000));

    // A socket closed during a wait that the drain ends: the handler runs at the drain.
    let (sender, closed) = SocketClosed::channel();
    let begun = Instant::now();
    let closing = status(&plugins, peer(5), closed, begun);
    sleep(ms(100)).await;
    sender.send_replace(true);
    sleep(ms(200)).await;
    node.queue(false);
    assert_eq!(closing.await.unwrap(), (ms(300), true));

    // A socket closed during a wait that the bound ends: the handler runs at the bound.
    node.queue(true);
    let (sender, closed) = SocketClosed::channel();
    let begun = Instant::now();
    let closing = status(&plugins, peer(6), closed, begun);
    sleep(ms(100)).await;
    drop(sender);
    assert_eq!(closing.await.unwrap(), (ms(8_000), true));

    let ends: Vec<_> = node
        .records()
        .iter()
        .map(|record| (record.caller, record.end, record.waited))
        .collect();
    assert_eq!(
        ends,
        [
            (peer(2), AwaitEnd::NoWait, ms(0)),
            (peer(3), AwaitEnd::Drained, ms(300)),
            (peer(4), AwaitEnd::Bound, ms(8_000)),
            (peer(5), AwaitEnd::SocketClosed, ms(300)),
            (peer(6), AwaitEnd::SocketClosed, ms(8_000)),
        ]
    );
    assert!(node.records().iter().all(|r| r.route == RouteId::GetStatus));
}

#[tokio::test(start_paused = true)]
async fn an_internal_route_makes_no_record_and_never_waits() {
    let node = Recording::new();
    node.queue(true);
    let plugins = plugins(&node);
    let start = Instant::now();
    for route in [RouteId::GetSlotNumber, RouteId::GetCurrentRound] {
        let answer = plugins.handle(
            Some(route),
            b"{}".to_vec(),
            FORGER,
            SessionId(1),
            SocketClosed::never(),
        );
        let answer = answer.await;
        assert!(answer.result.is_ok(), "{route:?}: {answer:?}");
    }
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert!(node.records().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_job_held_12_s_ends_early_waits_by_the_bound_with_the_tip_before_the_job() {
    let node = Recording::new();
    let plugins = plugins(&node);
    let start = Instant::now();
    node.queue(true);

    // Began at 0 s: the bound ends it at 8 s, before the job commits.
    let early = status(&plugins, peer(2), SocketClosed::never(), start);
    sleep(ms(5_000)).await;
    // Began at 5 s: the drain at 12 s ends it after 7 s, on the new tip.
    let late = status(&plugins, peer(3), SocketClosed::never(), start);
    sleep(ms(7_000)).await;
    node.commit();
    assert_eq!(early.await.unwrap(), (ms(8_000), true));
    assert_eq!(late.await.unwrap(), (ms(12_000), true));
    // Began after the commit: nothing to wait for, the new tip.
    let after = status(&plugins, peer(4), SocketClosed::never(), Instant::now());
    assert_eq!(after.await.unwrap().0, ms(0));

    let records = node.records();
    let shape = |record: &AwaitRecord| {
        (
            record.end,
            record.waited,
            record.tip_before,
            record.tip_after,
        )
    };
    let by_caller = |ip| records.iter().find(|record| record.caller == ip).unwrap();
    assert_eq!(
        shape(by_caller(peer(2))),
        (AwaitEnd::Bound, ms(8_000), BEFORE, BEFORE)
    );
    assert_eq!(
        shape(by_caller(peer(3))),
        (AwaitEnd::Drained, ms(7_000), BEFORE, AFTER)
    );
    assert_eq!(
        shape(by_caller(peer(4))),
        (AwaitEnd::NoWait, ms(0), AFTER, AFTER)
    );
}
