//! Fork recovery over a fake network and a scripted node, on tokio's paused time: `forkBlock`, the
//! recovery with a fixed, a set and a random depth, a job's rollback result, a job whose first
//! block does not chain, the removal's bounds, and a failed removal.

mod common;
mod service;

use std::sync::Arc;
use std::time::Duration;

use heartwood_blockchain::fork_recovery::{MAX_RANDOM_DEPTH, MIN_RANDOM_DEPTH};
use heartwood_blockchain::{BlockProcessorResult, IncomingBlock, RollbackDepth};
use service::{Harness, Setup, block, downloaded, incoming, range};

const MINUTE: Duration = Duration::from_secs(60);

/// A service idle at `tip`, with no peers (so every sync ends at once) and `depth`.
async fn idle_at(tip: u32, depth: RollbackDepth) -> Harness {
    let h = Harness::new(Setup {
        tip,
        peers: 0,
        now_ms: service::time_of(tip) + 1_000,
        depth,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    assert_eq!(h.state(), "idle");
    h
}

/// The removals the node was asked for.
fn removals(h: &Harness) -> Vec<String> {
    h.node
        .log()
        .into_iter()
        .filter(|l| l.starts_with("remove"))
        .collect()
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn a_forked_block_is_recovered_by_removing_the_fixed_depth() {
    let h = idle_at(20, RollbackDepth::Fixed(5)).await;
    h.chain().last_downloaded = Some(heartwood_kernel::ChainLink::of(&block(20)));
    h.service.fork_block(Arc::new(block(20)), None);
    // Marked, the queue cleared and stopped, and FORK at once.
    assert_eq!(
        h.chain().forked_block.as_ref().map(|b| b.height()),
        Some(20)
    );
    assert_eq!(h.state(), "fork");
    h.wait_for(MINUTE, |h| h.network.count("refresh_peers_after_fork") == 1)
        .await;
    assert_eq!(removals(&h), ["remove 5"]);
    let chain = h.chain().clone();
    assert_eq!(chain.last_height(), 15);
    assert_eq!(chain.last_downloaded.map(|l| l.height), Some(15));
    assert_eq!(chain.blocks_to_rollback, 0);
    // The forked block stays marked until a block at its height is accepted again.
    assert_eq!(chain.forked_block.map(|b| b.height()), Some(20));
    // SUCCESS: the machine syncs again, and is idle once synced.
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    assert_eq!(h.network.count("synced"), 2);
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn a_set_rollback_count_wins_over_the_depth() {
    let h = idle_at(20, RollbackDepth::Fixed(5)).await;
    h.service.fork_block(Arc::new(block(20)), Some(7));
    assert_eq!(h.chain().blocks_to_rollback, 7);
    h.wait_for(MINUTE, |h| h.network.count("refresh_peers_after_fork") == 1)
        .await;
    assert_eq!(removals(&h), ["remove 7"]);
    assert_eq!(h.chain().last_height(), 13);
    assert_eq!(h.chain().blocks_to_rollback, 0);
}

#[cfg(feature = "difftest")]
#[test]
fn a_seeded_depth_repeats_its_draws_within_the_random_range() {
    let draws = |seed| {
        let mut depth = RollbackDepth::Seeded(seed);
        (0..500).map(|_| depth.draw()).collect::<Vec<_>>()
    };
    let (first, again) = (draws(7), draws(7));
    assert_eq!(first, again);
    assert_ne!(first, draws(8));
    let range = MIN_RANDOM_DEPTH..=MAX_RANDOM_DEPTH;
    assert!(first.iter().all(|depth| range.contains(depth)));
    assert!(first.contains(&MIN_RANDOM_DEPTH) && first.contains(&MAX_RANDOM_DEPTH));
    assert_eq!(RollbackDepth::Fixed(54).draw(), 54);
}

#[tokio::test(start_paused = true)]
async fn without_a_set_count_the_recovery_removes_a_random_4_to_102_blocks() {
    let h = idle_at(200, RollbackDepth::Random).await;
    h.service.fork_block(Arc::new(block(200)), None);
    h.wait_for(MINUTE, |h| h.network.count("refresh_peers_after_fork") == 1)
        .await;
    let removed: u32 = removals(&h)[0]["remove ".len()..].parse().unwrap();
    assert!((MIN_RANDOM_DEPTH..=MAX_RANDOM_DEPTH).contains(&removed));
    assert_eq!(h.chain().last_height(), 200 - removed);
}

#[tokio::test(start_paused = true)]
async fn a_second_fork_during_a_recovery_is_ignored() {
    let h = idle_at(20, RollbackDepth::Random).await;
    h.service.fork_block(Arc::new(block(20)), Some(2));
    h.service.fork_block(Arc::new(block(20)), Some(3));
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    // FORK is not an event of the fork state; the second call only set the count, which the
    // running recovery then used and cleared.
    assert_eq!(removals(&h).len(), 1);
    assert_eq!(h.chain().blocks_to_rollback, 0);
}

#[tokio::test(start_paused = true)]
async fn a_job_s_rollback_result_forks_the_chain_at_its_block() {
    let h = idle_at(12, RollbackDepth::Random).await;
    h.node
        .script()
        .results
        .insert(13, BlockProcessorResult::Rollback);
    h.chain().blocks_to_rollback = 1;
    // Block 13's slot comes.
    h.settle(Duration::from_secs(8)).await;
    let next = IncomingBlock::downloaded(downloaded(&block(13)));
    h.service.handle_incoming_block(next, true).await;
    h.wait_for(MINUTE, |h| h.network.count("refresh_peers_after_fork") == 1)
        .await;
    assert!(h.node.log().contains(&"job [13]".to_owned()));
    assert_eq!(removals(&h), ["remove 1"]);
    assert_eq!(
        h.chain().forked_block.as_ref().map(|b| b.height()),
        Some(13)
    );
    assert_eq!(h.chain().last_height(), 11);
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
}

#[tokio::test(start_paused = true)]
async fn a_job_whose_first_block_does_not_chain_clears_the_queue_and_checks_for_a_fork() {
    let h = idle_at(12, RollbackDepth::Random).await;
    h.service.enqueue_blocks(incoming(range(15, 16)));
    h.wait_for(MINUTE, |h| h.network.count("check_for_fork") == 1)
        .await;
    assert!(h.node.log().contains(&"job [15, 16]".to_owned()));
    assert_eq!(h.chain().last_downloaded.map(|l| l.height), Some(12));
    assert!(removals(&h).is_empty(), "no fork found");
    assert_eq!(h.chain().last_height(), 12);
}

#[tokio::test(start_paused = true)]
async fn a_removal_is_at_most_the_tip_minus_one() {
    let h = idle_at(3, RollbackDepth::Random).await;
    h.service.remove_blocks(10).await.unwrap();
    assert_eq!(removals(&h), ["remove 2"]);
    assert_eq!(h.chain().last_height(), 1);
    assert_eq!(h.chain().last_downloaded.map(|l| l.height), Some(1));
    // At the genesis block there is nothing to remove, and the node is not asked.
    h.service.remove_blocks(1).await.unwrap();
    assert_eq!(removals(&h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_failed_removal_stops_the_recovery_and_exits() {
    let h = idle_at(20, RollbackDepth::Random).await;
    h.node.script().removal_fails = true;
    h.service.fork_block(Arc::new(block(20)), Some(4));
    h.wait_for(MINUTE, |h| h.state() == "exit").await;
    assert_eq!(h.network.count("refresh_peers_after_fork"), 0);
    let failed = h.service.remove_blocks(2).await;
    assert!(matches!(
        failed,
        Err(heartwood_blockchain::NodeError::Revert { .. })
    ));
}
