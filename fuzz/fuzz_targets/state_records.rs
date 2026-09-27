//! Fuzz the state's records: arbitrary text into the record decoders and into
//! `State::restore`, which may refuse it only with a typed error; and a state encoded as records
//! decodes to an equal state.
//!
//! A fixed chain of ten blocks (four delegates, three of them active, so a round starts every
//! three blocks; transfers, votes, a registration, a resignation, a second key and a burn) gives
//! a state at each height. An input picks a height, checks that the state's records, written as
//! JSON text and read back, restore the same state, then changes one record's text with byte
//! edits (or replaces it with arbitrary text) and restores again: that may succeed or refuse,
//! never panic, and a restored state keeps the supply check. The input's own bytes are also read
//! as each kind of record.

#![no_main]

use std::sync::LazyLock;

use arbitrary::Arbitrary;
use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Address, Aux, Block, KeyPair, Milestones, Network, Transaction};
use heartwood_state::digest::records;
use heartwood_state::persist::{RestoreError, StateRecord, UndoRecord, WalletRecord};
use heartwood_state::supply::check_supply;
use heartwood_state::{RoundState, State, Wallet, WalletRepository};
use heartwood_transactions::{Handlers, minimum_fee};
use libfuzzer_sys::fuzz_target;

const MILESTONES_TEXT: &str = include_str!("../milestones/state_blocks.json");
const NETWORK: u8 = 90;
/// Keys 0 to 3 are the genesis delegates; keys 4 to 7 are funded wallets; key 8 is the generator.
const KEYS: usize = 9;
const GENERATOR: usize = 8;
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
            let keys = KeyPair::from_passphrase(&format!("fuzz records {index}")).expect("keys");
            let address = Address::from_public_key(keys.public_key(), NETWORK);
            (keys, address)
        })
        .collect()
});

fn username(index: usize) -> String {
    format!("fuzzr_{index}")
}

/// The genesis block: the generator funds every other key, and the first four register and vote
/// for themselves.
fn genesis() -> Block {
    let params = MILESTONES.at(1);
    let (generator, _) = &KEY_PAIRS[GENERATOR];
    let signed = |builder: TransactionBuilder, keys: &KeyPair| {
        builder
            .fee(0)
            .build_with(keys, Aux::fixed([1; 32]), params, Mode::Genesis)
            .expect("a genesis transaction")
    };
    let items = (KEY_PAIRS[..GENERATOR].iter())
        .map(|(_, address)| TransferItem {
            amount: FUNDS,
            recipient: *address,
        })
        .collect();
    let mut transactions = vec![signed(
        TransactionBuilder::transfer(items).nonce(1),
        generator,
    )];
    for index in 0..4 {
        let (keys, _) = &KEY_PAIRS[index];
        transactions.push(signed(
            TransactionBuilder::delegate_registration(&username(index)).nonce(1),
            keys,
        ));
        let votes: Votes = [(username(index), 10_000)].into_iter().collect();
        transactions.push(signed(TransactionBuilder::vote(votes).nonce(2), keys));
    }
    forge(transactions, None, generator)
}

/// A block with `transactions` after `last` (none for the genesis block), signed by `keys`.
fn forge(transactions: Vec<Transaction>, last: Option<(&State, u32)>, keys: &KeyPair) -> Block {
    let count = transactions.len() as u32;
    let (height, timestamp, previous, reward) = match last {
        None => (1, 0, [0; 32], 0),
        Some((state, timestamp)) => {
            let generator = keys.public_key().to_compressed();
            let height = state.height() + 1;
            let reward = (state.reward_for_block_in_round(height, &generator, &MILESTONES))
                .expect("the scheduled delegate has a reward")
                .reward;
            (
                height,
                timestamp,
                *state.last_block().id().as_bytes(),
                reward,
            )
        }
    };
    let draft = BlockDraft {
        version: 0,
        timestamp,
        height,
        previous_block: previous,
        number_of_transactions: count,
        total_amount: transactions.iter().map(Transaction::amount).sum(),
        total_fee: transactions.iter().map(Transaction::fee).sum(),
        reward,
        payload_length: 32 * count,
        payload_hash: sha256_concat(transactions.iter().map(|tx| tx.id().as_bytes().as_slice())),
    };
    factory::make(draft, transactions, keys, Aux::fixed([2; 32]), &MILESTONES).expect("a block")
}

/// The transaction of the fixed chain at `height`, from the state before it.
fn transaction(state: &State, height: u32) -> Transaction {
    let (sender, builder) = match height {
        2 => (4, TransactionBuilder::transfer(vec![item(5, 1_000)])),
        3 => (
            4,
            TransactionBuilder::vote([(username(0), 10_000)].into_iter().collect()),
        ),
        4 => (5, TransactionBuilder::delegate_registration(&username(5))),
        5 => {
            let votes = [(username(1), 5_000), (username(2), 5_000)]
                .into_iter()
                .collect();
            (6, TransactionBuilder::vote(votes))
        }
        6 => (
            3,
            TransactionBuilder::delegate_resignation(Some(ResignationType::TemporaryResign)),
        ),
        7 => (
            7,
            TransactionBuilder::second_signature(KEY_PAIRS[8].0.public_key().to_compressed()),
        ),
        8 => (4, TransactionBuilder::burn(2_000)),
        9 => (5, TransactionBuilder::transfer(vec![item(4, 7)])),
        _ => (
            4,
            TransactionBuilder::vote([(username(1), 10_000)].into_iter().collect()),
        ),
    };
    let (keys, address) = &KEY_PAIRS[sender];
    let nonce = state
        .wallets()
        .find_by_address(address)
        .map_or(0, Wallet::nonce)
        + 1;
    let params = MILESTONES.at(height);
    let draft = builder
        .clone()
        .nonce(nonce)
        .build(keys, Aux::fixed([3; 32]), params)
        .expect("tx");
    let fee = u64::try_from(minimum_fee(&draft, params)).expect("a fee");
    builder
        .nonce(nonce)
        .fee(fee)
        .build(keys, Aux::fixed([3; 32]), params)
        .expect("a transaction")
}

fn item(to: usize, amount: u64) -> TransferItem {
    TransferItem {
        amount,
        recipient: KEY_PAIRS[to].1,
    }
}

/// The states of the fixed chain, at heights 1 to 10.
static CHAIN: LazyLock<Vec<State>> = LazyLock::new(|| {
    let mut state = State::from_genesis(&genesis(), &MILESTONES, &Handlers).expect("genesis");
    let mut states = vec![state.clone()];
    for height in 2..=10 {
        let block_time = MILESTONES.at(1).block_time() as u32;
        let timestamp = state.last_block().header().timestamp + block_time;
        let forging = state.forging_delegates();
        let scheduled = &forging[(timestamp / block_time) as usize % forging.len()];
        let keys = &(KEY_PAIRS.iter())
            .find(|(keys, _)| keys.public_key().to_compressed() == scheduled.public_key)
            .expect("the scheduled delegate's key")
            .0;
        let block = forge(
            vec![transaction(&state, height)],
            Some((&state, timestamp)),
            keys,
        );
        state
            .apply_block(&block, &MILESTONES, &Handlers)
            .expect("the fixed chain applies");
        states.push(state.clone());
    }
    states
});

/// The records of `state` as JSON text: every wallet that is not empty, the state record, and
/// every undo record.
fn texts(state: &State) -> (Vec<Vec<u8>>, Vec<u8>, Vec<Vec<u8>>) {
    let json = |value: serde_json::Result<Vec<u8>>| value.expect("a record writes");
    let wallets = (state.wallets().iter())
        .filter(|wallet| **wallet != Wallet::new(*wallet.address()))
        .map(|wallet| json(serde_json::to_vec(&wallet.to_record())))
        .collect();
    let undo = (state.journal().iter())
        .map(|undo| json(serde_json::to_vec(&UndoRecord::from_undo(undo))))
        .collect();
    (
        wallets,
        json(serde_json::to_vec(&state.state_record())),
        undo,
    )
}

/// A record error for text that is not a record.
fn unreadable(error: serde_json::Error) -> RestoreError {
    RestoreError::Record {
        what: "json",
        detail: error.to_string(),
    }
}

/// The block of the fixed chain at `height`.
fn block_at(height: u32) -> Option<Block> {
    let index = usize::try_from(height.checked_sub(1)?).ok()?;
    CHAIN.get(index).map(|state| state.last_block().clone())
}

/// The state that the record texts describe, as the store's start reads it.
fn restore(wallets: &[Vec<u8>], record: &[u8], undo: &[Vec<u8>]) -> Result<State, RestoreError> {
    let record: StateRecord = serde_json::from_slice(record).map_err(unreadable)?;
    let wallets = (wallets.iter())
        .map(|text| {
            let record: WalletRecord = serde_json::from_slice(text).map_err(unreadable)?;
            Wallet::from_record(record, NETWORK)
        })
        .collect::<Result<Vec<Wallet>, RestoreError>>()?;
    let wallets = WalletRepository::from_wallets(NETWORK, wallets)?;
    let journal = (undo.iter())
        .map(|text| {
            let record: UndoRecord = serde_json::from_slice(text).map_err(unreadable)?;
            let block = block_at(record.h).ok_or(RestoreError::Journal { expected: record.h })?;
            record.into_undo(block, NETWORK)
        })
        .collect::<Result<Vec<_>, RestoreError>>()?;
    let last = journal
        .last()
        .map_or_else(|| CHAIN[0].last_block().clone(), |u| u.block.clone());
    let genesis = CHAIN[0].genesis_block().clone();
    State::restore(genesis, last, wallets, record, journal, &MILESTONES)
}

#[derive(Arbitrary, Debug)]
enum Target {
    Wallet(u8),
    State,
    Undo(u8),
}

#[derive(Arbitrary, Debug)]
struct Edit {
    at: u16,
    remove: u8,
    insert: Vec<u8>,
}

#[derive(Arbitrary, Debug)]
struct Input {
    height: u8,
    target: Target,
    edits: Vec<Edit>,
    replace: Option<Vec<u8>>,
    raw: Vec<u8>,
}

fuzz_target!(|input: Input| {
    let state = &CHAIN[usize::from(input.height) % CHAIN.len()];
    let (mut wallets, mut record, mut undo) = texts(state);

    // The records as written restore the same state.
    let restored = restore(&wallets, &record, &undo).expect("the records restore");
    let sections = |state: &State| records(state, &MILESTONES, &[]);
    assert_eq!(sections(&restored), sections(state));
    assert_eq!(restored.journal(), state.journal());
    assert_eq!(restored.round(), state.round());

    // A changed record restores or is refused.
    let text = match input.target {
        Target::Wallet(index) if !wallets.is_empty() => {
            let count = wallets.len();
            &mut wallets[usize::from(index) % count]
        }
        Target::Undo(index) if !undo.is_empty() => {
            let count = undo.len();
            &mut undo[usize::from(index) % count]
        }
        _ => &mut record,
    };
    if let Some(replace) = input.replace {
        *text = replace;
    }
    for edit in input.edits.iter().take(8) {
        let at = usize::from(edit.at) % (text.len() + 1);
        let end = (at + usize::from(edit.remove % 8)).min(text.len());
        text.splice(at..end, edit.insert.iter().copied().take(16));
    }
    if let Ok(state) = restore(&wallets, &record, &undo) {
        check_supply(&state).expect("a restored state keeps the supply check");
    }

    // The input's own bytes as each kind of record.
    if let Ok(record) = serde_json::from_slice::<WalletRecord>(&input.raw) {
        let _ = Wallet::from_record(record, NETWORK);
    }
    if let Ok(record) = serde_json::from_slice::<StateRecord>(&input.raw) {
        let _ = record.totals.to_totals();
        let _ = RoundState::from_record(record.round);
    }
    if let Ok(record) = serde_json::from_slice::<UndoRecord>(&input.raw)
        && let Some(block) = block_at(record.h)
    {
        let _ = record.into_undo(block, NETWORK);
    }
});
