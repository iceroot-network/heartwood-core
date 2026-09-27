//! A start that refuses: a replay that fails with the reference's texts, and the boot
//! checks (another chain, a changed store, an incomplete identity, a genesis block that does not
//! decode or verify).

mod common;

use common::{Chain, Node, files_with, read, scratch, start};
use heartwood_blockchain::rows::transaction_rows;
use heartwood_blockchain::{ChainStore, RestoreMode, RestorePath, StartError, StoreError};
use heartwood_crypto::Block;
use heartwood_crypto::transactions::Asset;
use heartwood_database::{ChainMeta, IntegrityProblem, StateRow};
use heartwood_kernel::calculate_round;
use heartwood_state::errors::{ApplyError, GenesisError};
use heartwood_transactions::TransactionError;
use serde_json::Value;

/// The first block after height 2 that holds a transaction `select` accepts, is not the last
/// block of a round, and does not follow one: its rows can be replaced without touching a
/// round's rows or a milestone entry.
fn block_with(chain: &Chain, select: impl Fn(&Asset) -> bool) -> (u32, usize) {
    let rounds = |height| calculate_round(height, chain.milestones());
    chain
        .blocks
        .iter()
        .filter(|b| b.height() > 2 && rounds(b.height()).round == rounds(b.height() + 1).round)
        .filter(|b| rounds(b.height() - 1).round == rounds(b.height()).round)
        .find_map(|block| {
            let index = block
                .transactions()
                .iter()
                .position(|t| select(&t.data().asset))?;
            Some((block.height(), index))
        })
        .unwrap()
}

/// A node with the chain up to `tip`, whose stored block at `tip` has the bytes of its
/// transaction `index` changed by `tamper` (the block's id stays: it hashes the header only).
fn tampered<'c>(
    chain: &'c Chain,
    name: &str,
    tip: u32,
    index: usize,
    tamper: impl Fn(&mut Vec<u8>),
) -> Node<'c> {
    let milestones = chain.milestones();
    let mut node = Node::first_start(chain, name);
    for height in 2..=tip {
        node.step((Some(height), true));
    }
    let database = node.store().database();
    let stored = database.blocks(tip, tip).unwrap().remove(0);
    let missed: Vec<_> = (database.missed_blocks().unwrap().into_iter())
        .filter(|row| row.height == tip)
        .collect();
    let undo = database.undo_records().unwrap().pop().unwrap();
    assert_eq!(undo.height, tip);
    let block: &Block = chain.block(tip);
    let mut rows = transaction_rows(block, milestones).unwrap();
    tamper(&mut rows[index].serialised);
    let round = calculate_round(tip - 1, milestones).round;
    let mut chunk = node.store().database_mut().begin().unwrap();
    chunk.delete_above(tip - 1, round).unwrap();
    chunk.save_block(&stored.row, &rows).unwrap();
    chunk.save_missed(&missed).unwrap();
    chunk.put_undo(tip, &undo.record).unwrap();
    chunk.commit().unwrap();
    node
}

#[test]
fn a_chunk_before_the_start_is_refused() {
    let chain = Chain::load();
    let dir = scratch("chunk-before-start");
    let mut store = ChainStore::open(&common::settings(&dir)).unwrap();
    assert!(matches!(store.chunk(), Err(StoreError::NotStarted)));
    drop(store);
    let (mut store, restored) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::FirstStart);
    assert!(store.chunk().is_ok());
}

#[test]
fn a_replay_that_would_take_a_balance_below_zero_refuses_with_the_reference_text() {
    let chain = Chain::load();
    let (tip, index) = block_with(&chain, |asset| matches!(asset, Asset::Transfer { .. }));
    let Asset::Transfer { transfers } = &chain.block(tip).transactions()[index].data().asset else {
        unreachable!()
    };
    let amount = transfers[0].amount.to_le_bytes();
    let mut node = tampered(&chain, "negative-balance", tip, index, |bytes| {
        let at = bytes.windows(8).position(|w| w == amount).unwrap();
        bytes[at..at + 8].copy_from_slice(&(u64::MAX / 4).to_le_bytes());
    });
    // The state tables still hold the state of the stored block's original bytes: the table
    // path reads them without applying any transaction again.
    let restored = node.restart_with(&chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::Table);
    let error = node
        .restart_with(&chain.files, RestoreMode::Replay)
        .unwrap_err();
    assert_eq!(error, StartError::NegativeBalance { height: tip });
    assert_eq!(
        error.to_string(),
        "State Generation failed: Wallet with negative balance"
    );
}

#[test]
fn another_replay_failure_refuses_with_the_reference_prefix() {
    let chain = Chain::load();
    let (tip, index) = block_with(&chain, |_| true);
    // The nonce is the 8 bytes after the header type, version, network, type group and type.
    let mut node = tampered(&chain, "replay-failure", tip, index, |bytes| {
        let nonce = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
        bytes[9..17].copy_from_slice(&(nonce + 5).to_le_bytes());
    });
    let error = node
        .restart_with(&chain.files, RestoreMode::Replay)
        .unwrap_err();
    let StartError::Replay {
        height,
        error: ApplyError::Transaction { rejected, .. },
    } = &error
    else {
        panic!("{error:?}")
    };
    assert_eq!((*height, rejected.class()), (tip, "UnexpectedNonceError"));
    let text = error.to_string();
    assert!(
        text.starts_with("State Generation failed: transaction "),
        "{text}"
    );
    assert!(matches!(rejected, TransactionError::UnexpectedNonce { .. }));
}

#[test]
fn a_store_of_another_chain_or_changed_outside_the_node_refuses_the_start() {
    let chain = Chain::load();
    let mut node = Node::first_start(&chain, "boot-checks");
    for height in 2..=12 {
        node.step((Some(height), true));
    }
    let meta = node.store().database().meta().unwrap().unwrap();
    let row = node.store().database().state_row().unwrap().unwrap();

    let mut files = chain.files.clone();
    files.network.nethash = "00".repeat(32);
    let error = node.restart_with(&files, RestoreMode::Auto).unwrap_err();
    assert!(
        matches!(
            error,
            StartError::OtherChain {
                what: "nethash",
                ..
            }
        ),
        "{error:?}"
    );

    let mut chunk = node.store().database_mut().begin().unwrap();
    let other = *chain.block(5).id();
    chunk
        .put_meta(&ChainMeta {
            genesis_id: other,
            ..meta.clone()
        })
        .unwrap();
    chunk.commit().unwrap();
    let error = node
        .restart_with(&chain.files, RestoreMode::Auto)
        .unwrap_err();
    assert_eq!(
        error,
        StartError::OtherChain {
            what: "genesis block",
            stored: other.to_hex(),
            configured: chain.genesis().id().to_hex(),
        }
    );
    let mut chunk = node.store().database_mut().begin().unwrap();
    chunk.put_meta(&meta).unwrap();
    chunk
        .put_state(&StateRow {
            height: 11,
            ..row.clone()
        })
        .unwrap();
    chunk.commit().unwrap();
    let error = node
        .restart_with(&chain.files, RestoreMode::Auto)
        .unwrap_err();
    assert_eq!(
        error,
        StartError::Integrity(IntegrityProblem::StateRow {
            height: 11,
            tip: 12
        })
    );
    assert_eq!(
        error.to_string(),
        "the database failed a check at start: the state row is at height 11, but the tip is at \
         height 12"
    );

    // A store with blocks and no identity, or an identity and no block.
    let genesis = node.store().database().blocks(1, 1).unwrap().remove(0);
    node.store().database_mut().reset().unwrap();
    let mut chunk = node.store().database_mut().begin().unwrap();
    let rows = transaction_rows(&chain.genesis(), chain.milestones()).unwrap();
    chunk.save_block(&genesis.row, &rows).unwrap();
    chunk.commit().unwrap();
    let error = node
        .restart_with(&chain.files, RestoreMode::Auto)
        .unwrap_err();
    assert_eq!(
        error,
        StartError::Identity {
            detail: "no identity"
        }
    );
    node.store().database_mut().reset().unwrap();
    let mut chunk = node.store().database_mut().begin().unwrap();
    chunk.put_meta(&meta).unwrap();
    chunk.commit().unwrap();
    let error = node
        .restart_with(&chain.files, RestoreMode::Auto)
        .unwrap_err();
    assert_eq!(error, StartError::Identity { detail: "no block" });
}

#[test]
fn a_genesis_block_that_does_not_decode_or_verify_refuses_the_start() {
    let chain = Chain::load();
    let dir = scratch("genesis");
    let mut files = chain.files.clone();
    files.genesis = Value::Object(Default::default());
    let error = start(&dir, &files, RestoreMode::Auto).err().unwrap();
    assert!(
        matches!(error, StartError::GenesisDecode { .. }),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .starts_with("the genesis block could not be decoded: ")
    );

    // A payload length that is not 32 bytes per transaction: one more.
    let mut files = files_with(&read("crypto/milestones.json"));
    let length = files.genesis["payloadLength"].as_u64().unwrap();
    files.genesis["payloadLength"] = Value::from(length + 1);
    let error = start(&dir, &files, RestoreMode::Auto).err().unwrap();
    assert!(
        matches!(error, StartError::Genesis(GenesisError::Invalid { .. })),
        "{error:?}"
    );
    // Nothing was written.
    let (_, restored) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::FirstStart);
}
