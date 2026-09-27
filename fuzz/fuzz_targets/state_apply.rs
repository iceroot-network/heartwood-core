//! Fuzz the transaction handlers on the state: an arbitrary wallet fixture (balances, nonces,
//! keys, second keys, delegates and their resignations, votes with their vote balances) and a
//! transaction of a kept type from one of its wallets, with arbitrary fields, fee, nonce and
//! second signature, optionally with one byte changed before it is decoded.
//!
//! Neither the checks, apply nor revert may panic. The checks alone and apply give the same
//! verdict. A refusal leaves the wallets as they were. An accepted transaction changes the sum of
//! the balances by its fee and burned amount only, and its revert succeeds and restores every
//! wallet exactly; the indexes stay in step with the wallets throughout.

#![no_main]

use std::sync::LazyLock;

use arbitrary::Arbitrary;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::{Address, Aux, KeyPair, Milestones, Network, Transaction};
use heartwood_state::wallets::vote_balances::{increase_vote_balances, update_vote_shares};
use heartwood_state::wallets::{Resignation, Wallet, WalletRepository};
use heartwood_state::{ApplyContext, TransactionHandlers};
use heartwood_transactions::{Handlers, minimum_fee};
use libfuzzer_sys::fuzz_target;

/// Three active delegates, a fee table, a revoke delay of 106 blocks and a burn minimum of 1,000.
const MILESTONES_TEXT: &str = include_str!("../milestones/state_apply.json");
const NETWORK: u8 = 90;
/// The most wallets a fixture has; two more keys serve as outsiders.
const WALLETS: usize = 12;
/// Each fixture balance is below 2^58, so the sum of all balances stays far below 2^63.
const BALANCE_BITS: u32 = 58;

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

static KEYS: LazyLock<Vec<(KeyPair, Address)>> = LazyLock::new(|| {
    (0..WALLETS + 2)
        .map(|index| {
            let keys = KeyPair::from_passphrase(&format!("fuzz wallet {index}")).expect("keys");
            let address = Address::from_public_key(keys.public_key(), NETWORK);
            (keys, address)
        })
        .collect()
});

#[derive(Arbitrary, Debug)]
struct Input {
    wallets: Vec<WalletSpec>,
    last_block_height: u32,
    transaction: TransactionSpec,
    /// A byte to change in the signed transaction, by index and XOR mask.
    mutation: Option<(u16, u8)>,
}

#[derive(Arbitrary, Debug)]
struct WalletSpec {
    balance: u64,
    nonce: u8,
    second_key: Option<u8>,
    delegate: Option<DelegateSpec>,
    votes: Vec<(u8, u16)>,
}

#[derive(Arbitrary, Debug)]
struct DelegateSpec {
    /// 0 not resigned, 1 temporarily, 2 permanently.
    resigned: u8,
    /// Blocks from the resignation to the last block; `None` for a resignation not found.
    resigned_before: Option<u8>,
}

#[derive(Arbitrary, Debug)]
struct TransactionSpec {
    /// The sender: a fixture wallet, or an outsider past the fixture.
    sender: u8,
    /// Added to the sender's nonce plus one.
    nonce_offset: i8,
    fee: Fee,
    /// 0 no second signature, 1 with the wallet's second key, 2 with another key.
    second_signature: u8,
    asset: AssetSpec,
}

#[derive(Arbitrary, Debug)]
enum Fee {
    Exactly(u32),
    BelowFloor,
    AtFloor,
    AboveFloor(u16),
}

#[derive(Arbitrary, Debug)]
enum AssetSpec {
    Transfer(Vec<(u8, u64)>),
    SecondSignature(u8),
    DelegateRegistration(u8),
    DelegateResignation(u8),
    Burn(u64),
    Vote(Vec<(u8, u16)>),
}

fn username(index: u8) -> String {
    // A few names beyond the fixture's, so that registrations and votes name free names too.
    format!("fuzz_{}", index % 16)
}

/// `weights` as basis points that add up to 10,000, each at least 1.
fn basis_points(weights: &[u16]) -> Vec<u16> {
    let count = weights.len() as u64;
    let weights: Vec<u64> = weights.iter().map(|w| 1 + u64::from(*w % 1_000)).collect();
    let total: u64 = weights.iter().sum();
    let mut points: Vec<u64> = weights
        .iter()
        .map(|w| 1 + (10_000 - count) * w / total)
        .collect();
    let rest = 10_000 - points.iter().sum::<u64>();
    if let Some(first) = points.first_mut() {
        *first += rest;
    }
    points.into_iter().map(|p| p as u16).collect()
}

/// The fixture's wallets, with the vote balances their voters give.
fn fixture(specs: &[WalletSpec], last_block_height: u32) -> WalletRepository {
    let mut wallets = WalletRepository::new(NETWORK);
    let count = specs.len().min(WALLETS);
    for (index, spec) in specs.iter().take(count).enumerate() {
        let (keys, address) = &KEYS[index];
        // A key is bound by a transaction, which sets the nonce; delegates have registered.
        let nonce = u64::from(spec.nonce) + u64::from(spec.delegate.is_some());
        if nonce > 0 {
            wallets.bind_public_key(address, keys.public_key().clone());
        }
        let wallet = wallets.get_or_create(address);
        wallet
            .credit(spec.balance >> (64 - BALANCE_BITS))
            .expect("fits");
        wallet.set_nonce(nonce);
        if let Some(key) = spec.second_key {
            let (second, _) = &KEYS[usize::from(key) % KEYS.len()];
            wallet.set_second_public_key(Some(second.public_key().to_compressed()));
        }
        if let Some(delegate) = &spec.delegate {
            let registered = wallets
                .register_delegate(address, username(index as u8))
                .expect("fixture names are unique");
            registered.resigned = match delegate.resigned % 3 {
                0 => None,
                1 => Some(Resignation::Temporary),
                _ => Some(Resignation::Permanent),
            };
            registered.resigned_at = delegate
                .resigned_before
                .map(|before| last_block_height.saturating_sub(before.into()).max(1));
        }
    }
    for (index, spec) in specs.iter().take(count).enumerate() {
        let names: Vec<String> = spec
            .votes
            .iter()
            .map(|(delegate, _)| username(*delegate))
            .filter(|name| wallets.has_by_username(name))
            .collect();
        let mut votes = Votes::new();
        let weights: Vec<u16> = spec.votes.iter().map(|(_, weight)| *weight).collect();
        for name in names {
            if !votes.contains_key(&name) && votes.len() < 3 {
                votes.insert(name, 0);
            }
        }
        let points = basis_points(&weights[..votes.len()]);
        for (share, points) in votes.values_mut().zip(points) {
            *share = points;
        }
        if votes.is_empty() {
            continue;
        }
        let address = KEYS[index].1;
        wallets.get_or_create(&address).set_votes(&votes);
        update_vote_shares(&mut wallets, &address).expect("a fixture voter");
        increase_vote_balances(&mut wallets, &address, true).expect("fixture vote balances fit");
    }
    wallets
}

/// The transaction of `spec`, built by the sender with a fee of `fee`.
fn build(
    spec: &TransactionSpec,
    wallets: &WalletRepository,
    fee: u64,
    params: &heartwood_crypto::Params,
) -> Option<Transaction> {
    let sender = usize::from(spec.sender) % KEYS.len();
    let (keys, address) = &KEYS[sender];
    let recipient = |index: u8| KEYS[usize::from(index) % KEYS.len()].1;
    let builder = match &spec.asset {
        AssetSpec::Transfer(items) => TransactionBuilder::transfer(
            items
                .iter()
                .take(8)
                .map(|(to, amount)| TransferItem {
                    amount: amount >> (64 - BALANCE_BITS),
                    recipient: recipient(*to),
                })
                .collect(),
        ),
        AssetSpec::SecondSignature(key) => TransactionBuilder::second_signature(
            KEYS[usize::from(*key) % KEYS.len()]
                .0
                .public_key()
                .to_compressed(),
        ),
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
        AssetSpec::Burn(amount) => TransactionBuilder::burn(amount >> (64 - BALANCE_BITS)),
        AssetSpec::Vote(entries) => {
            let names: Vec<String> = entries
                .iter()
                .take(5)
                .map(|(name, _)| username(*name))
                .collect();
            let mut votes = Votes::new();
            for name in names {
                votes.entry(name).or_insert(0);
            }
            let weights: Vec<u16> = entries.iter().map(|(_, weight)| *weight).collect();
            let points = basis_points(&weights[..votes.len()]);
            for (share, points) in votes.values_mut().zip(points) {
                *share = points;
            }
            TransactionBuilder::vote(votes)
        }
    };
    let wallet_nonce = wallets.find_by_address(address).map_or(0, Wallet::nonce);
    let nonce = wallet_nonce
        .saturating_add(1)
        .saturating_add_signed(spec.nonce_offset.clamp(-2, 2).into());
    let builder = builder.nonce(nonce).fee(fee);
    let second = match spec.second_signature % 3 {
        0 => None,
        1 => wallets
            .find_by_address(address)
            .and_then(Wallet::second_public_key)
            .and_then(|second| {
                KEYS.iter()
                    .find(|(keys, _)| keys.public_key().to_compressed() == *second)
            })
            .map(|(keys, _)| keys),
        _ => Some(&KEYS[WALLETS + 1].0),
    };
    match second {
        Some(second) => {
            builder.build_second_signed(keys, Aux::random(), second, Aux::random(), params)
        }
        None => builder.build(keys, Aux::random(), params),
    }
    .ok()
}

/// Every wallet that is not empty, by address.
fn visible(wallets: &WalletRepository) -> Vec<Wallet> {
    let mut shown: Vec<Wallet> = wallets
        .iter()
        .filter(|wallet| **wallet != Wallet::new(*wallet.address()))
        .cloned()
        .collect();
    shown.sort_by_key(|wallet| *wallet.address());
    shown
}

fn total(wallets: &WalletRepository) -> u128 {
    wallets
        .iter()
        .map(|wallet| u128::from(wallet.balance()))
        .sum()
}

/// The indexes hold exactly the wallets' keys and delegate names.
fn check_indexes(wallets: &WalletRepository) {
    for wallet in wallets.iter() {
        if let Some(key) = wallet.public_key() {
            assert_eq!(
                wallets
                    .find_by_public_key(&key.to_compressed())
                    .map(Wallet::address),
                Some(wallet.address())
            );
        }
        if let Some(delegate) = wallet.delegate() {
            assert_eq!(
                wallets
                    .find_by_username(delegate.username())
                    .map(Wallet::address),
                Some(wallet.address())
            );
        }
    }
    for delegate in wallets.delegates() {
        assert!(delegate.is_delegate());
    }
}

fuzz_target!(|input: Input| {
    let last_block_height = input.last_block_height % 1_000_000 + 1;
    let height = last_block_height + 1;
    let params = MILESTONES.at(height);
    let ctx = ApplyContext {
        height,
        params,
        last_block_height,
    };
    let mut wallets = fixture(&input.wallets, last_block_height);
    check_indexes(&wallets);

    let fee = match input.transaction.fee {
        Fee::Exactly(fee) => u64::from(fee),
        Fee::BelowFloor | Fee::AtFloor | Fee::AboveFloor(_) => {
            // The floor depends on the size only, which a fee does not change.
            let Some(probe) = build(&input.transaction, &wallets, 1, params) else {
                return;
            };
            let floor = u64::try_from(minimum_fee(&probe, params).max(1)).unwrap_or(u64::MAX);
            match input.transaction.fee {
                Fee::BelowFloor => floor.saturating_sub(1).max(1),
                Fee::AboveFloor(extra) => floor.saturating_add(extra.into()),
                _ => floor,
            }
        }
    };
    let Some(built) = build(&input.transaction, &wallets, fee, params) else {
        return;
    };
    let transaction = match input.mutation {
        None => built,
        Some((index, mask)) => {
            let mut bytes = built.bytes().to_vec();
            let at = usize::from(index) % bytes.len().max(1);
            if let Some(byte) = bytes.get_mut(at) {
                *byte ^= mask;
            }
            match Transaction::decode(&bytes, params) {
                Ok(transaction) => transaction,
                Err(_) => return,
            }
        }
    };

    let before = wallets.clone();
    let checked = Handlers.throw_if_cannot_be_applied(&ctx, &wallets, &transaction);
    let applied = Handlers.apply(&ctx, &mut wallets, &transaction);
    check_indexes(&wallets);
    match (checked, applied) {
        (Err(checked), Err(applied)) => {
            assert_eq!(checked.class(), applied.class());
            assert_eq!(wallets, before, "a refused transaction changed the wallets");
        }
        // Only a value past the range of u64 can fail a step after the checks, and the
        // fixture's balances stay far below it.
        (Ok(()), Err(applied)) => panic!("apply failed after the checks passed: {applied}"),
        (Err(checked), Ok(_)) => panic!("the checks refused ({checked}) what apply accepted"),
        (Ok(()), Ok(undo)) => {
            let spent = u128::from(transaction.fee()) + u128::from(transaction.amount());
            assert_eq!(total(&wallets) + spent, total(&before));
            Handlers
                .revert(&ctx, &mut wallets, &transaction, &undo)
                .expect("an applied transaction reverts");
            check_indexes(&wallets);
            assert_eq!(
                visible(&wallets),
                visible(&before),
                "the revert did not restore the wallets"
            );
        }
    }
});
