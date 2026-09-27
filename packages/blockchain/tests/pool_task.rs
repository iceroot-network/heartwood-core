//! The pool on the node thread, on the committed one-node chain's transactions: each processor
//! with its own cooldown and the admitted transactions it broadcasts, a forged transaction removed
//! with its sender's earlier ones, a removal's transactions re-added in chain order, the online
//! record's expiry at each round start before the clean-up, the online record's view and store.

mod common;
mod node_support;

use std::sync::Arc;

use common::{Chain, scratch, settings};
use heartwood_blockchain::pool_task::PoolEnv;
use heartwood_blockchain::processor::PoolSteps;
use heartwood_blockchain::{ChainStore, Channel, OnlineWork, PoolTask};
use heartwood_crypto::{PublicKeyBytes, Transaction, TransactionId};
use heartwood_kernel::ManualClock;
use heartwood_pool::{DelegateVersions, Entry, Pool, PoolCode};
use heartwood_state::State;
use node_support::{at_epoch_seconds, pool_options, replayed};

/// A pool task in a fresh directory whose online record holds every delegate of `state`, in
/// round 1.
fn task(name: &str, max_transaction_age: u32, state: &State) -> PoolTask {
    let dir = scratch(name);
    let pool = Pool::open(pool_options(&dir, max_transaction_age), None).unwrap();
    let mut task = PoolTask::new(pool, DelegateVersions::new());
    for wallet in state.wallets().delegates() {
        let key = wallet.public_key().unwrap().to_compressed();
        let version = "4.3.1".to_owned();
        task.online(OnlineWork::Set {
            key,
            round: 1,
            version,
        });
    }
    task
}

fn clock(chain: &Chain) -> ManualClock {
    ManualClock::new(at_epoch_seconds(chain, 3_000))
}

fn env<'a>(chain: &'a Chain, state: &'a State, clock: &'a ManualClock) -> PoolEnv<'a> {
    PoolEnv {
        state,
        milestones: chain.milestones(),
        network: &chain.files.network,
        clock,
    }
}

fn entries(chain: &Chain, height: u32) -> Vec<Entry> {
    let transactions = chain.block(height).transactions();
    let bytes = |transaction: &Transaction| Entry::Bytes(transaction.bytes().to_vec());
    transactions.iter().map(bytes).collect()
}

fn ids(transactions: &[Transaction]) -> Vec<TransactionId> {
    transactions
        .iter()
        .map(|transaction| *transaction.id())
        .collect()
}

fn pooled(task: &PoolTask) -> Vec<TransactionId> {
    let mut ids: Vec<_> = task.view().by_priority.iter().map(|t| *t.id()).collect();
    ids.sort();
    ids
}

fn sorted(mut ids: Vec<TransactionId>) -> Vec<TransactionId> {
    ids.sort();
    ids
}

#[test]
fn each_processor_keeps_its_own_cooldown_and_returns_what_it_admitted_for_broadcast() {
    let chain = Chain::load();
    let (state, clock) = (replayed(&chain, 58), clock(&chain));
    let mut task = task("pool-task-processors", 2_700, &state);
    let block = ids(chain.block(59).transactions());
    let env = env(&chain, &state, &clock);
    let (result, broadcast) = task
        .process(Channel::Api, &env, entries(&chain, 59))
        .unwrap();
    let hex: Vec<_> = block.iter().map(TransactionId::to_hex).collect();
    assert_eq!(result.accept, hex);
    assert_eq!(result.broadcast, hex, "every fee passes the broadcast rule");
    assert_eq!(
        ids(&broadcast),
        block,
        "the admitted transactions, in the result's order"
    );
    assert_eq!(pooled(&task), sorted(block.clone()));
    // The same entries again: the API's processor saw them less than 30 seconds ago; the
    // download's processor did not, and the pool refuses them as duplicates.
    let mut codes = |channel| {
        let (result, broadcast) = task.process(channel, &env, entries(&chain, 59)).unwrap();
        assert!(result.accept.is_empty() && broadcast.is_empty());
        let errors = result.errors.unwrap();
        errors
            .values()
            .map(|refusal| refusal.code)
            .collect::<Vec<_>>()
    };
    assert_eq!(codes(Channel::Api), [PoolCode::Cooldown; 4]);
    assert_eq!(codes(Channel::Download), [PoolCode::Duplicate; 4]);
}

#[test]
fn a_forged_transaction_leaves_with_the_senders_earlier_ones() {
    let chain = Chain::load();
    let (state, clock) = (replayed(&chain, 59), clock(&chain));
    let mut task = task("pool-task-forged", 2_700, &state);
    let env = env(&chain, &state, &clock);
    let (result, _) = task
        .process(Channel::Download, &env, entries(&chain, 60))
        .unwrap();
    assert_eq!(result.accept.len(), 13);
    // The third of one sender's seven transfers is forged: the first three leave.
    let block = chain.block(60).transactions();
    let params = chain.milestones().at(60);
    task.remove_forged(&block[3], params);
    let left: Vec<_> = (block.iter().enumerate())
        .filter(|(index, _)| !(1..=3).contains(index))
        .map(|(_, transaction)| *transaction.id())
        .collect();
    assert_eq!(pooled(&task), sorted(left));
    assert!(task.take_failure().is_none());
}

#[test]
fn a_removals_transactions_re_add_in_chain_order() {
    let chain = Chain::load();
    let (state, clock) = (replayed(&chain, 50), clock(&chain));
    let env = env(&chain, &state, &clock);
    let removed: Vec<Transaction> = [59, 60]
        .iter()
        .flat_map(|height| chain.block(*height).transactions().to_vec())
        .collect();
    let mut task_in_order = task("pool-task-readd", 2_700, &state);
    let readded = task_in_order.readd(&env, &removed).unwrap();
    assert_eq!(readded.forged, ids(&removed));
    assert!(readded.forged_failed.is_empty());
    assert_eq!(pooled(&task_in_order), sorted(ids(&removed)));
    // Newest first, a sender's later nonces come before its earlier ones and fail.
    let reversed: Vec<Transaction> = removed.iter().rev().cloned().collect();
    let mut task_reversed = task("pool-task-readd-reversed", 2_700, &state);
    let readded = task_reversed.readd(&env, &reversed).unwrap();
    assert_eq!(readded.forged_failed.len(), 9);
}

#[test]
fn after_a_commit_the_online_record_expires_at_each_round_start_then_the_pool_is_cleaned_up() {
    let chain = Chain::load();
    let clock = clock(&chain);
    let state = replayed(&chain, 50);
    // Five blocks is the longest a transaction stays.
    let mut task = task("pool-task-committed", 5, &state);
    let key = |byte| PublicKeyBytes::from_array([byte; 33]);
    for (byte, round) in [(1, 0), (2, 1), (3, 5)] {
        let (key, version) = (key(byte), "4.3.1".to_owned());
        task.online(OnlineWork::Set {
            key,
            round,
            version,
        });
    }
    let removed = chain.block(59).transactions().to_vec();
    task.readd(&env(&chain, &state, &clock), &removed).unwrap();
    let online = |task: &PoolTask| {
        let view = task.view();
        [1, 2, 3].map(|byte| view.version(&key(byte)).is_some())
    };
    let at_54 = replayed(&chain, 54);
    task.committed(&env(&chain, &at_54, &clock), &[2]).unwrap();
    assert_eq!(
        online(&task),
        [false, true, true],
        "round 0 expires at every round start"
    );
    assert_eq!(task.view().len(), 4, "admitted at height 50, kept at 54");
    let at_55 = replayed(&chain, 55);
    task.committed(&env(&chain, &at_55, &clock), &[7]).unwrap();
    assert_eq!(
        online(&task),
        [false, false, true],
        "round 1 expires at round 7"
    );
    assert!(
        task.view().is_empty(),
        "admitted at height 50, five blocks old at 55"
    );
}

#[test]
fn a_change_of_the_online_record_alone_keeps_the_pools_transactions() {
    let chain = Chain::load();
    let (state, clock) = (replayed(&chain, 58), clock(&chain));
    let mut task = task("pool-task-online", 2_700, &state);
    let env = env(&chain, &state, &clock);
    task.process(Channel::Api, &env, entries(&chain, 59))
        .unwrap();
    let before = task.view();
    let key = PublicKeyBytes::from_array([7; 33]);
    let version = "4.3.2".to_owned();
    task.online(OnlineWork::Set {
        key,
        round: 2,
        version,
    });
    let after = task.view_online(&before);
    assert_eq!(after.version(&key), Some("4.3.2"));
    assert_eq!(after.online.len(), before.online.len() + 1);
    let shared = before.by_priority.iter().zip(&after.by_priority);
    assert!(shared.clone().count() == 4 && shared.into_iter().all(|(a, b)| Arc::ptr_eq(a, b)));
    assert_eq!(after, task.view());
    task.online(OnlineWork::Expire { round: 9 });
    assert!(
        task.view().online.is_empty(),
        "round 1 and round 2 expire at round 9"
    );
    // The record goes to the store as it is.
    let dir = scratch("pool-task-online-store");
    let mut store = ChainStore::open(&settings(&dir)).unwrap();
    task.online(OnlineWork::Set {
        key,
        round: 9,
        version: "4.3.3".to_owned(),
    });
    task.save_online(&mut store).unwrap();
    assert_eq!(store.online().unwrap().entries(), task.view().online);
}
