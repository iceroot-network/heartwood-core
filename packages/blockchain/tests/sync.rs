//! The service's sync and idle work over a fake network and a scripted node, on tokio's paused
//! time: the enqueueing's cuts (the milestone list-shift quirk and the empty trailing job),
//! incoming blocks, the queue's drain, the sync from genesis in chunks, an unchained download and
//! its fork check, the halted network, the idle loops and the wake-up timer; and, through the
//! node thread and the processor, a sync from genesis.

mod common;
mod service;

use std::time::Duration;

use heartwood_blockchain::IncomingBlock;
use heartwood_blockchain::blockchain::cut_jobs;
use heartwood_crypto::Milestones;
use heartwood_kernel::ChainLink;
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::monitor::NetworkStatus;
use service::{Harness, PEER, Served, Setup, block, c1, downloaded, heights, incoming, range};

const MINUTE: Duration = Duration::from_secs(60);

/// A block at `height` stating `transactions` (the cuts read nothing else of it).
fn stated(height: u32, transactions: Option<u32>) -> IncomingBlock {
    let mut block = DownloadedBlock {
        header: Default::default(),
        transactions: None,
        ip: PEER,
    };
    block.header.header.height = Some(height);
    block.header.header.number_of_transactions = transactions;
    IncomingBlock::downloaded(block)
}

fn blocks(from: u32, to: u32, transactions: u32) -> Vec<IncomingBlock> {
    (from..=to).map(|h| stated(h, Some(transactions))).collect()
}

/// The chain's milestones with entries added at `heights`.
fn milestones_at(heights: &[u32]) -> Milestones {
    let text = common::read("crypto/milestones.json");
    let mut entries: serde_json::Value = serde_json::from_str(&text).unwrap();
    for height in heights {
        let entry = serde_json::json!({ "height": height });
        entries.as_array_mut().unwrap().push(entry);
    }
    common::files_with(&entries.to_string()).milestones
}

fn cut(blocks: Vec<IncomingBlock>, from: u32, milestones: &Milestones) -> Vec<Vec<u32>> {
    cut_jobs(blocks, from, milestones)
        .iter()
        .map(heights)
        .collect()
}

fn span(from: u32, to: u32) -> Vec<u32> {
    (from..=to).collect()
}

/// The sync downloads (the idle poll's are silent).
fn sync_downloads(h: &Harness) -> Vec<String> {
    let calls = h.network.calls().into_iter();
    calls.filter(|c| c.contains("silent=false")).collect()
}

#[test]
fn jobs_are_cut_at_100_blocks_and_the_rest_is_a_last_job() {
    let none = milestones_at(&[]);
    assert_eq!(
        cut(blocks(3, 252, 0), 2, &none),
        [span(3, 102), span(103, 202), span(203, 252)]
    );
    // A cut at the last block leaves an empty last job, which is pushed too.
    assert_eq!(
        cut(blocks(3, 202, 0), 2, &none),
        [span(3, 102), span(103, 202), vec![]]
    );
    assert_eq!(cut(blocks(3, 3, 0), 2, &none), [vec![3]]);
}

#[test]
fn jobs_are_cut_after_the_block_that_brings_them_to_150_transactions() {
    let none = milestones_at(&[]);
    assert_eq!(
        cut(blocks(3, 9, 60), 2, &none),
        [span(3, 5), span(6, 8), vec![9]]
    );
    let mut mixed = blocks(3, 4, 70);
    mixed.extend([stated(5, Some(10)), stated(6, Some(0)), stated(7, Some(1))]);
    assert_eq!(cut(mixed, 2, &none), [span(3, 5), span(6, 7)]);
}

#[test]
fn a_block_without_a_stated_count_stops_the_transaction_cuts_until_the_next_cut() {
    let none = milestones_at(&[]);
    // The reference's sum becomes NaN, which is never 150 or more, until a cut by the block count
    // starts it again at 0.
    let mut blocks = vec![stated(3, None)];
    blocks.extend((4..=110).map(|h| stated(h, Some(150))));
    let mut expected = vec![span(3, 102)];
    expected.extend((103..=110).map(|h| vec![h]));
    expected.push(vec![]);
    assert_eq!(cut(blocks, 2, &none), expected);
}

#[test]
fn jobs_are_cut_at_each_milestone_height_above_the_last_downloaded_block() {
    let milestones = milestones_at(&[15, 20, 40]);
    assert_eq!(
        cut(blocks(13, 25, 0), 12, &milestones),
        [span(13, 15), span(16, 20), span(21, 25)]
    );
    // A milestone below the last downloaded height is not in the list.
    assert_eq!(
        cut(blocks(17, 25, 0), 16, &milestones),
        [span(17, 20), span(21, 25)]
    );
}

#[test]
fn a_milestone_at_the_last_downloaded_height_suppresses_every_later_milestone_cut() {
    let milestones = milestones_at(&[12, 15, 20]);
    // The list starts at 12, which no block of the call has, so it never shifts.
    assert_eq!(cut(blocks(13, 25, 0), 12, &milestones), [span(13, 25)]);
    // From 13 the list starts at 15.
    assert_eq!(
        cut(blocks(14, 25, 0), 13, &milestones),
        [span(14, 15), span(16, 20), span(21, 25)]
    );
}

#[tokio::test(start_paused = true)]
async fn enqueueing_pushes_the_cuts_from_the_last_downloaded_height_and_resumes_the_queue() {
    let h = Harness::new(Setup {
        tip: 12,
        milestones: Some(milestones_at(&[15])),
        ..Setup::default()
    });
    h.service.enqueue_blocks(incoming(range(13, 17)));
    h.settle(Duration::from_secs(1)).await;
    let pushed: Vec<_> = h.queue.pushed().iter().map(heights).collect();
    assert_eq!(pushed, [span(13, 15), span(16, 17)]);
    assert_eq!(h.node.log(), ["job [13, 14, 15]", "job [16, 17]", "drain"]);
    assert_eq!(h.chain().last_height(), 17);
    // Nothing is pushed for no blocks.
    h.service.enqueue_blocks(Vec::new());
    assert_eq!(h.queue.pushed().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn enqueueing_names_each_block_from_the_published_state_before_its_jobs_run() {
    let h = Harness::new(Setup {
        tip: 12,
        ..Setup::default()
    });
    let mut blocks = incoming(range(13, 16));
    // A key that is no wallet's, a block of another version, and a block named before (a job's
    // blocks enqueued again after a fork check keep their names).
    blocks[1].block.header.header.generator_public_key = Some(format!("02{}", "00".repeat(32)));
    blocks[2].block.header.header.version = Some(1);
    blocks[3].name = Some("kept".into());
    h.service.enqueue_blocks(blocks);
    let names: Vec<_> = (h.queue.pushed().iter())
        .flat_map(|job| job.blocks.iter().map(|block| block.name.clone()))
        .collect();
    let view = service::genesis_view();
    let key = block(13).header().generator_public_key;
    let delegate = view.wallets.find_by_public_key(&key).unwrap().delegate();
    let name = delegate.map(|delegate| delegate.username().to_owned());
    assert!(name.is_some());
    assert_eq!(names, [name, None, None, Some("kept".into())]);
}

#[tokio::test(start_paused = true)]
async fn an_incoming_block_in_idle_is_processed_broadcast_and_drained_back_to_idle() {
    let h = Harness::new(Setup {
        tip: 12,
        peers: 0,
        now_ms: service::time_of(13) + 1_000,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    assert_eq!(h.state(), "idle");
    let log = h.node.log().len();
    let next = IncomingBlock::downloaded(downloaded(&block(13)));
    h.service.handle_incoming_block(next, true).await;
    assert_eq!(h.state(), "newBlock");
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    // The job, then the queue's drain: the pool's re-add and the online record, then
    // PROCESSFINISHED.
    assert_eq!(h.node.log()[log..], ["job [13]", "drain"]);
    assert_eq!(h.network.state().broadcasts, [13], "accepted in newBlock");
    let ping = h.chain().ping.clone().unwrap();
    assert_eq!(
        (ping.ping.height, ping.count, ping.ping.from_forger),
        (13, 1, false)
    );
}

#[tokio::test(start_paused = true)]
async fn an_incoming_block_is_discarded_in_a_future_slot_and_disregarded_before_the_start() {
    let now = service::time_of(13);
    let h = Harness::new(Setup {
        tip: 12,
        now_ms: now - 8_000,
        ..Setup::default()
    });
    let next = IncomingBlock::downloaded(downloaded(&block(13)));
    h.service.handle_incoming_block(next.clone(), true).await;
    assert!(
        h.chain().ping.is_none(),
        "a block in a future slot is not even pinged"
    );
    h.settle(Duration::from_secs(8)).await;
    h.service.handle_incoming_block(next, true).await;
    let ping = h.chain().ping.clone().unwrap();
    assert_eq!((ping.ping.height, ping.count), (13, 1));
    assert_eq!(h.state(), "uninitialised");
    assert!(h.queue.pushed().is_empty(), "disregarded before the start");
}

#[tokio::test(start_paused = true)]
async fn the_sync_downloads_the_chain_in_400_block_chunks_from_one_peer() {
    let h = sync_from_genesis(400, 1).await;
    assert_eq!(sync_downloads(&h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_sync_downloads_the_chain_in_40_block_chunks_from_three_peers() {
    let h = sync_from_genesis(40, 3).await;
    let downloads = sync_downloads(&h);
    let froms: Vec<_> = downloads
        .iter()
        .map(|c| c.split(' ').nth(1).unwrap())
        .collect();
    assert_eq!(froms, ["1", "121", "241"]);
}

async fn sync_from_genesis(chunk: u32, peers: u32) -> Harness {
    let tip = c1().tip();
    let h = Harness::new(Setup {
        now_ms: service::time_of(tip) + 2_000,
        ..Setup::default()
    });
    let blocks = range(2, tip).into_iter().map(|b| (b.height(), b)).collect();
    h.network.state().served = Some(Served {
        blocks,
        chunk,
        peers,
    });
    h.service.boot().await.unwrap();
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    assert_eq!(h.chain().last_height(), tip);
    let first = &sync_downloads(&h)[0];
    assert_eq!(
        first,
        "download 1 parallel=10 silent=false timeout=configured throttle=false"
    );
    // Every block was committed once, in the reference's chunks of at most 100 blocks.
    let jobs: Vec<_> = h
        .node
        .log()
        .into_iter()
        .filter(|l| l.starts_with("job"))
        .collect();
    let committed: usize = jobs.iter().map(|job| job.matches(',').count() + 1).sum();
    assert!(committed >= usize::try_from(tip - 1).unwrap(), "{jobs:?}");
    assert_eq!(h.chain().last_downloaded.map(|l| l.height), Some(tip));
    assert_eq!(h.network.count("synced"), 1);
    assert_eq!(
        h.node
            .log()
            .iter()
            .filter(|l| l.starts_with("expire"))
            .count(),
        1
    );
    assert!(
        h.network.state().broadcasts.is_empty(),
        "no broadcast outside newBlock"
    );
    h
}

#[tokio::test(start_paused = true)]
async fn an_unchained_download_clears_the_queue_and_checks_for_a_fork_in_the_background() {
    let h = Harness::new(Setup {
        tip: 16,
        now_ms: service::time_of(265),
        ..Setup::default()
    });
    {
        let mut network = h.network.state();
        // Blocks 15 to 17 do not chain to 16; the fork check rolls back 3 blocks, and the peer
        // gives the block below the first one.
        network.downloads.push_back(range(15, 17));
        network.forks.push_back(3);
        network.at_height = Some(downloaded(&block(14)));
    }
    h.service.boot().await.unwrap();
    h.wait_for(MINUTE, |h| h.chain().last_height() == 17).await;
    assert_eq!(
        sync_downloads(&h)[0],
        "download 16 parallel=10 silent=false timeout=configured throttle=false"
    );
    assert_eq!(h.network.count("check_for_fork"), 1);
    assert_eq!(h.network.count("download_block_at_height 192.0.2.7 14"), 1);
    let log = h.node.log();
    let removal = log.iter().position(|l| l == "remove 3").unwrap();
    assert!(
        log[removal..].contains(&"job [14, 15, 16, 17]".to_owned()),
        "{log:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_unchained_download_without_a_fork_counts_as_an_empty_download() {
    let h = Harness::new(Setup {
        tip: 16,
        now_ms: service::time_of(265),
        ..Setup::default()
    });
    h.network.state().downloads.push_back(range(15, 17));
    h.service.boot().await.unwrap();
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    // The unchained download and three empty ones, then the network is deemed halted.
    assert_eq!(sync_downloads(&h).len(), 4);
    assert_eq!(h.network.count("check_for_fork"), 1);
    assert!(
        h.node
            .log()
            .iter()
            .all(|l| !l.starts_with("job") && !l.starts_with("remove"))
    );
    assert_eq!(h.chain().last_height(), 16);
}

#[tokio::test(start_paused = true)]
async fn the_fork_check_is_skipped_within_one_block_time_of_the_last() {
    let h = Harness::new(Setup {
        tip: 16,
        ..Setup::default()
    });
    let blocks = incoming(range(18, 19));
    assert!(
        !h.service.check_for_fork(&blocks).await,
        "no rollback asked"
    );
    assert!(!h.service.check_for_fork(&blocks).await);
    assert_eq!(
        h.network.count("check_for_fork"),
        1,
        "the second came too soon"
    );
    h.settle(Duration::from_millis(8_001)).await;
    assert!(!h.service.check_for_fork(&blocks).await);
    assert_eq!(h.network.count("check_for_fork"), 2);
}

#[tokio::test(start_paused = true)]
async fn after_more_than_three_empty_downloads_the_network_is_halted_and_every_third_time_checked()
{
    let h = Harness::new(Setup {
        now_ms: service::time_of(265),
        ..Setup::default()
    });
    let calm = NetworkStatus {
        forked: false,
        blocks_to_rollback: None,
    };
    h.network.state().health.push_back(calm);
    h.service.boot().await.unwrap();
    // Four empty downloads, then NETWORKHALTED to the end of the sync, each time the wake-up
    // timer sends the machine back; the third time also checks the network's health.
    for (syncs, updates) in [(1, 1), (2, 2), (3, 0)] {
        h.wait_for(Duration::from_secs(61), |h| {
            h.network.count("synced") == syncs
        })
        .await;
        assert_eq!(sync_downloads(&h).len(), 4 * syncs);
        assert_eq!(h.chain().p2p_update_counter, updates);
        assert_eq!(h.chain().no_block_counter, 0);
    }
    assert_eq!(h.network.count("health false"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_halted_network_found_forked_starts_a_fork_recovery() {
    let h = Harness::new(Setup {
        tip: 30,
        now_ms: service::time_of(265),
        ..Setup::default()
    });
    let forked = NetworkStatus {
        forked: true,
        blocks_to_rollback: Some(6),
    };
    h.chain().p2p_update_counter = 2;
    h.network.state().health.push_back(forked);
    h.service.boot().await.unwrap();
    h.wait_for(MINUTE, |h| h.network.count("refresh_peers_after_fork") == 1)
        .await;
    assert!(h.node.log().contains(&"remove 6".to_owned()));
    assert_eq!(h.chain().last_height(), 24);
    assert_eq!(h.chain().blocks_to_rollback, 0);
}

#[tokio::test(start_paused = true)]
async fn the_idle_loops_poll_blocks_transactions_seeds_and_peers() {
    let h = Harness::new(Setup {
        tip: 12,
        now_ms: service::time_of(12) + 1_000,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    h.network.state().transactions = vec![vec![0xff]];
    let start = h.network.calls().len();
    h.settle(Duration::from_secs(30)).await;
    let calls = &h.network.calls()[start..];
    let count = |prefix: &str| calls.iter().filter(|c| c.starts_with(prefix)).count();
    let polls = count("download 12 parallel=10 silent=true timeout=2000 throttle=true");
    assert!((59..=61).contains(&polls), "{polls} block polls in 30 s");
    let pulls = count("download_transactions 0");
    assert!(
        (7..=8).contains(&pulls),
        "{pulls} transaction pulls in 30 s"
    );
    let admitted = h
        .node
        .log()
        .iter()
        .filter(|l| *l == "pool Download 1")
        .count();
    assert_eq!(admitted, pulls);
    // Below the minimum reach, the seeds each second; the discovery every 30 s.
    assert!((29..=31).contains(&count("populate_seed_peers")));
    assert_eq!(count("discover false true true"), 1);
    h.network.state().minimum = true;
    let seeded = h.network.count("populate_seed_peers");
    h.settle(Duration::from_secs(5)).await;
    assert_eq!(h.network.count("populate_seed_peers"), seeded);
}

#[tokio::test(start_paused = true)]
async fn the_block_poll_handles_one_new_block_and_enqueues_several() {
    let h = Harness::new(Setup {
        tip: 12,
        now_ms: service::time_of(13) + 1_000,
        peers: 1,
        ..Setup::default()
    });
    h.network.state().minimum = true;
    let tip = downloaded(&block(12));
    // The start's sync downloads once from the tip and finds nothing. Then the poll: the tip
    // itself is left out; one block above it named by a ranked delegate is an incoming block
    // (NEWBLOCK, then its job).
    let downloads = [vec![], vec![tip, downloaded(&block(13))]];
    h.network.state().downloads.extend(downloads);
    h.service.boot().await.unwrap();
    h.wait_for(MINUTE, |h| h.chain().last_height() == 13).await;
    let jobs: Vec<_> = h.queue.pushed().iter().map(heights).collect();
    assert_eq!(jobs, [vec![13]]);
    // Several blocks: NEWBLOCK, and enqueued as they are.
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    h.network.state().downloads.push_back(range(14, 16));
    h.wait_for(MINUTE, |h| h.chain().last_height() == 16).await;
    let jobs: Vec<_> = h.queue.pushed().iter().map(heights).collect();
    assert_eq!(jobs, [vec![13], vec![14, 15, 16]]);
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
}

#[tokio::test(start_paused = true)]
async fn the_wake_up_timer_syncs_60_s_after_the_idle_entry_or_the_last_accepted_block() {
    let h = Harness::new(Setup {
        tip: 12,
        peers: 0,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    let idle = tokio::time::Instant::now() - Duration::from_secs(1);
    // A block accepted 30 s after the idle entry restarts the timer.
    tokio::time::sleep_until(idle + Duration::from_secs(30)).await;
    h.chain().last_accepted_ms = Some(h.clock_now());
    // The waits look every 10 ms.
    let around =
        |seconds| Duration::from_secs(seconds)..=Duration::from_millis(seconds * 1000 + 10);
    h.wait_for(Duration::from_secs(120), |h| h.network.count("synced") == 2)
        .await;
    assert!(around(90).contains(&idle.elapsed()), "{:?}", idle.elapsed());
    // Back in idle, the timer is set again.
    h.wait_for(MINUTE + MINUTE, |h| h.network.count("synced") == 3)
        .await;
    assert!(
        around(150).contains(&idle.elapsed()),
        "{:?}",
        idle.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn is_synced_is_three_block_times_and_true_without_peers() {
    let h = Harness::new(Setup {
        tip: 12,
        now_ms: service::time_of(12) + 23_999,
        ..Setup::default()
    });
    assert!(h.service.is_synced(None));
    h.settle(Duration::from_millis(1)).await;
    assert!(!h.service.is_synced(None));
    let old = ChainLink::of(&block(2));
    assert!(!h.service.is_synced(Some(&old)));
    h.network.state().peers = 0;
    assert!(h.service.is_synced(Some(&old)));
}

/// A sync from genesis through the node thread, the job queue and the block processor.
mod through_the_node {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use heartwood_blockchain::events::NoListener;
    use heartwood_blockchain::pool_task::TransactionBroadcast;
    use heartwood_blockchain::{
        BlockProcessor, Blockchain, BlockchainParts, ChainState, Node, NodeParts, PoolTask,
        RestoreMode, RollbackDepth,
    };
    use heartwood_crypto::Transaction;
    use heartwood_kernel::{Clock, ManualClock};
    use heartwood_pool::{Options, Pool};
    use heartwood_state::State;
    use heartwood_transactions::Handlers;

    use crate::common;
    use crate::service::{FakeNetwork, Served, c1, range, time_of};

    struct Silent;

    impl TransactionBroadcast for Silent {
        fn broadcast_transactions(&self, _: &[Transaction]) {}
    }

    /// Sync the committed chain from genesis, served in chunks of `chunk` blocks by `peers`
    /// peers, and check the store it leaves.
    async fn sync(name: &str, chunk: u32, peers: u32) {
        let chain = c1();
        let dir = common::scratch(name);
        let (store, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
        let milestones = Arc::new(chain.milestones().clone());
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(time_of(chain.tip()) + 2_000));
        let options = Options::defaults(&|_| None, &dir, 1);
        let pool = PoolTask::new(Pool::open(options, None).unwrap(), restored.online);
        let tip = Arc::new(restored.state.last_block().clone());
        let shared = Arc::new(Mutex::new(ChainState::at(tip)));
        let (node, handle) = Node::spawn(NodeParts {
            state: restored.state,
            store,
            pool,
            chain: Arc::clone(&shared),
            processor: Arc::new(BlockProcessor),
            handlers: Handlers,
            milestones: Arc::clone(&milestones),
            network: Arc::new(chain.files.network.clone()),
            options: Arc::default(),
            clock: Arc::clone(&clock),
            listener: Arc::new(NoListener),
            observer: None,
            broadcast: Arc::new(Silent),
            own_keys: Arc::new(heartwood_p2p::NoAttestedKeys),
            version: "4.3.1-heartwood.0.1.0".into(),
        })
        .unwrap();
        let network = FakeNetwork::default();
        let blocks = range(2, chain.tip())
            .into_iter()
            .map(|b| (b.height(), b))
            .collect();
        network.state().served = Some(Served {
            blocks,
            chunk,
            peers,
        });
        network.state().peers = usize::try_from(peers).unwrap();
        let service = Blockchain::new(BlockchainParts {
            node: handle.clone(),
            chain: shared,
            network: network.clone(),
            options: Arc::default(),
            milestones,
            clock,
            rollback: RollbackDepth::Random,
            runtime: tokio::runtime::Handle::current(),
        });
        tokio::time::timeout(Duration::from_secs(120), service.boot())
            .await
            .unwrap()
            .unwrap();
        let mut status = service.status();
        let idle = status.wait_for(|status| status.state.is_idle() && status.started);
        tokio::time::timeout(Duration::from_secs(120), idle)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(handle.state().borrow().height(), chain.tip());
        service.dispose().await;
        handle.stop().await;
        node.join().unwrap();
        // The store holds the chain as a continuous replay leaves it.
        let (_, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
        let mut direct =
            State::from_genesis(&chain.genesis(), chain.milestones(), &Handlers).unwrap();
        for block in &chain.blocks {
            direct
                .apply_block(block, chain.milestones(), &Handlers)
                .unwrap();
        }
        common::assert_same_state(&restored.state, &direct, name);
    }

    #[tokio::test]
    #[ignore = "integration"]
    async fn the_chain_syncs_in_400_block_chunks_from_one_peer() {
        sync("sync-400", 400, 1).await;
    }

    #[tokio::test]
    #[ignore = "integration"]
    async fn the_chain_syncs_in_40_block_chunks_from_three_peers() {
        sync("sync-40", 40, 3).await;
    }
}
