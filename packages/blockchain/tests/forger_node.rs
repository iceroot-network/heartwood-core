//! The node thread's work for the peer-to-peer server and the node's own forger, on the
//! committed one-node chain: the delegates on this node in the online record at a round start and
//! at the sync's end, the collation of the pool's candidates, and the P2P channel's processor with
//! its own cooldown and its broadcast.

mod common;
mod node_support;

use std::sync::Arc;

use common::{Chain, scratch};
use heartwood_blockchain::{Channel, OnlineWork, OwnKeys, RestoreMode};
use heartwood_crypto::{PublicKeyBytes, TransactionId};
use heartwood_kernel::clock::ManualClock;
use heartwood_pool::{Entry, PoolCode};
use node_support::{Scripted, TestNode, at_epoch_seconds, lock, pool_options, replayed};

/// The version the test nodes give the delegates on them.
const VERSION: &str = "4.3.1-heartwood.0.1.0";

/// The delegates on a test node, as the forger's attestation session would give them.
struct Attested(Vec<PublicKeyBytes>);

impl OwnKeys for Attested {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        self.0.clone()
    }
}

/// A key of the chain's first registered delegate, and one that is no delegate's.
fn keys(chain: &Chain) -> (PublicKeyBytes, PublicKeyBytes) {
    let state = replayed(chain, 1);
    let delegate = state.wallets().delegates().next().unwrap();
    let key = delegate.public_key().unwrap().to_compressed();
    (key, PublicKeyBytes::from_array([2; 33]))
}

/// A node from genesis with the delegates on it `own`, run to `tip`.
async fn node_with(chain: &Chain, name: &str, own: Vec<PublicKeyBytes>, tip: u32) -> TestNode {
    let dir = scratch(name);
    let last = chain.blocks.last().unwrap().header().timestamp;
    let clock = Arc::new(ManualClock::new(at_epoch_seconds(chain, last + 60)));
    let (store, restored) = common::start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    let options = pool_options(&dir, 2_700);
    let processor = Scripted::with(&[]);
    let parts = (clock, store, restored);
    let node = TestNode::spawn_with(
        chain,
        &dir,
        processor,
        options,
        parts,
        Arc::new(Attested(own)),
    );
    if tip > 1 {
        node.run(chain, 2, tip).await;
    }
    node
}

/// The online record's entry of `key`: its round and version.
fn entry(node: &TestNode, key: &PublicKeyBytes) -> Option<(u64, String)> {
    let view = node.handle.pool().borrow().clone();
    let entry = view.online.iter().find(|entry| entry.public_key == *key);
    entry.map(|entry| (entry.round, entry.version.clone()))
}

#[tokio::test]
async fn the_delegates_on_this_node_take_its_version_at_a_round_start() {
    let chain = Chain::load();
    let (delegate, stranger) = keys(&chain);
    let node = node_with(
        &chain,
        "forger-node-round",
        vec![delegate, stranger, delegate],
        52,
    )
    .await;
    assert_eq!(
        entry(&node, &delegate),
        None,
        "no round started since genesis"
    );
    node.run(&chain, 53, 55).await;
    let round = heartwood_kernel::calculate_round(55, chain.milestones()).round;
    assert_eq!(entry(&node, &delegate), Some((round, VERSION.into())));
    assert_eq!(
        entry(&node, &stranger),
        None,
        "a key of no delegate is never set"
    );
    node.stop().await.unwrap();
}

#[tokio::test]
async fn the_delegates_on_this_node_take_its_version_at_the_syncs_end_and_never_expire() {
    let chain = Chain::load();
    let (delegate, stranger) = keys(&chain);
    let node = node_with(&chain, "forger-node-sync", vec![delegate, stranger], 20).await;
    node.handle.online(OnlineWork::Expire { round: 1 }).unwrap();
    let mut pool = node.handle.pool();
    pool.wait_for(|view| view.version(&delegate).is_some())
        .await
        .unwrap();
    assert_eq!(entry(&node, &delegate), Some((1, VERSION.into())));
    // A later sync's end sets it again before the expiry, so it never expires.
    node.handle.online(OnlineWork::Expire { round: 9 }).unwrap();
    pool.wait_for(|view| view.online.iter().any(|e| e.round == 9))
        .await
        .unwrap();
    assert_eq!(entry(&node, &delegate), Some((9, VERSION.into())));
    assert_eq!(entry(&node, &stranger), None);
    node.stop().await.unwrap();
}

#[tokio::test]
async fn the_node_collates_the_pools_candidates_and_the_p2p_channel_has_its_own_cooldown() {
    let chain = Chain::load();
    let node = node_with(&chain, "forger-node-collate", Vec::new(), 58).await;
    // Every delegate announced a version, so the votes pass the online check.
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
    let transactions = chain.block(59).transactions();
    let entries: Vec<Entry> = (transactions.iter())
        .map(|transaction| Entry::Bytes(transaction.bytes().to_vec()))
        .collect();
    let result = node
        .handle
        .pool_entries(Channel::P2p, entries.clone())
        .await
        .unwrap();
    assert_eq!(result.accept.len(), 4);
    let broadcast: Vec<TransactionId> = (result.broadcast.iter())
        .map(|id| TransactionId::from_hex(id).unwrap())
        .collect();
    assert_eq!(
        *lock(&node.broadcasts.0),
        [broadcast],
        "broadcast as admitted"
    );
    // The same entries again: the P2P processor saw them, the API's did not.
    let again = node
        .handle
        .pool_entries(Channel::P2p, entries.clone())
        .await
        .unwrap();
    let codes = |result: &heartwood_pool::ProcessorResult| -> Vec<PoolCode> {
        let errors = result.errors.as_ref().unwrap();
        errors.values().map(|refusal| refusal.code).collect()
    };
    assert_eq!(codes(&again), [PoolCode::Cooldown; 4]);
    let api = node
        .handle
        .pool_entries(Channel::Api, entries)
        .await
        .unwrap();
    assert!(!codes(&api).contains(&PoolCode::Cooldown));
    // The collation of a forger (validated) and of a peer (not), without the excluded ids.
    let mut ours: Vec<Vec<u8>> = transactions.iter().map(|t| t.bytes().to_vec()).collect();
    ours.sort();
    let forger = node.handle.collate(true, Vec::new()).await.unwrap();
    let mut collated = forger.transactions.clone();
    collated.sort();
    assert_eq!((forger.pool_size, collated), (4, ours));
    let excluded = vec![*transactions[0].id()];
    let peer = node.handle.collate(false, excluded).await.unwrap();
    assert_eq!((peer.pool_size, peer.transactions.len()), (4, 3));
    assert!(
        !peer
            .transactions
            .contains(&transactions[0].bytes().to_vec())
    );
    // Once forged, nothing is left to collate.
    node.run(&chain, 59, 59).await;
    let empty = node.handle.collate(true, Vec::new()).await.unwrap();
    assert_eq!((empty.pool_size, empty.transactions.len()), (0, 0));
    node.stop().await.unwrap();
}
