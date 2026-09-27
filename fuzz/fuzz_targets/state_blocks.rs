//! Fuzz the state machine on whole blocks: a small chain (four delegates, three of them active,
//! and funded wallets) takes an arbitrary sequence of blocks and reverts. Each block carries up
//! to four transactions of the kept types from the chain's wallets, with arbitrary nonces, fees
//! and assets; it is forged in a slot a few slots after the last block, by the scheduled delegate
//! or by another key, with the reward the state gives it or another one.
//!
//! Neither apply nor revert may panic. A refused block leaves every record of the state as it
//! was; an accepted block keeps the supply check; a revert gives back the records from before the
//! block it takes back, and a failed rollback is never reported.

#![no_main]

use std::sync::LazyLock;

use arbitrary::Arbitrary;
use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Address, Aux, Block, KeyPair, Milestones, Network, Transaction};
use heartwood_state::State;
use heartwood_state::digest::{Sections, records};
use heartwood_state::errors::ApplyError;
use heartwood_state::supply::check_supply;
use heartwood_transactions::Handlers;
use libfuzzer_sys::fuzz_target;

/// Three active delegates, 8-second slots, a reward of 5 and a fee table.
const MILESTONES_TEXT: &str = include_str!("../milestones/state_blocks.json");
const NETWORK: u8 = 90;
/// Keys 0 to 3 are the genesis delegates; keys 4 to 7 are funded wallets; key 8 is the generator.
const KEYS: usize = 9;
const GENERATOR: usize = 8;
const DELEGATES: usize = 4;
/// What the genesis block gives each key but the generator.
const FUNDS: u64 = 1_000_000_000_000;

static MILESTONES: LazyLock<Milestones> = LazyLock::new(|| {
    let network = Network {
        name: "devnet".into(),
        message_prefix: "devnet message:\n".into(),
        address_character: Some("d".into()),
        bip32_public: 70_617_039,
        bip32_private: 70_615_956,
        pub_key_hash: NETWORK,
        nethash: "0".repeat(64),
        wif: 252,
        slip44: 1,
        token: "dROOT".into(),
        symbol: "dRT".into(),
        explorer: String::new(),
    };
    Milestones::load(MILESTONES_TEXT, &network).expect("the fuzz milestones load")
});

static KEY_PAIRS: LazyLock<Vec<(KeyPair, Address)>> = LazyLock::new(|| {
    (0..KEYS)
        .map(|index| {
            let keys = KeyPair::from_passphrase(&format!("fuzz blocks {index}")).expect("keys");
            let address = Address::from_public_key(keys.public_key(), NETWORK);
            (keys, address)
        })
        .collect()
});

fn username(index: u8) -> String {
    format!("fuzzb_{}", index % 6)
}

/// The chain's genesis block: the generator funds every other key, and the first four register
/// and vote for themselves.
static GENESIS: LazyLock<Block> = LazyLock::new(|| {
    let params = MILESTONES.at(1);
    let (generator, _) = &KEY_PAIRS[GENERATOR];
    let genesis = |builder: TransactionBuilder, keys: &KeyPair| {
        builder
            .fee(0)
            .build_with(keys, Aux::fixed([1; 32]), params, Mode::Genesis)
            .expect("a genesis transaction")
    };
    let items = KEY_PAIRS[..GENERATOR]
        .iter()
        .map(|(_, address)| TransferItem {
            amount: FUNDS,
            recipient: *address,
        })
        .collect();
    let mut transactions = vec![genesis(
        TransactionBuilder::transfer(items).nonce(1),
        generator,
    )];
    for index in 0..DELEGATES {
        let (keys, _) = &KEY_PAIRS[index];
        let name = username(index as u8);
        transactions.push(genesis(
            TransactionBuilder::delegate_registration(&name).nonce(1),
            keys,
        ));
        let mut votes = Votes::new();
        votes.insert(name, 10_000);
        transactions.push(genesis(TransactionBuilder::vote(votes).nonce(2), keys));
    }
    let count = transactions.len() as u32;
    let draft = BlockDraft {
        version: 0,
        timestamp: 0,
        height: 1,
        previous_block: [0; 32],
        number_of_transactions: count,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 32 * count,
        payload_hash: sha256_concat(transactions.iter().map(|tx| tx.id().as_bytes().as_slice())),
    };
    factory::make(
        draft,
        transactions,
        generator,
        Aux::fixed([2; 32]),
        &MILESTONES,
    )
    .expect("the genesis block")
});

#[derive(Arbitrary, Debug)]
enum Step {
    Block(BlockSpec),
    Revert,
}

#[derive(Arbitrary, Debug)]
struct BlockSpec {
    /// Slots skipped after the last block (at most 7).
    gap: u8,
    /// A key that signs instead of the scheduled delegate.
    signer: Option<u8>,
    /// Added to the reward the state gives the block.
    reward_offset: i8,
    transactions: Vec<TransactionSpec>,
}

#[derive(Arbitrary, Debug)]
struct TransactionSpec {
    sender: u8,
    /// Added to the sender's nonce plus one.
    nonce_offset: i8,
    fee: u32,
    asset: AssetSpec,
}

#[derive(Arbitrary, Debug)]
enum AssetSpec {
    Transfer(u8, u32),
    SecondSignature(u8),
    DelegateRegistration(u8),
    DelegateResignation(u8),
    Burn(u32),
    Vote(Vec<(u8, u16)>),
}

fn key(index: u8) -> &'static (KeyPair, Address) {
    &KEY_PAIRS[usize::from(index) % KEYS]
}

/// The transaction of `spec` at the next height of `state`, or `None` when it cannot be built.
fn transaction(state: &State, spec: &TransactionSpec) -> Option<Transaction> {
    let params = MILESTONES.at(state.height() + 1);
    let (keys, address) = key(spec.sender);
    let nonce = state
        .wallets()
        .find_by_address(address)
        .map_or(0, |wallet| wallet.nonce());
    let nonce = nonce
        .checked_add(1)?
        .checked_add_signed(i64::from(spec.nonce_offset % 3))?;
    let builder = match &spec.asset {
        AssetSpec::Transfer(to, amount) => TransactionBuilder::transfer(vec![TransferItem {
            amount: u64::from(*amount),
            recipient: key(*to).1,
        }]),
        AssetSpec::SecondSignature(other) => {
            TransactionBuilder::second_signature(key(*other).0.public_key().to_compressed())
        }
        AssetSpec::DelegateRegistration(name) => {
            TransactionBuilder::delegate_registration(&username(*name))
        }
        AssetSpec::DelegateResignation(kind) => {
            TransactionBuilder::delegate_resignation(match kind % 4 {
                0 => None,
                1 => Some(ResignationType::TemporaryResign),
                2 => Some(ResignationType::PermanentResign),
                _ => Some(ResignationType::NotResigned),
            })
        }
        AssetSpec::Burn(amount) => TransactionBuilder::burn(u64::from(*amount)),
        AssetSpec::Vote(entries) => {
            let mut votes = Votes::new();
            let mut left: u16 = 10_000;
            for (index, (name, share)) in entries.iter().take(3).enumerate() {
                let last = index + 1 == entries.len().min(3);
                let share = if last {
                    left
                } else {
                    (share % left.max(1)).max(1)
                };
                if share == 0 || votes.contains_key(&username(*name)) {
                    continue;
                }
                votes.insert(username(*name), share);
                left = left.saturating_sub(share);
            }
            TransactionBuilder::vote(votes)
        }
    };
    builder
        .nonce(nonce)
        .fee(u64::from(spec.fee))
        .build(keys, Aux::fixed([3; 32]), params)
        .ok()
}

/// The block of `spec` after the last block of `state`, or `None` when it cannot be made.
fn block(state: &State, spec: &BlockSpec) -> Option<Block> {
    let transactions: Vec<Transaction> = spec
        .transactions
        .iter()
        .take(4)
        .filter_map(|tx| transaction(state, tx))
        .collect();
    let last = state.last_block().header();
    let height = last.height + 1;
    let block_time = MILESTONES.at(1).block_time() as u32;
    let timestamp = last.timestamp + block_time * (1 + u32::from(spec.gap % 8));
    let forging = state.forging_delegates();
    let scheduled = forging.get((timestamp / block_time) as usize % forging.len().max(1))?;
    let signer = match spec.signer {
        Some(index) => &key(index).0,
        None => {
            &KEY_PAIRS
                .iter()
                .find(|(keys, _)| keys.public_key().to_compressed() == scheduled.public_key)?
                .0
        }
    };
    let reward = state
        .reward_for_block_in_round(height, &signer.public_key().to_compressed(), &MILESTONES)
        .map_or(0, |reward| reward.reward)
        .saturating_add_signed(i64::from(spec.reward_offset % 2));
    let count = transactions.len() as u32;
    let draft = BlockDraft {
        version: 0,
        timestamp,
        height,
        previous_block: *state.last_block().id().as_bytes(),
        number_of_transactions: count,
        total_amount: transactions.iter().map(Transaction::amount).sum(),
        total_fee: transactions.iter().map(Transaction::fee).sum(),
        reward,
        payload_length: 32 * count,
        payload_hash: sha256_concat(transactions.iter().map(|tx| tx.id().as_bytes().as_slice())),
    };
    factory::make(
        draft,
        transactions,
        signer,
        Aux::fixed([4; 32]),
        &MILESTONES,
    )
    .ok()
}

fn snapshot(state: &State) -> Sections {
    records(state, &MILESTONES, &[])
}

fuzz_target!(|steps: Vec<Step>| {
    let mut state =
        State::from_genesis(&GENESIS, &MILESTONES, &Handlers).expect("the genesis block starts");
    let mut before: Vec<Sections> = Vec::new();
    for step in steps.iter().take(24) {
        match step {
            Step::Block(spec) => {
                let Some(block) = block(&state, spec) else {
                    continue;
                };
                let previous = snapshot(&state);
                let totals = *state.totals();
                match state.apply_block(&block, &MILESTONES, &Handlers) {
                    Ok(_) => {
                        check_supply(&state).expect("the supply holds after a block");
                        before.push(previous);
                    }
                    Err(ApplyError::RollbackFailed(error)) => panic!("{error}"),
                    Err(_) => {
                        assert_eq!(
                            snapshot(&state),
                            previous,
                            "a refused block changed the state"
                        );
                        assert_eq!(state.totals(), &totals);
                    }
                }
            }
            Step::Revert => {
                let Some(previous) = before.pop() else {
                    continue;
                };
                state
                    .revert_block(&MILESTONES, &Handlers)
                    .expect("the last block reverts");
                check_supply(&state).expect("the supply holds after a revert");
                assert_eq!(
                    snapshot(&state),
                    previous,
                    "the revert did not give back the state"
                );
            }
        }
    }
});
