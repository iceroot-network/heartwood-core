//! A pool storage failure in the accept step: the block stays applied, the job commits its
//! chunk, and then the node stops; the next start restores that tip, and its first re-add checks
//! the stored transactions against it, which drops the ones the block forged.

mod common;
mod node_support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use common::{Chain, assert_same_state, scratch, settings};
use heartwood_blockchain::{
    BlockProcessor, Channel, NodeError, OnlineWork, RestoreMode, RestorePath,
};
use heartwood_crypto::TransactionId;
use heartwood_database::Database;
use heartwood_pool::{Entry, Storage, StorageError};
use node_support::{Scripted, TestNode, job, pool_options, replayed};

/// The ids of the pool's transactions, sorted.
fn pooled(node: &TestNode) -> Vec<TransactionId> {
    let view = node.handle.pool().borrow().clone();
    let mut ids: Vec<_> = view.by_priority.iter().map(|t| *t.id()).collect();
    ids.sort();
    ids
}

/// `ids`, sorted.
fn sorted<'a>(ids: impl Iterator<Item = &'a TransactionId>) -> Vec<TransactionId> {
    let mut ids: Vec<_> = ids.copied().collect();
    ids.sort();
    ids
}

/// Set the mode of every file in the pool's directory: its file and the `-wal` and `-shm` files
/// that a read-only connection leaves behind.
fn set_pool_mode(dir: &Path, mode: u32) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }
}

#[tokio::test]
async fn a_pool_storage_failure_in_the_accept_step_stops_the_node_after_its_commit() {
    let chain = Chain::load();
    let dir = scratch("pool-storage-failure");
    let options = || pool_options(&dir, 2_700);
    let (file, forged) = (options().storage, chain.block(59).transactions());
    let forged_ids = sorted(forged.iter().map(|transaction| transaction.id()));
    // A node at 58, whose online record holds every delegate (block 59 votes), admits and
    // stores block 59's transactions, and stops.
    let node = TestNode::start(&chain, &dir, Scripted::with(&[]), options());
    node.run(&chain, 2, 58).await;
    let view = node.handle.state().borrow().clone();
    for wallet in view.wallets.delegates() {
        let (key, version) = (wallet.public_key().unwrap().to_compressed(), "4.3.1".into());
        let set = OnlineWork::Set {
            key,
            round: 1,
            version,
        };
        node.handle.online(set).unwrap();
    }
    let entries = forged.iter().map(|t| Entry::Bytes(t.bytes().to_vec()));
    let result = node.handle.pool_entries(Channel::Api, entries.collect());
    assert_eq!(result.await.unwrap().accept.len(), forged.len());
    node.stop().await.unwrap();
    // The pool's files become read-only: the pool opens and reads them, and a removal fails.
    let pool_dir = file.parent().unwrap();
    set_pool_mode(pool_dir, 0o444);
    if fs::OpenOptions::new().write(true).open(&file).is_ok() {
        set_pool_mode(pool_dir, 0o644);
        eprintln!("skipped: this user writes a read-only file (as root does), so no failure");
        return;
    }
    let processor = Scripted::through(Arc::new(BlockProcessor));
    let mut node = TestNode::start(&chain, &dir, processor, options());
    node.handle.drain().await.unwrap();
    assert_eq!(pooled(&node), forged_ids, "re-added from the pool's file");
    // Block 59 forges them: the accept step's removal fails, and the node stops after the
    // job's commit.
    let job = node.enqueued(job(&chain, 59, 59));
    assert!(node.handle.job(job).await.is_err(), "no answer");
    let stopped = node.node.take().unwrap().join();
    let Err(NodeError::Pool(StorageError::Sqlite(why))) = &stopped else {
        panic!("{stopped:?}");
    };
    assert!(why.contains("readonly"), "{why}");
    let stored = Database::open(&settings(&dir)).unwrap().last_block();
    let stored = stored.unwrap().unwrap().row;
    assert_eq!((stored.height, stored.id), (59, *chain.block(59).id()));
    // The removal left the block's transactions in the pool's file.
    set_pool_mode(pool_dir, 0o644);
    let rows = Storage::open(&file, None).unwrap().all().unwrap();
    assert_eq!(sorted(rows.iter().map(|row| &row.id)), forged_ids);
    // A start from the same directory restores block 59, and its first re-add leaves none of
    // the block's transactions in the pool or its file.
    let (store, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::Table);
    assert_same_state(&restored.state, &replayed(&chain, 59), "the committed tip");
    let (processor, clock) = (Scripted::with(&[]), node.clock);
    let node = TestNode::spawn(&chain, &dir, processor, options(), clock, store, restored);
    node.handle.drain().await.unwrap();
    assert!(pooled(&node).is_empty());
    node.stop().await.unwrap();
    let rows = Storage::open(&file, None).unwrap().all().unwrap();
    assert!(rows.is_empty());
}
