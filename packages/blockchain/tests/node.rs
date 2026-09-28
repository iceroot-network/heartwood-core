//! The node thread on the committed one-node chain: the events and snapshots of each commit, the
//! pool's steps per admitted batch, per applied block, per round start and per drain, a removal
//! across a round end with its re-adds, a commit that fails and the start after it, and the
//! graceful stop.

mod common;
mod node_support;

use common::{Chain, assert_same_state, scratch, settings};
use heartwood_blockchain::events::NodeEvent;
use heartwood_blockchain::rows::block_row;
use heartwood_blockchain::{
    Channel, NodeError, OnlineWork, RestoreMode, RestorePath, StoreError, Work,
};
use heartwood_crypto::{PublicKeyBytes, TransactionId};
use heartwood_database::Database;
use heartwood_kernel::ChainLink;
use heartwood_pool::Entry;
use node_support::{Script, Scripted, TestNode, job, lock, pool_options, replayed};
use tokio::sync::oneshot;

/// A node at `tip`, whose online record holds every delegate of its state in round 1.
async fn node_at(chain: &Chain, name: &str, tip: u32) -> TestNode {
    let dir = scratch(name);
    let node = TestNode::start(chain, &dir, Scripted::with(&[]), pool_options(&dir, 2_700));
    if tip > 1 {
        node.run(chain, 2, tip).await;
    }
    let view = node.handle.state().borrow().clone();
    for wallet in view.wallets.delegates() {
        let key = wallet.public_key().unwrap().to_compressed();
        let version = "4.3.1".to_owned();
        let set = OnlineWork::Set {
            key,
            round: 1,
            version,
        };
        node.handle.online(set).unwrap();
    }
    lock(&node.events.0).clear();
    node
}

fn pooled(node: &TestNode) -> Vec<TransactionId> {
    let view = node.handle.pool().borrow().clone();
    let mut ids: Vec<_> = view.by_priority.iter().map(|t| *t.id()).collect();
    ids.sort();
    ids
}

fn block_ids(chain: &Chain, heights: &[u32]) -> Vec<TransactionId> {
    let transactions = heights.iter().flat_map(|h| chain.block(*h).transactions());
    let mut ids: Vec<_> = transactions.map(|transaction| *transaction.id()).collect();
    ids.sort();
    ids
}

fn applied(height: u32, round_started: Option<u64>) -> NodeEvent {
    NodeEvent::BlockApplied {
        height,
        round_started,
    }
}

#[tokio::test]
async fn each_commit_publishes_then_tells_its_events_in_the_reference_order() {
    let chain = Chain::load();
    let node = node_at(&chain, "node-events", 1).await;
    let mut state = node.handle.state();
    node.run(&chain, 2, 4).await;
    assert!(state.has_changed().unwrap());
    assert_eq!(state.borrow_and_update().height(), 4);
    // Height 2 starts a milestone of the chain's file.
    let milestone = NodeEvent::MilestoneChanged { height: 2 };
    let events = vec![
        milestone,
        applied(2, None),
        applied(3, None),
        applied(4, None),
    ];
    assert_eq!(*lock(&node.events.0), [events]);
    // Block 53 ends round 1, and so starts round 2.
    lock(&node.events.0).clear();
    node.run(&chain, 5, 54).await;
    let events = lock(&node.events.0).concat();
    let expected: Vec<_> = (5..=54)
        .map(|h| applied(h, (h == 53).then_some(2)))
        .collect();
    assert_eq!(events, expected);
    let observed = lock(&node.observed.0).clone();
    let expected: Vec<_> = (2..=54).map(|h| format!("applied {h} missed 0")).collect();
    assert_eq!(observed[0], "restarted 1");
    assert_eq!(observed[1..], expected);
    // A job that commits nothing publishes nothing.
    let before = node.handle.state().borrow().clone();
    node.handle.job(job(&chain, 20, 30)).await.unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &before,
        &node.handle.state().borrow()
    ));
    node.stop().await.unwrap();
}

#[tokio::test]
async fn a_job_frees_the_chain_state_between_its_blocks() {
    let chain = Chain::load();
    let node = node_at(&chain, "node-chain-lock", 1).await;
    node.run(&chain, 2, 6).await;
    // After each block's apply, the service and the network can read the chain state, and see
    // that block as the tip, as the reference's readers see each applied block.
    let tips = lock(&node.observed.1).clone();
    assert_eq!(tips, [2, 3, 4, 5, 6].map(Some));
    node.stop().await.unwrap();
}

#[tokio::test]
async fn the_pool_steps_per_batch_per_applied_block_and_per_drain() {
    let chain = Chain::load();
    let node = node_at(&chain, "node-pool-steps", 58).await;
    let entries: Vec<_> = (chain.block(59).transactions().iter())
        .map(|transaction| Entry::Bytes(transaction.bytes().to_vec()))
        .collect();
    let result = node
        .handle
        .pool_entries(Channel::Api, entries)
        .await
        .unwrap();
    assert_eq!(result.accept.len(), 4);
    let broadcast: Vec<_> = (result.broadcast.iter())
        .map(|id| TransactionId::from_hex(id).unwrap())
        .collect();
    assert_eq!(*lock(&node.broadcasts.0), [broadcast]);
    assert_eq!(pooled(&node), block_ids(&chain, &[59]));
    // The block that forges them removes them.
    node.run(&chain, 59, 59).await;
    assert!(pooled(&node).is_empty());
    node.handle.remove_blocks(1).await.unwrap();
    assert_eq!(
        pooled(&node),
        block_ids(&chain, &[59]),
        "re-added by the removal"
    );
    // A processor that forgets to remove them leaves them to the drain's re-add, which drops
    // them, as their nonces are used; then the online record is saved.
    lock(&node.processor.script).insert(59, Script::AcceptWithoutPool);
    node.run(&chain, 59, 59).await;
    assert_eq!(pooled(&node), block_ids(&chain, &[59]));
    let key = PublicKeyBytes::from_array([9; 33]);
    let version = "4.3.9".to_owned();
    let set = OnlineWork::Set {
        key,
        round: 2,
        version,
    };
    node.handle.online(set).unwrap();
    node.handle.drain().await.unwrap();
    assert!(pooled(&node).is_empty());
    let stored = Database::open(&settings(&node.dir)).unwrap();
    let rows = stored.delegate_versions().unwrap();
    let row = rows
        .iter()
        .find(|row| row.public_key == key.to_hex())
        .unwrap();
    assert_eq!((row.round, row.version.as_str()), (2, "4.3.9"));
    assert_eq!(rows.len(), node.handle.pool().borrow().online.len());
    node.stop().await.unwrap();
}

#[tokio::test]
async fn a_round_start_expires_the_online_record_after_the_commit() {
    let chain = Chain::load();
    let node = node_at(&chain, "node-round-start", 50).await;
    let key = |byte| PublicKeyBytes::from_array([byte; 33]);
    for (byte, round) in [(1, 0), (2, 1)] {
        let (key, version) = (key(byte), "4.3.1".to_owned());
        let set = OnlineWork::Set {
            key,
            round,
            version,
        };
        node.handle.online(set).unwrap();
    }
    let version = |byte| node.handle.pool().borrow().version(&key(byte)).is_some();
    node.run(&chain, 51, 52).await;
    assert!(version(1) && version(2));
    node.run(&chain, 53, 55).await;
    assert!(
        !version(1) && version(2),
        "an entry of round 0 expires when round 2 starts"
    );
    node.stop().await.unwrap();
}

#[tokio::test]
async fn a_removal_across_a_round_end_re_adds_its_transactions_in_chain_order() {
    let chain = Chain::load();
    let node = node_at(&chain, "node-removal", 60).await;
    let tip = node.handle.remove_blocks(10).await.unwrap();
    assert_eq!(tip, 50);
    let reverted = (51..=60)
        .rev()
        .map(|height| NodeEvent::BlockReverted { height });
    let reverted: Vec<_> = reverted.collect();
    assert_eq!(*lock(&node.events.0), [reverted]);
    let observed = lock(&node.observed.0).clone();
    let expected: Vec<_> = (50..=59).rev().map(|h| format!("reverted {h}")).collect();
    assert_eq!(observed[observed.len() - 10..], expected);
    assert_eq!(node.height(), 50);
    let status = node.handle.chain().borrow().clone();
    assert_eq!(status.stored_height, 50);
    assert_eq!(status.tip().map(|tip| tip.height()), Some(50));
    let downloaded = node_support::lock(&node.chain).last_downloaded;
    assert_eq!(downloaded, Some(ChainLink::of(chain.block(50))));
    // Every transaction of blocks 59 and 60 is back: one sender's seven transfers too, which
    // only their chain order admits.
    assert_eq!(pooled(&node), block_ids(&chain, &[59, 60]));
    // A removal of more blocks than the chain has keeps the genesis block; the new tips at the
    // chain's milestone heights are milestone changes.
    lock(&node.events.0).clear();
    assert_eq!(node.handle.remove_blocks(1_000).await.unwrap(), 1);
    let events = lock(&node.events.0).concat();
    let tail = [
        NodeEvent::BlockReverted { height: 3 },
        NodeEvent::MilestoneChanged { height: 2 },
        NodeEvent::BlockReverted { height: 2 },
        NodeEvent::MilestoneChanged { height: 1 },
    ];
    assert_eq!(events.len(), 49 + 2);
    assert_eq!(events[events.len() - 4..], tail);
    assert_eq!(
        node.handle.remove_blocks(3).await.unwrap(),
        1,
        "nothing to remove"
    );
    let dir = node.dir.clone();
    node.stop().await.unwrap();
    let (_, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::Table);
    assert_eq!(restored.state.height(), 1);
}

#[tokio::test]
async fn a_removal_leaves_the_state_the_chain_had_at_its_height() {
    let chain = Chain::load();
    let node = node_at(&chain, "node-removal-state", 110).await;
    assert_eq!(node.handle.remove_blocks(58).await.unwrap(), 52);
    let dir = node.dir.clone();
    node.stop().await.unwrap();
    let (_, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_same_state(&restored.state, &replayed(&chain, 52), "after the removal");
}

#[tokio::test]
async fn a_failed_commit_stops_the_node_and_the_next_start_restores_the_committed_tip() {
    let chain = Chain::load();
    let mut node = node_at(&chain, "node-commit-failure", 3).await;
    // A row the node did not write holds block 6's height, so the job's write of block 6 fails
    // after blocks 4 and 5 were written in the job's transaction.
    let mut other = Database::open(&settings(&node.dir)).unwrap();
    let row = block_row(chain.block(6), None, chain.milestones()).unwrap();
    let mut chunk = other.begin().unwrap();
    chunk.save_block(&row, &[]).unwrap();
    chunk.commit().unwrap();
    assert!(
        node.handle.job(job(&chain, 4, 7)).await.is_err(),
        "no answer"
    );
    let stopped = node.node.take().unwrap().join();
    assert!(matches!(
        stopped,
        Err(NodeError::Store(StoreError::Database(_)))
    ));
    let mut chunk = other.begin().unwrap();
    chunk.delete_above(3, 1).unwrap();
    chunk.commit().unwrap();
    drop(other);
    let (store, restored) = common::start(&node.dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::Table);
    assert_same_state(&restored.state, &replayed(&chain, 3), "the committed tip");
    // The node goes on from there.
    let dir = node.dir.clone();
    let processor = Scripted::with(&[]);
    let options = pool_options(&dir, 2_700);
    let node = TestNode::spawn(
        &chain, &dir, processor, options, node.clock, store, restored,
    );
    assert_eq!(node.run(&chain, 4, 7).await.committed, 4);
    node.stop().await.unwrap();
}

#[tokio::test]
async fn a_stop_comes_after_the_running_job_and_the_node_saves_its_online_record() {
    let chain = Chain::load();
    let node = node_at(&chain, "node-stop", 1).await;
    let key = PublicKeyBytes::from_array([4; 33]);
    let version = "4.3.4".to_owned();
    let set = OnlineWork::Set {
        key,
        round: 3,
        version,
    };
    node.handle.online(set).unwrap();
    let (answer, outcome) = oneshot::channel();
    node.handle
        .submit(Work::Job(job(&chain, 2, 40), answer))
        .unwrap();
    let handle = node.handle.clone();
    let dir = node.dir.clone();
    node.stop().await.unwrap();
    assert_eq!(
        outcome.await.unwrap().committed,
        39,
        "the job before the stop ran"
    );
    assert!(
        handle.job(job(&chain, 41, 42)).await.is_err(),
        "no work after the stop"
    );
    let (_, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.state.height(), 40);
    assert_eq!(restored.online.version(&key), Some("4.3.4"));
    assert_same_state(&restored.state, &replayed(&chain, 40), "after the stop");
}
