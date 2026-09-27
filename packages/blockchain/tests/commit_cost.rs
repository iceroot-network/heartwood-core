//! The cost of the store's commit, measured (ignored by default; `make pool-bench` runs it in a
//! release build): the time to write and commit one block per chunk with 1, 10 and 150
//! transactions, and the bytes per block of the database file and of the undo records on the
//! committed one-node chain.

#![allow(
    clippy::disallowed_types,
    reason = "a measurement reads the wall clock"
)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Chain, Node, scratch, start};
use heartwood_blockchain::{RestoreMode, RestorePath};
use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::TransferItem;
use heartwood_crypto::{Aux, Block, KeyPair, Milestones, Network, Transaction};
use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_state::State;
use heartwood_transactions::{Handlers, minimum_fee};
use serde_json::Value;

const V10: &str = include_str!("../../crypto/tests/vectors/V10-genesis.jsonl");

/// The first devnet chain definition of the V10 vectors, with every key by name.
fn devnet() -> (CryptoFiles, BTreeMap<String, KeyPair>) {
    let record: Value = V10
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["op"] == "genesis.generate")
        .unwrap();
    let output = &record["output"];
    let file = |name: &str| output["files"][name].as_str().unwrap().to_owned();
    let network = Network::load(&file("crypto/network.json")).unwrap();
    let text = file("crypto/milestones.json");
    let milestones = Milestones::load(&text, &network).unwrap();
    let files = CryptoFiles {
        network,
        milestones,
        milestones_text: text,
        genesis: serde_json::from_str(&file("crypto/genesisBlock.json")).unwrap(),
    };
    let mut keys = BTreeMap::new();
    for (list, name) in [
        ("delegates", "username"),
        ("genesisWallets", "role"),
        ("testWallets", "role"),
    ] {
        for entry in output[list].as_array().unwrap() {
            let passphrase = entry["passphrase"].as_str().unwrap();
            let key = KeyPair::from_passphrase(passphrase).unwrap();
            keys.insert(entry[name].as_str().unwrap().to_owned(), key);
        }
    }
    (files, keys)
}

/// The next block of `state` with `transactions`, forged in the next slot by the delegate it
/// schedules.
fn next_block(
    state: &State,
    files: &CryptoFiles,
    keys: &BTreeMap<String, KeyPair>,
    transactions: Vec<Transaction>,
) -> Block {
    let milestones = &files.milestones;
    let block_time = u32::try_from(milestones.at(1).block_time()).unwrap();
    let last = state.last_block();
    let timestamp = last.header().timestamp + block_time;
    let forging = state.forging_delegates();
    let forger = &forging[usize::try_from(timestamp / block_time).unwrap() % forging.len()];
    let keys = &keys[&forger.username];
    let height = last.height() + 1;
    let reward = state
        .reward_for_block_in_round(height, &forger.public_key, milestones)
        .unwrap()
        .reward;
    let count = u32::try_from(transactions.len()).unwrap();
    let draft = BlockDraft {
        version: 0,
        timestamp,
        height,
        previous_block: *last.id().as_bytes(),
        number_of_transactions: count,
        total_amount: transactions.iter().map(Transaction::amount).sum(),
        total_fee: transactions.iter().map(Transaction::fee).sum(),
        reward,
        payload_length: 32 * count,
        payload_hash: sha256_concat(transactions.iter().map(|t| t.id().as_bytes().as_slice())),
    };
    factory::make(
        draft,
        transactions,
        keys,
        Aux::fixed([0x42; 32]),
        milestones,
    )
    .unwrap()
}

/// `count` transfers of 1 unit from the richest of `keys` to the test wallets, in nonce order.
fn transfers(
    state: &State,
    files: &CryptoFiles,
    keys: &BTreeMap<String, KeyPair>,
    count: usize,
) -> Vec<Transaction> {
    let network = files.network.pub_key_hash;
    let address = |keys: &KeyPair| Address::from_public_key(keys.public_key(), network);
    let wallet = |keys: &KeyPair| state.wallets().find_by_address(&address(keys)).cloned();
    let sender = keys
        .values()
        .max_by_key(|keys| wallet(keys).map_or(0, |w| w.balance()))
        .unwrap();
    let nonce = wallet(sender).map_or(0, |w| w.nonce());
    let recipients: Vec<Address> = (keys.iter())
        .filter(|(name, _)| name.starts_with("wallet/"))
        .map(|(_, keys)| address(keys))
        .collect();
    let params = files.milestones.at(state.height() + 1);
    (0..count)
        .map(|i| {
            let item = TransferItem {
                amount: 1,
                recipient: recipients[i % recipients.len()],
            };
            let builder = TransactionBuilder::transfer(vec![item]).nonce(nonce + 1 + i as u64);
            let transaction = builder
                .clone()
                .build(sender, Aux::fixed([7; 32]), params)
                .unwrap();
            let fee = u64::try_from(minimum_fee(&transaction, params)).unwrap();
            builder
                .fee(fee.max(transaction.fee()))
                .build(sender, Aux::fixed([7; 32]), params)
                .unwrap()
        })
        .collect()
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn bytes(dir: &Path) -> u64 {
    let database = dir.join("database");
    fs::read_dir(&database)
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum()
}

#[test]
#[ignore = "a measurement, run in a release build by make pool-bench"]
fn commit_cost() {
    let (files, keys) = devnet();
    let milestones = &files.milestones;
    for count in [1, 10, 150] {
        let dir = scratch(&format!("commit-cost-{count}"));
        let (mut store, restored) = start(&dir, &files, RestoreMode::Auto).unwrap();
        assert_eq!(restored.path, RestorePath::FirstStart);
        let mut state = restored.state;
        let mut times = Vec::new();
        for _ in 0..21 {
            let block = next_block(
                &state,
                &files,
                &keys,
                transfers(&state, &files, &keys, count),
            );
            let applied = state.apply_block(&block, milestones, &Handlers).unwrap();
            let started = Instant::now();
            let mut chunk = store.chunk().unwrap();
            chunk.applied(&state, &block, &applied, milestones).unwrap();
            chunk.commit().unwrap();
            times.push(started.elapsed());
        }
        println!(
            "commit of a block with {count} transaction(s): median {:?} of 21 \
             (min {:?}, max {:?}), {} wallets",
            median(times.clone()),
            times.iter().min().unwrap(),
            times.iter().max().unwrap(),
            state.wallets().len(),
        );
    }

    let chain = Chain::load();
    let mut node = Node::first_start(&chain, "commit-cost-c1");
    let before = bytes(&node.dir);
    let started = Instant::now();
    for height in 2..=chain.tip() {
        node.step((Some(height), true));
    }
    let elapsed = started.elapsed();
    let blocks = u64::from(chain.tip() - 1);
    let undo: u64 = (node.store().database().undo_records().unwrap().iter())
        .map(|row| row.record.len() as u64)
        .sum();
    let heavy = node
        .store()
        .database()
        .undo_records()
        .unwrap()
        .iter()
        .map(|r| r.record.len())
        .max()
        .unwrap();
    // Closing the store checkpoints its write-ahead log into the file.
    let dir = node.dir.clone();
    drop(node);
    let after = bytes(&dir);
    println!(
        "txgen-one-node, {blocks} blocks one per chunk: {:?} per block (apply, digest and \
         commit); database {} bytes per block ({before} bytes after the first start, {after} at \
         the tip); undo records {} bytes per block, the largest {heavy} bytes",
        elapsed / u32::try_from(blocks).unwrap(),
        (after - before) / blocks,
        undo / blocks,
    );
}
