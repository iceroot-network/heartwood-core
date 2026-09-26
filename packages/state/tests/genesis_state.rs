//! The state after the genesis block, for the three devnet chain definitions of the
//! `V10-genesis` vectors: the issued amount, the funded wallets, the generator paid nothing, the
//! delegates with their nonces, votes and ranks, round 1 and its forging list, and the supply.
//! Then the genesis refusals: a block that does not verify, an issuance over `2^63 − 1`, a sender
//! that cannot pay, and donations at height 1.

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::blocks::{VerifyFailure, serialiser};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::TransferItem;
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Aux, Block, KeyPair, Milestones, Network, Transaction};
use heartwood_state::State;
use heartwood_state::digest::{Step, height_digest, records};
use heartwood_state::errors::{GenesisError, RoundError};
use heartwood_state::genesis::{self, MAX_ISSUANCE};
use heartwood_state::supply::check_supply;
use heartwood_transactions::Handlers;
use serde_json::Value;

const V10: &str = include_str!("../../crypto/tests/vectors/V10-genesis.jsonl");

/// The round-1 order of 53 delegates: position `k` of the forging list holds the delegate of
/// rank `ROUND_1[k] + 1`.
const ROUND_1: [usize; 53] = [
    8, 15, 41, 21, 36, 47, 40, 50, 38, 10, 35, 16, 20, 31, 4, 48, 1, 18, 34, 0, 25, 46, 42, 9, 3,
    22, 17, 13, 39, 28, 45, 33, 30, 6, 44, 7, 26, 51, 24, 43, 19, 37, 12, 29, 32, 49, 14, 2, 23, 5,
    11, 52, 27,
];

/// One chain definition of `V10-genesis`: its record output, milestones and genesis block.
struct Definition {
    output: Value,
    network: Network,
    milestones: Milestones,
    genesis: Block,
}

fn definitions() -> Vec<Definition> {
    V10.lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["op"] == "genesis.generate")
        .map(|record| {
            let output = record["output"].clone();
            let file = |name: &str| output["files"][name].as_str().unwrap().to_owned();
            let network = Network::load(&file("crypto/network.json")).unwrap();
            let milestones = Milestones::load(&file("crypto/milestones.json"), &network).unwrap();
            let json: Value = serde_json::from_str(&file("crypto/genesisBlock.json")).unwrap();
            let genesis = factory::from_json(&json, &milestones).unwrap();
            Definition {
                output,
                network,
                milestones,
                genesis,
            }
        })
        .collect()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap()
}

#[test]
fn the_state_after_each_devnet_genesis_block() {
    let definitions = definitions();
    assert_eq!(definitions.len(), 3);
    for definition in &definitions {
        let output = &definition.output;
        let network = definition.network.pub_key_hash;
        let state =
            State::from_genesis(&definition.genesis, &definition.milestones, &Handlers).unwrap();
        let wallets = state.wallets();
        let issued: u64 = text(output, "issued").parse().unwrap();
        assert_eq!(state.totals().issued, issued);
        assert_eq!(state.totals().rewards, 0);
        assert_eq!(state.height(), 1);
        assert_eq!(state.last_block(), &definition.genesis);
        assert!(state.journal().is_empty());
        check_supply(&state).unwrap();

        // The generator sent the issuance record: its key is bound and it paid nothing.
        let generator = &output["generator"];
        let wallet = wallets
            .find_by_address(
                &Address::from_base58_for(text(generator, "address"), network).unwrap(),
            )
            .unwrap();
        assert_eq!(wallet.balance(), 0);
        assert_eq!(wallet.nonce(), 1);
        assert_eq!(
            wallet.public_key().unwrap().to_hex(),
            text(generator, "publicKey")
        );
        for funded in output["genesisWallets"].as_array().unwrap() {
            let address = Address::from_base58_for(text(funded, "address"), network).unwrap();
            let wallet = wallets.find_by_address(&address).unwrap();
            assert_eq!(wallet.balance().to_string(), text(funded, "genesisBalance"));
            assert_eq!(wallet.nonce(), 0);
            assert!(wallet.public_key().is_none());
        }

        // Every delegate registered itself and voted for itself with no balance: nonce 2, one
        // voter, vote balance 0; ranked by public key.
        let delegates = output["delegates"].as_array().unwrap();
        let mut by_key: Vec<(&str, &str)> = delegates
            .iter()
            .map(|d| (text(d, "publicKey"), text(d, "username")))
            .collect();
        by_key.sort();
        for (index, (public_key, username)) in by_key.iter().enumerate() {
            let wallet = wallets.find_by_username(username).unwrap();
            assert_eq!(wallet.public_key().unwrap().to_hex(), *public_key);
            assert_eq!(wallet.nonce(), 2, "{username}");
            assert_eq!(wallet.balance(), 0);
            assert_eq!(wallet.votes().get(*username), Some(&10_000));
            let delegate = wallet.delegate().unwrap();
            assert_eq!(delegate.vote_balance, 0);
            assert_eq!(delegate.voters, 1);
            assert_eq!(delegate.rank, Some(u32::try_from(index + 1).unwrap()));
            assert_eq!(delegate.resigned, None);
            assert_eq!(delegate.produced_blocks, 0);
            assert_eq!(delegate.last_block, None);
        }

        // Round 1: every delegate, in rank order; the forging list is their shuffle for round 1;
        // no block of the round yet.
        let round = state.round();
        assert_eq!(round.info().round, 1);
        let ranked: Vec<&str> = round
            .round_delegates()
            .iter()
            .map(|d| d.username.as_str())
            .collect();
        let expected: Vec<&str> = by_key.iter().map(|(_, username)| *username).collect();
        assert_eq!(ranked, expected);
        let forging: Vec<&str> = state
            .forging_delegates()
            .iter()
            .map(|d| d.username.as_str())
            .collect();
        let shuffled: Vec<&str> = ROUND_1.iter().map(|&rank| expected[rank]).collect();
        assert_eq!(forging, shuffled);
        assert!(round.blocks().is_empty());

        // The genesis line is a function of the state alone.
        let line = height_digest(
            &state,
            &definition.milestones,
            Step::Genesis,
            &[],
            None,
            &[0; 32],
        );
        assert_eq!(line.height, 1);
        assert_eq!(&line.id, definition.genesis.id());
        let sections = records(&state, &definition.milestones, &[]);
        assert_eq!(line.sections, sections.hashes());
        assert!(sections.round.contains("\"active\":"));
        assert!(sections.round.contains("\"prod\":[]"));
    }
}

/// The first chain definition's generator keys, from its passphrase.
fn generator_keys(definition: &Definition) -> KeyPair {
    KeyPair::from_passphrase(text(&definition.output["generator"], "passphrase")).unwrap()
}

/// A genesis block with `transactions`, signed by `keys`, under `milestones`.
fn genesis_block(transactions: Vec<Transaction>, keys: &KeyPair, milestones: &Milestones) -> Block {
    let count = u32::try_from(transactions.len()).unwrap();
    let draft = BlockDraft {
        version: 0,
        timestamp: 0,
        height: 1,
        previous_block: [0; 32],
        number_of_transactions: count,
        total_amount: transactions.iter().map(Transaction::amount).sum(),
        total_fee: transactions.iter().map(Transaction::fee).sum(),
        reward: 0,
        payload_length: 32 * count,
        payload_hash: sha256_concat(transactions.iter().map(|tx| tx.id().as_bytes().as_slice())),
    };
    factory::make(draft, transactions, keys, Aux::fixed([7; 32]), milestones).unwrap()
}

/// A genesis transfer of `amounts` from `keys` to fresh addresses, with `nonce` and `fee`.
fn transfer(
    keys: &KeyPair,
    amounts: &[u64],
    nonce: u64,
    fee: u64,
    milestones: &Milestones,
) -> Transaction {
    let params = milestones.at(1);
    let items = amounts
        .iter()
        .enumerate()
        .map(|(i, &amount)| TransferItem {
            amount,
            recipient: Address::from_public_key(
                &PublicKey::from_secret_key(
                    KeyPair::from_passphrase(&format!("recipient {i}"))
                        .unwrap()
                        .secret_key(),
                ),
                params.network(),
            ),
        })
        .collect();
    TransactionBuilder::transfer(items)
        .nonce(nonce)
        .fee(fee)
        .build_with(keys, Aux::fixed([9; 32]), params, Mode::Genesis)
        .unwrap()
}

#[test]
fn genesis_refusals() {
    let definitions = definitions();
    let definition = &definitions[0];
    let milestones = &definition.milestones;
    let generator = generator_keys(definition);

    // A block whose header was changed after signing does not verify.
    let mut bytes = serialiser::serialise(&definition.genesis);
    bytes[4] ^= 0xff; // the low byte of the timestamp: the signature no longer verifies
    let forged = Block::decode(&bytes, milestones).unwrap();
    assert!(matches!(
        State::from_genesis(&forged, milestones, &Handlers),
        Err(GenesisError::Invalid { failures }) if failures == [VerifyFailure::Signature]
    ));

    // Without delegates there is no round 1, and the reference does not start either.
    let issuance = transfer(&generator, &[1_000], 1, 0, milestones);
    let block = genesis_block(vec![issuance.clone()], &generator, milestones);
    assert_eq!(
        State::from_genesis(&block, milestones, &Handlers).unwrap_err(),
        GenesisError::Round(RoundError::NotEnoughDelegates {
            required: 53,
            ranked: 0
        })
    );

    // At most 2^63 − 1 issued, in one record or several.
    let most = transfer(&generator, &[MAX_ISSUANCE], 1, 0, milestones);
    assert_eq!(
        genesis::issued(&genesis_block(vec![most], &generator, milestones)),
        Ok(MAX_ISSUANCE)
    );
    let over = transfer(&generator, &[MAX_ISSUANCE, 1], 1, 0, milestones);
    let block = genesis_block(vec![over], &generator, milestones);
    assert_eq!(genesis::issued(&block), Err(GenesisError::Issuance));
    assert_eq!(
        State::from_genesis(&block, milestones, &Handlers).unwrap_err(),
        GenesisError::Issuance
    );

    // The generator pays the fee of its issuance record; a sender that received nothing cannot
    // pay its transfer.
    let with_fee = transfer(&generator, &[1_000], 1, 1, milestones);
    let block = genesis_block(vec![with_fee], &generator, milestones);
    let address = Address::from_public_key(generator.public_key(), milestones.at(1).network());
    assert_eq!(
        State::from_genesis(&block, milestones, &Handlers).unwrap_err(),
        GenesisError::NegativeBalance { address }
    );
    let other = KeyPair::from_passphrase("genesis test sender").unwrap();
    let block = genesis_block(
        vec![issuance, transfer(&other, &[1], 1, 0, milestones)],
        &generator,
        milestones,
    );
    let address = Address::from_public_key(other.public_key(), milestones.at(1).network());
    assert_eq!(
        State::from_genesis(&block, milestones, &Handlers).unwrap_err(),
        GenesisError::NegativeBalance { address }
    );
}

#[test]
fn donations_at_height_one_refuse_the_genesis_block() {
    let definitions = definitions();
    let definition = &definitions[0];
    // The devnet pays donations from height 2; move them to height 1.
    let text = definition.output["files"]["crypto/milestones.json"]
        .as_str()
        .unwrap();
    let mut milestones: Vec<Value> = serde_json::from_str(text).unwrap();
    let donations = milestones
        .iter()
        .filter_map(|milestone| milestone.get("donations"))
        .find(|donations| donations.as_object().is_some_and(|d| !d.is_empty()))
        .unwrap()
        .clone();
    milestones[0]["donations"] = donations;
    let milestones = Milestones::load(
        &serde_json::to_string(&milestones).unwrap(),
        &definition.network,
    )
    .unwrap();
    let generator = generator_keys(definition);
    let block = genesis_block(
        vec![transfer(&generator, &[1_000], 1, 0, &milestones)],
        &generator,
        &milestones,
    );
    assert_eq!(
        State::from_genesis(&block, &milestones, &Handlers).unwrap_err(),
        GenesisError::Donations
    );
}
