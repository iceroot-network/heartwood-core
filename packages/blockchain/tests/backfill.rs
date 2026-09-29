//! The productivity backfill of a start, on the committed chain's configuration with blocks built
//! on its genesis block: a store that holds no missed-slot row gets, at its start, the rows of the
//! slots missed between the genesis block and block 2, as the reference's start writes them
//! (`calculateMissedBlocks`: height 2, round 1's forging list by slot, one block time apart from
//! the genesis block's slot), on the table path and on a replay alike; a store that holds a
//! missed-slot row gets none.

mod common;

use common::{Chain, scratch, start};
use heartwood_blockchain::{ChainStore, ReplayReason, RestoreMode, RestorePath};
use heartwood_crypto::blocks::{BlockHeader, factory, serialiser};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use heartwood_crypto::genesis::DerivedKey;
use heartwood_crypto::genesis::derive::role_key;
use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_crypto::{Aux, Block};
use heartwood_database::MissedRow;
use heartwood_state::State;
use heartwood_state::round_state::shuffle_delegates;
use heartwood_transactions::Handlers;

/// The devnet tooling's key-derivation tag.
const TAG: &str = "solar-devnet-v1/";

/// The keys of the chain's 53 delegates, which its public seed derives.
fn delegate_keys(chain: &Chain) -> Vec<DerivedKey> {
    let meta: serde_json::Value = serde_json::from_str(&common::read("corpus.json")).unwrap();
    let seed = meta["seed"].as_str().unwrap();
    let network = chain.files.network.pub_key_hash;
    (1..=53)
        .map(|k| role_key(TAG, &format!("delegate/{k}"), seed, network).unwrap())
        .collect()
}

/// The next block on `state`, empty, in the first slot at least `gap` slots after the one that
/// follows the tip's, by the delegate the forging list schedules for that slot.
fn next_block(chain: &Chain, keys: &[DerivedKey], state: &State, gap: u32) -> Block {
    let ms = chain.milestones();
    let last = state.last_block().header();
    let block_time = u32::try_from(ms.at(1).block_time()).unwrap();
    let slot = last.timestamp / block_time + 1 + gap;
    let forging = state.forging_delegates();
    let forger = &forging[slot as usize % forging.len()];
    let signer = keys
        .iter()
        .find(|k| k.keys.public_key().to_compressed() == forger.public_key)
        .unwrap();
    let height = last.height + 1;
    let reward = state
        .reward_for_block_in_round(height, &forger.public_key, ms)
        .unwrap()
        .reward;
    let mut header = BlockHeader {
        version: 0,
        timestamp: slot * block_time,
        height,
        previous_block: *state.last_block().id().as_bytes(),
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward,
        payload_length: 0,
        payload_hash: sha256(&[]),
        generator_public_key: forger.public_key,
        block_signature: Signature::from_array([0; 64]),
    };
    header.block_signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &serialiser::signing_hash(&header),
        signer.keys.secret_key(),
        Aux::fixed([0x5a; 32]),
    )
    .unwrap();
    factory::from_data(header, &[], ms).unwrap()
}

/// A first start in a fresh store, then the blocks built with `gaps` (one block per gap), each
/// applied and committed in a chunk of its own. The store and the state at its tip.
fn chain_with(chain: &Chain, name: &str, gaps: &[u32]) -> (std::path::PathBuf, State) {
    let keys = delegate_keys(chain);
    let dir = scratch(name);
    let (mut store, restored) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::FirstStart);
    let mut state = restored.state;
    for &gap in gaps {
        let block = next_block(chain, &keys, &state, gap);
        let applied = state
            .apply_block(&block, chain.milestones(), &Handlers)
            .unwrap();
        let mut chunk = store.chunk().unwrap();
        chunk
            .applied(&state, &block, &applied, chain.milestones())
            .unwrap();
        chunk.commit().unwrap();
    }
    (dir, state)
}

/// The rows the reference's backfill writes for block 2 in `slot`: for each slot from 1 below it,
/// the delegate of round 1's stored rows, shuffled for round 1, at that slot, and the slot's time.
fn reference_rows(store: &ChainStore, state: &State, slot: u32) -> Vec<MissedRow> {
    let rows = store.database().round(1).unwrap();
    let names: Vec<String> = rows
        .iter()
        .map(|row| {
            let key = PublicKeyBytes::from_hex(&row.public_key).unwrap();
            let wallet = state.wallets().find_by_public_key(&key).unwrap();
            wallet.delegate().unwrap().username().to_owned()
        })
        .collect();
    let order = shuffle_delegates(1, &names);
    (1..slot)
        .map(|s| MissedRow {
            timestamp: i64::from(s) * 8,
            height: 2,
            username: order[s as usize % order.len()].clone(),
        })
        .collect()
}

#[test]
fn a_start_with_no_missed_row_writes_the_slots_before_block_2_on_either_path() {
    let chain = Chain::load();
    for (name, mode, path) in [
        ("backfill-table", RestoreMode::Auto, RestorePath::Table),
        (
            "backfill-replay",
            RestoreMode::Replay,
            RestorePath::Replay(ReplayReason::Requested),
        ),
    ] {
        // Block 2 in slot 4, block 3 in the next slot: no apply counts a missed slot.
        let (dir, _) = chain_with(&chain, name, &[3, 0]);
        let (store, restored) = start(&dir, &chain.files, mode).unwrap();
        assert_eq!(restored.path, path, "{name}");
        let rows = store.database().missed_blocks().unwrap();
        assert_eq!(rows, reference_rows(&store, &restored.state, 4), "{name}");
        assert_eq!(rows.len(), 3);
        // Rows are there now: the next start writes nothing.
        drop(store);
        let (store, _) = start(&dir, &chain.files, mode).unwrap();
        assert_eq!(store.database().missed_blocks().unwrap(), rows, "{name}");
    }
}

#[test]
fn a_start_with_a_missed_row_or_block_2_in_slot_1_writes_none() {
    let chain = Chain::load();
    // Block 2 in slot 3 and block 3 two slots late: the apply of block 3 wrote its two rows.
    let (dir, _) = chain_with(&chain, "backfill-has-row", &[2, 2]);
    let (store, _) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    let rows = store.database().missed_blocks().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.height == 3), "{rows:?}");
    drop(store);
    // Block 2 in slot 1: no slot before it to count.
    let (dir, _) = chain_with(&chain, "backfill-on-time", &[0, 0]);
    let (store, _) = start(&dir, &chain.files, RestoreMode::Replay).unwrap();
    assert!(store.database().missed_blocks().unwrap().is_empty());
}
