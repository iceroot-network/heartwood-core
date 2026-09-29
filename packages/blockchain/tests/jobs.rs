//! The job and the queue: the committed one-node chain imported in the jobs the reference cuts it
//! into, each committed in one SQLite transaction and published after its commit; each way a job
//! ends (dropped, not chained, a refused, discarded, forked or corrupted block, a block in a
//! future slot, a rebuild failure first or in the middle); and the queue's order, pause, resume,
//! clear, stop and drain against a scripted node loop.

mod common;
mod node_support;

use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use common::{Chain, assert_same_state, scratch};
use heartwood_blockchain::chain_state::lock as lock_chain;
use heartwood_blockchain::queue::{JobQueue, QueueEvents};
use heartwood_blockchain::{
    BlockProcessor, BlockProcessorResult, ChainStatus, IncomingBlock, Job, JobEnd, JobOutcome,
    NodeError, NodeHandle, RestoreMode, Work,
};
use heartwood_kernel::ChainLink;
use heartwood_p2p::DownloadedBlock;
use heartwood_pool::{DelegateVersions, Pool};
use heartwood_transactions::Handlers;
use node_support::{Script, Scripted, TestNode, at_epoch_seconds, job, lock, pool_options};
use tokio::sync::{Notify, mpsc, watch};

const ACCEPTED: BlockProcessorResult = BlockProcessorResult::Accepted;

fn heights(outcome: &JobOutcome, chain: &Chain) -> Vec<u32> {
    let height = |id| {
        let block = chain.blocks.iter().find(|block| block.id() == id);
        block.map_or(0, |block| block.height())
    };
    outcome.results.iter().map(|(id, _)| height(id)).collect()
}

fn broadcast(chain: &Chain, height: u32) -> JobEnd {
    JobEnd::Broadcast(Arc::new(chain.block(height).clone()))
}

#[tokio::test]
async fn the_reference_jobs_commit_once_each_and_publish_after_the_commit() {
    let chain = Chain::load();
    let dir = scratch("jobs-reference-cuts");
    let scripted = Scripted::with(&[]);
    let node = TestNode::start(&chain, &dir, scripted.clone(), pool_options(&dir, 2_700));
    // The reference's cuts of the whole chain downloaded from height 1: the milestone at height 1,
    // the last downloaded height, suppresses the cut at the milestone of height 2; no 100 blocks
    // hold 150 transactions.
    for (from, to) in [(2, 101), (102, 201), (202, 265)] {
        lock(&scripted.seen).clear();
        let outcome = node.run(&chain, from, to).await;
        assert_eq!(outcome.committed, to - from + 1);
        assert_eq!(heights(&outcome, &chain), (from..=to).collect::<Vec<_>>());
        assert!(
            outcome
                .results
                .iter()
                .all(|(_, result)| *result == ACCEPTED)
        );
        assert_eq!(outcome.end, broadcast(&chain, to));
        assert!(!outcome.clear_queue);
        // While the job ran, the store and the published state stayed at the job's start, and
        // the processor saw each accepted block pending.
        for (index, seen) in lock(&scripted.seen).iter().enumerate() {
            assert_eq!(seen.pending, index, "{seen:?}");
            assert_eq!(seen.stored, Some(from - 1), "{seen:?}");
            assert_eq!(seen.published, Some(from - 1), "{seen:?}");
        }
        assert_eq!(node.height(), to);
        let status: ChainStatus = (**node.handle.chain().borrow()).clone();
        assert_eq!(status.stored_height, to);
        assert_eq!(status.tip().map(|tip| tip.height()), Some(to));
        let expected = (to.saturating_sub(99).max(1)..=to).collect::<Vec<_>>();
        let kept: Vec<u32> = status
            .last_blocks
            .iter()
            .map(|block| block.height())
            .collect();
        assert_eq!(kept, expected, "the last 100 blocks");
        let downloaded = lock_chain(&node.chain).last_downloaded;
        assert_eq!(downloaded, Some(ChainLink::of(chain.block(to))));
    }
    let observed = lock(&node.observed.0).clone();
    assert_eq!(observed.first().map(String::as_str), Some("restarted 1"));
    assert_eq!(observed.len(), 265);
    assert!(observed.iter().any(|line| line == "applied 265 missed 0"));
    node.stop().await.unwrap();
    let (_, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_same_state(
        &restored.state,
        &node_support::replayed(&chain, 265),
        "the final tip",
    );
}

/// A node at `tip` on the chain, in a fresh directory.
async fn node_at(chain: &Chain, name: &str, tip: u32, script: &[(u32, Script)]) -> TestNode {
    let dir = scratch(name);
    let scripted = Scripted::with(script);
    let node = TestNode::start(chain, &dir, scripted.clone(), pool_options(&dir, 2_700));
    if tip > 1 {
        node.run(chain, 2, tip).await;
    }
    lock(&scripted.seen).clear();
    node
}

#[tokio::test]
async fn a_rebuild_failure_ends_the_loop_with_the_previous_result() {
    let chain = Chain::load();
    // Block 4 holds two transactions; a listed count of five fails the rebuild.
    let node = node_at(&chain, "jobs-rebuild-middle", 1, &[]).await;
    let mut broken = job(&chain, 2, 6);
    broken.blocks[2].block.header.header.number_of_transactions = Some(5);
    let outcome = node.handle.job(node.enqueued(broken)).await.unwrap();
    assert_eq!(heights(&outcome, &chain), [2, 3]);
    assert_eq!(outcome.committed, 2);
    assert_eq!(
        outcome.end,
        broadcast(&chain, 3),
        "the accepted blocks are stored and broadcast"
    );
    assert_eq!(node.processor.heights(), [2, 3]);
    assert_eq!(node.height(), 3);

    // As the first block: nothing is processed, and the job ends with a clear.
    let mut broken = job(&chain, 4, 5);
    broken.blocks[0].block.header.header.number_of_transactions = Some(5);
    let outcome = node.handle.job(broken).await.unwrap();
    assert!(outcome.results.is_empty());
    assert_eq!((outcome.committed, outcome.end), (0, JobEnd::Clear));
    assert_eq!(node.height(), 3);
    node.stop().await.unwrap();
}

#[tokio::test]
async fn a_job_at_or_below_the_tip_is_dropped_and_an_unchained_one_asks_for_a_fork_check() {
    let chain = Chain::load();
    let node = node_at(&chain, "jobs-dropped", 10, &[]).await;
    for dropped in [job(&chain, 10, 12), job(&chain, 5, 12), Job::default()] {
        let outcome = node.handle.job(dropped).await.unwrap();
        assert_eq!((outcome.committed, outcome.end), (0, JobEnd::Dropped));
    }
    let ahead = job(&chain, 12, 13);
    let outcome = node.handle.job(ahead.clone()).await.unwrap();
    assert_eq!(outcome.end, JobEnd::ForkCheck(ahead.blocks));
    let mut other = job(&chain, 11, 12);
    other.blocks[0].block.header.header.previous_block = Some("ab".repeat(32));
    let outcome = node.handle.job(other.clone()).await.unwrap();
    assert_eq!(outcome.end, JobEnd::ForkCheck(other.blocks));
    assert!(node.processor.heights().is_empty());
    assert_eq!(node.height(), 10);
    node.stop().await.unwrap();
}

#[tokio::test]
async fn a_block_in_a_future_slot_ends_the_loop() {
    let chain = Chain::load();
    let node = node_at(&chain, "jobs-future-slot", 1, &[]).await;
    // The current slot is block 3's: block 4 takes a later one.
    node.clock
        .set(at_epoch_seconds(&chain, chain.block(3).header().timestamp));
    let outcome = node.run(&chain, 2, 6).await;
    assert_eq!(heights(&outcome, &chain), [2, 3]);
    assert_eq!(outcome.end, broadcast(&chain, 3));
    // An honest block ahead of our clock names no peer.
    assert_eq!(outcome.faulty, None);
    node.stop().await.unwrap();
}

/// A block discarded for its future slot is judged without the clock: the peer that served one
/// no delegate forged is named, as for a block the processor refuses, so that it is not asked
/// again and again for a block that is never processed.
#[tokio::test]
async fn a_block_in_a_future_slot_that_no_delegate_forged_names_its_peer() {
    let chain = Chain::load();
    let node = node_at(&chain, "jobs-future-slot-fault", 1, &[]).await;
    node.clock
        .set(at_epoch_seconds(&chain, chain.block(3).header().timestamp));
    let mut junk = node.enqueued(job(&chain, 2, 6));
    let signature = junk.blocks[2].block.header.header.block_signature.as_mut();
    let signature = signature.expect("a signature");
    let flipped = if signature.starts_with('0') { "1" } else { "0" };
    signature.replace_range(..1, flipped);
    junk.blocks[2].block.ip = IpAddr::from([192, 0, 2, 9]);
    let outcome = node.handle.job(junk).await.unwrap();
    assert_eq!(heights(&outcome, &chain), [2, 3]);
    assert_eq!(outcome.end, broadcast(&chain, 3));
    assert_eq!(outcome.faulty, Some(IpAddr::from([192, 0, 2, 9])));
    node.stop().await.unwrap();
}

#[tokio::test]
async fn each_result_ends_the_job_as_the_reference_does() {
    let chain = Chain::load();
    let script = [
        (5, Script::Reject),
        (7, Script::Discard),
        (9, Script::Rollback),
    ];
    let node = node_at(&chain, "jobs-results", 1, &script).await;
    // A refused block: the accepted blocks before it are stored, and the queue is cleared.
    let outcome = node.run(&chain, 2, 6).await;
    assert_eq!(heights(&outcome, &chain), [2, 3, 4, 5]);
    let rejected = outcome.results.last().map(|(_, result)| *result);
    assert_eq!(rejected, Some(BlockProcessorResult::Rejected));
    assert_eq!((outcome.committed, outcome.end), (3, JobEnd::Clear));
    assert_eq!(node.height(), 4);
    // A discarded block may be broadcast; its handler asked for the queue to be cleared.
    lock(&node.processor.script).remove(&5);
    let outcome = node.run(&chain, 5, 8).await;
    assert_eq!(heights(&outcome, &chain), [5, 6, 7]);
    assert_eq!(outcome.committed, 2);
    assert_eq!(outcome.end, broadcast(&chain, 7));
    assert!(outcome.clear_queue);
    // A double-forged block: it is the forked block and the last downloaded block.
    lock(&node.processor.script).remove(&7);
    let outcome = node.run(&chain, 7, 10).await;
    assert_eq!(heights(&outcome, &chain), [7, 8, 9]);
    let forked = Arc::new(chain.block(9).clone());
    assert_eq!(outcome.end, JobEnd::Fork(forked));
    let downloaded = lock_chain(&node.chain).last_downloaded;
    assert_eq!(downloaded, Some(ChainLink::of(chain.block(9))));
    assert_eq!(node.height(), 8);
    node.stop().await.unwrap();
}

#[tokio::test]
async fn a_corrupted_state_stops_the_node_before_the_commit() {
    let chain = Chain::load();
    let node = node_at(&chain, "jobs-corrupted", 4, &[(7, Script::Corrupt)]).await;
    let answer = node.handle.job(job(&chain, 5, 8)).await;
    assert!(answer.is_err(), "the job gets no answer");
    let dir = node.dir.clone();
    let stopped = node.node.unwrap().join();
    assert_eq!(stopped, Err(NodeError::Corrupted { height: 7 }));
    let (_, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(
        restored.state.height(),
        4,
        "the job's blocks were not committed"
    );
}

/// A block of a job: only its height matters to the queue.
fn stub(height: u32) -> Job {
    let mut block = DownloadedBlock {
        header: Default::default(),
        transactions: Some(Vec::new()),
        ip: IpAddr::from([192, 0, 2, 9]),
    };
    block.header.header.height = Some(height);
    let blocks = vec![IncomingBlock::downloaded(block)];
    Job { blocks }
}

/// What the queue's listener and the scripted node loop saw, in order.
#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Log {
    fn push(&self, line: String) {
        lock(&self.0).push(line);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.0))
    }
}

impl QueueEvents for Log {
    fn finished(&self, outcome: JobOutcome) {
        self.push(format!("finished {}", outcome.committed));
    }

    fn drained(&self) {
        self.push("drained".to_owned());
    }
}

/// A node loop that logs each job (answering its first height as the committed count) and each
/// drain; a job at a height in `held` waits for `release` first; `clear` clears the queue from
/// inside the job at that height, as the reference's unchained handler does; a job at `stop`
/// gets no answer, as when the node stops.
struct Loop {
    log: Arc<Log>,
    release: Notify,
    held: Vec<u32>,
    clear: Option<u32>,
    stop: Option<u32>,
    queue: OnceLock<Arc<JobQueue>>,
}

async fn queue_with(name: &str, held: &[u32], clear: Option<u32>, stop: Option<u32>) -> Arc<Loop> {
    let chain = Chain::load();
    let log = Arc::new(Log::default());
    let fake = Arc::new(Loop {
        log: Arc::clone(&log),
        release: Notify::new(),
        held: held.to_vec(),
        clear,
        stop,
        queue: OnceLock::new(),
    });
    let (work, mut received) = mpsc::unbounded_channel();
    let state =
        heartwood_state::State::from_genesis(&chain.genesis(), chain.milestones(), &Handlers);
    let (_, state) = watch::channel(Arc::new(state.unwrap().view()));
    let pool = Pool::open(pool_options(&scratch(name), 1), None).unwrap();
    let (_, pool) = watch::channel(Arc::new(pool.view(&DelegateVersions::new())));
    let (_, status) = watch::channel(Arc::new(ChainStatus::default()));
    let handle = NodeHandle::from_channels(work, state, pool, status);
    let queue = JobQueue::new(handle, log, tokio::runtime::Handle::current());
    let _ = fake.queue.set(Arc::new(queue));
    let script = Arc::clone(&fake);
    tokio::spawn(async move {
        while let Some(work) = received.recv().await {
            match work {
                Work::Job(job, answer) => {
                    let height = job.blocks[0].height();
                    script.log.push(format!("job {height}"));
                    if script.held.contains(&height) {
                        script.release.notified().await;
                    }
                    if script.clear == Some(height) {
                        script.queue.get().unwrap().clear();
                    }
                    if script.stop == Some(height) {
                        continue;
                    }
                    let (results, end) = (Vec::new(), JobEnd::Dropped);
                    let outcome = JobOutcome {
                        results,
                        committed: height,
                        clear_queue: false,
                        end,
                        faulty: None,
                    };
                    let _ = answer.send(outcome);
                }
                Work::Drain(answer, _) => {
                    script.log.push("drain".to_owned());
                    let _ = answer.send(Ok(()));
                }
                other => panic!("the queue sent {other:?}"),
            }
        }
    });
    fake
}

impl Loop {
    fn queue(&self) -> &JobQueue {
        self.queue.get().unwrap()
    }

    /// Wait until the log has `count` lines, for at most ten seconds.
    async fn logged(&self, count: usize) -> Vec<String> {
        for _ in 0..5_000 {
            if lock(&self.log.0).len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        self.log.take()
    }
}

#[tokio::test]
async fn the_queue_runs_jobs_in_order_then_drains_once() {
    let fake = queue_with("jobs-queue-order", &[], None, None).await;
    let queue = fake.queue();
    for height in [2, 3, 4] {
        queue.push(stub(height));
    }
    let expected = [
        "job 2",
        "finished 2",
        "job 3",
        "finished 3",
        "job 4",
        "finished 4",
        "drain",
        "drained",
    ];
    assert_eq!(fake.logged(8).await, expected);
    // A resume of an idle queue without jobs drains at once.
    queue.resume();
    assert_eq!(fake.logged(2).await, ["drain", "drained"]);
    assert!(!queue.is_running() && !queue.is_paused() && queue.size() == 0);
}

#[tokio::test]
async fn a_paused_queue_holds_its_jobs_without_a_drain_until_resumed() {
    let fake = queue_with("jobs-queue-pause", &[2], None, None).await;
    let queue = fake.queue();
    queue.push(stub(2));
    assert_eq!(fake.logged(1).await, ["job 2"]);
    assert!(queue.is_running());
    queue.pause();
    queue.push(stub(3));
    assert_eq!((queue.size(), queue.is_paused()), (1, true));
    fake.release.notify_one();
    assert_eq!(fake.logged(1).await, ["finished 2"]);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(fake.log.take().is_empty(), "no drain while a job waits");
    assert!(!queue.is_running());
    queue.resume();
    assert_eq!(
        fake.logged(4).await,
        ["job 3", "finished 3", "drain", "drained"]
    );
}

#[tokio::test]
async fn a_clear_inside_a_job_drops_the_later_jobs_and_drains_after_it() {
    let fake = queue_with("jobs-queue-clear", &[], Some(3), None).await;
    let queue = fake.queue();
    queue.pause();
    for height in [2, 3, 4, 5] {
        queue.push(stub(height));
    }
    queue.resume();
    let expected = [
        "job 2",
        "finished 2",
        "job 3",
        "finished 3",
        "drain",
        "drained",
    ];
    assert_eq!(fake.logged(6).await, expected);
    assert_eq!(queue.size(), 0);
}

#[tokio::test]
async fn stop_waits_for_the_running_job_and_drops_the_others() {
    let fake = queue_with("jobs-queue-stop", &[2], None, None).await;
    let queue = fake.queue();
    queue.push(stub(2));
    queue.push(stub(3));
    assert_eq!(fake.logged(1).await, ["job 2"]);
    let release = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        fake.release.notify_one();
    };
    let ((), ()) = tokio::join!(queue.stop(), release);
    assert!(!queue.is_running());
    assert_eq!(queue.size(), 0);
    assert_eq!(fake.logged(3).await, ["finished 2", "drain", "drained"]);
    assert!(queue.is_paused());
}

#[tokio::test]
async fn a_node_that_stops_ends_the_processing_without_a_drain() {
    let fake = queue_with("jobs-queue-node-stops", &[], None, Some(3)).await;
    let queue = fake.queue();
    for height in [2, 3, 4] {
        queue.push(stub(height));
    }
    assert_eq!(fake.logged(3).await, ["job 2", "finished 2", "job 3"]);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(fake.log.take().is_empty(), "no job after it, and no drain");
    assert!(!queue.is_running());
}

/// The committed chain through the block processor in the reference's jobs: every block is
/// accepted, each job commits once, and the final state is the chain's.
#[tokio::test]
#[ignore = "integration"]
async fn the_chain_through_the_block_processor_in_the_reference_jobs() {
    let chain = Chain::load();
    let dir = scratch("jobs-processor");
    let scripted = Scripted::through(Arc::new(BlockProcessor));
    let node = TestNode::start(&chain, &dir, scripted.clone(), pool_options(&dir, 2_700));
    for (from, to) in [(2, 101), (102, 201), (202, 265)] {
        lock(&scripted.seen).clear();
        let outcome = node.run(&chain, from, to).await;
        assert!(
            outcome
                .results
                .iter()
                .all(|(_, result)| *result == ACCEPTED)
        );
        assert_eq!(
            (outcome.committed, outcome.end),
            (to - from + 1, broadcast(&chain, to))
        );
        let stored = lock(&scripted.seen)
            .iter()
            .all(|seen| seen.stored == Some(from - 1));
        assert!(stored, "one commit per job");
    }
    node.stop().await.unwrap();
    let (_, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_same_state(
        &restored.state,
        &node_support::replayed(&chain, 265),
        "the final tip",
    );
}

/// A block processed with the name it got when it was enqueued: enqueued without one, as a block
/// whose delegate registered earlier in the same download is, its generator is refused and the job
/// ends there, through the block processor.
#[tokio::test]
#[ignore = "integration"]
async fn a_block_enqueued_without_its_delegate_name_is_refused() {
    let chain = Chain::load();
    let dir = scratch("jobs-processor-unnamed");
    let scripted = Scripted::through(Arc::new(BlockProcessor));
    let node = TestNode::start(&chain, &dir, scripted, pool_options(&dir, 2_700));
    let mut unnamed = node.enqueued(job(&chain, 2, 5));
    assert!(unnamed.blocks.iter().all(|block| block.name.is_some()));
    unnamed.blocks[2].name = None;
    let outcome = node.handle.job(unnamed).await.unwrap();
    let results: Vec<_> = outcome.results.iter().map(|(_, result)| *result).collect();
    assert_eq!(
        results,
        [ACCEPTED, ACCEPTED, BlockProcessorResult::Rejected]
    );
    assert_eq!(heights(&outcome, &chain), [2, 3, 4]);
    assert_eq!((outcome.committed, outcome.end), (2, JobEnd::Clear));
    // An honest peer serves such a block: it is no fault of the peer.
    assert_eq!(outcome.faulty, None);
    // Downloaded again, it is named and accepted.
    assert_eq!(node.run(&chain, 4, 5).await.committed, 2);
    node.stop().await.unwrap();
}

/// A rebuild failure in the middle of a job, through the block processor.
#[tokio::test]
#[ignore = "integration"]
async fn a_rebuild_failure_through_the_block_processor() {
    let chain = Chain::load();
    let dir = scratch("jobs-processor-rebuild");
    let scripted = Scripted::through(Arc::new(BlockProcessor));
    let node = TestNode::start(&chain, &dir, scripted, pool_options(&dir, 2_700));
    let mut broken = job(&chain, 2, 6);
    broken.blocks[2].block.header.header.number_of_transactions = Some(5);
    let outcome = node.handle.job(node.enqueued(broken)).await.unwrap();
    assert_eq!(heights(&outcome, &chain), [2, 3]);
    assert_eq!((outcome.committed, outcome.end), (2, broadcast(&chain, 3)));
    // No honest peer serves a block that cannot be rebuilt: the peer is named.
    assert_eq!(outcome.faulty, Some(IpAddr::from([192, 0, 2, 1])));
    node.stop().await.unwrap();
}

/// A block whose signature fails, through the block processor: the job names the peer that
/// served it, so that the network records the fault and asks another peer.
#[tokio::test]
#[ignore = "integration"]
async fn a_block_no_honest_peer_serves_names_its_peer() {
    let chain = Chain::load();
    let dir = scratch("jobs-processor-fault");
    let scripted = Scripted::through(Arc::new(BlockProcessor));
    let node = TestNode::start(&chain, &dir, scripted, pool_options(&dir, 2_700));
    let mut broken = node.enqueued(job(&chain, 2, 6));
    let signature = broken.blocks[2]
        .block
        .header
        .header
        .block_signature
        .as_mut();
    let signature = signature.expect("a signature");
    let flipped = if signature.starts_with('0') { "1" } else { "0" };
    signature.replace_range(..1, flipped);
    broken.blocks[2].block.ip = IpAddr::from([192, 0, 2, 9]);
    let outcome = node.handle.job(broken).await.unwrap();
    let results: Vec<_> = outcome.results.iter().map(|(_, result)| *result).collect();
    assert_eq!(
        results,
        [ACCEPTED, ACCEPTED, BlockProcessorResult::Rejected]
    );
    assert_eq!((outcome.committed, outcome.end), (2, JobEnd::Clear));
    assert_eq!(outcome.faulty, Some(IpAddr::from([192, 0, 2, 9])));
    // The chain goes on from the good copy of the block.
    let outcome = node.run(&chain, 4, 6).await;
    assert_eq!((outcome.committed, outcome.faulty), (3, None));
    node.stop().await.unwrap();
}
