//! A delegate's block: the draft's fields, the refusals, and vector class V29's block assembly
//! records rebuilt byte for byte.
//!
//! The class is read from `tests/vectors` of the crate, or from the directory in `FORGER_VECTORS`
//! (a relative path is taken from the workspace root), such as a local oracle run in
//! `target/vectors/local`. A class that is not installed is reported and the test passes. The
//! devnet's network and milestones are the committed chain definitions of `heartwood-crypto`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use heartwood_crypto::crypto::hash::{sha256, sha256_concat};
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockId, KeyPair, Milestones, Network, SecretKey, Transaction};
use heartwood_forger::{Delegate, ForgeError, ForgeInputs};
use serde_json::Value;

const CLASS: &str = "V29-forging";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The records of a vector file, its meta record first.
fn lines(path: &Path) -> Option<Vec<Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

/// The class's meta record and records, or `None` when it is not installed.
fn class() -> Option<(Value, Vec<Value>)> {
    let dir = std::env::var_os("FORGER_VECTORS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors"),
        |dir| workspace().join(dir),
    );
    let mut records = lines(&dir.join(format!("{CLASS}.jsonl")))?;
    let meta = records.remove(0);
    assert_eq!(meta["class"], CLASS);
    Some((meta, records))
}

/// The devnet whose milestone file has the SHA-256 `digest`, from the committed chain definitions.
fn devnet(digest: &str) -> (Network, Milestones) {
    let file = workspace().join("packages/crypto/tests/vectors/V10-genesis.jsonl");
    let records = lines(&file).unwrap();
    let files = (records.iter().skip(1))
        .map(|record| &record["output"]["files"])
        .find(|files| {
            let text = files["crypto/milestones.json"].as_str().unwrap();
            hex::encode(&sha256(text.as_bytes())) == digest
        })
        .expect("the devnet's chain definition");
    let network = Network::load(files["crypto/network.json"].as_str().unwrap()).unwrap();
    let milestones = Milestones::load(files["crypto/milestones.json"].as_str().unwrap(), &network);
    (network, milestones.unwrap())
}

/// The devnet of the committed vectors.
fn committed_devnet() -> (Network, Milestones) {
    let file = workspace().join("packages/crypto/tests/vectors/V02-transactions.jsonl");
    let meta = lines(&file).unwrap().remove(0);
    devnet(meta["network"]["milestonesSha256"].as_str().unwrap())
}

fn delegate(passphrase: &str) -> Delegate {
    Delegate::new(Arc::new(KeyPair::from_passphrase(passphrase).unwrap()), 90)
}

fn inputs(transactions: &[Transaction]) -> ForgeInputs<'_> {
    ForgeInputs {
        transactions,
        previous_id: BlockId::from_array([0xab; 32]),
        previous_height: 5,
        timestamp: 48,
        reward: 180_000_000,
    }
}

#[test]
fn v29_blocks_are_the_references() {
    let Some((meta, records)) = class() else {
        eprintln!("{CLASS} is not installed: set FORGER_VECTORS to a local oracle run");
        return;
    };
    let (network, devnet) = devnet(meta["network"]["milestonesSha256"].as_str().unwrap());
    let mut compared = 0;
    for record in records.iter().filter(|r| r["op"] == "forger.forge") {
        let (input, name) = (&record["input"], record["name"].as_str().unwrap());
        let milestones = match input["milestones"].as_str() {
            Some(text) => Milestones::load(text, &network).unwrap(),
            None => devnet.clone(),
        };
        let previous = &input["previousBlock"];
        let height = u32::try_from(previous["height"].as_u64().unwrap()).unwrap();
        let transactions: Result<Vec<Transaction>, _> = (input["transactions"].as_array())
            .unwrap()
            .iter()
            .map(|t| {
                let bytes = hex::decode(t.as_str().unwrap()).unwrap();
                Transaction::decode_trusted(&bytes, milestones.at(height))
            })
            .collect();
        let secret = SecretKey::from_hex(input["privateKey"].as_str().unwrap()).unwrap();
        let delegate = Delegate::new(Arc::new(KeyPair::from_secret_key(secret)), 90);
        let aux = hex::decode_array(input["aux"].as_str().unwrap()).unwrap();
        let forged = transactions
            .map_err(|e| e.to_string())
            .and_then(|transactions| {
                let inputs = ForgeInputs {
                    transactions: &transactions,
                    previous_id: BlockId::from_hex(previous["id"].as_str().unwrap()).unwrap(),
                    previous_height: height,
                    timestamp: u32::try_from(input["timestamp"].as_u64().unwrap()).unwrap(),
                    reward: input["reward"].as_str().unwrap().parse().unwrap(),
                };
                (delegate.forge(&inputs, Aux::fixed(aux), &milestones)).map_err(|e| e.to_string())
            });
        match (forged, record.get("output")) {
            (Ok(block), Some(output)) => {
                assert_eq!(block.id().to_hex(), output["id"], "{name}");
                assert_eq!(hex::encode(&block.encode()), output["hex"], "{name}");
            }
            (Err(_), None) => {}
            (forged, _) => panic!("{name}: {forged:?} where the reference gave {record}"),
        }
        compared += 1;
    }
    assert!(compared >= 25, "{compared} records");
}

#[test]
fn the_draft_counts_burned_amounts_and_fees_and_hashes_the_ids() {
    let (_, milestones) = committed_devnet();
    let params = milestones.at(6);
    let keys = KeyPair::from_passphrase("differential test sender").unwrap();
    let recipient = heartwood_crypto::Address::from_public_key(keys.public_key(), 90);
    let item = |amount| heartwood_crypto::transactions::types::TransferItem { amount, recipient };
    let transactions = [
        TransactionBuilder::transfer(vec![item(7), item(8)])
            .fee(10)
            .nonce(1),
        TransactionBuilder::burn(1_000).fee(20).nonce(2),
        TransactionBuilder::burn(2_000).fee(30).nonce(3),
    ]
    .map(|builder| builder.build(&keys, Aux::fixed([1; 32]), params).unwrap());
    let draft = delegate("delegate one")
        .draft(&inputs(&transactions))
        .unwrap();
    assert_eq!((draft.version, draft.height, draft.timestamp), (0, 6, 48));
    assert_eq!(draft.previous_block, [0xab; 32]);
    assert_eq!(draft.number_of_transactions, 3);
    // A transfer's items do not count; a burn's amount does.
    assert_eq!((draft.total_amount, draft.total_fee), (3_000, 60));
    assert_eq!((draft.reward, draft.payload_length), (180_000_000, 96));
    let ids = transactions.iter().map(|t| t.id().as_bytes().as_slice());
    assert_eq!(draft.payload_hash, sha256_concat(ids));
    let empty = delegate("delegate one").draft(&inputs(&[])).unwrap();
    assert_eq!((empty.payload_length, empty.payload_hash), (0, sha256(b"")));
}

#[test]
fn totals_beyond_the_header_and_the_last_height_are_refused() {
    let (_, milestones) = committed_devnet();
    let params = milestones.at(6);
    let keys = KeyPair::from_passphrase("differential test sender").unwrap();
    let most = (1u64 << 63) - 1;
    let burns: Vec<Transaction> = (1..=3)
        .map(|nonce| {
            let builder = TransactionBuilder::burn(most).fee(most).nonce(nonce);
            builder.build(&keys, Aux::fixed([1; 32]), params).unwrap()
        })
        .collect();
    let forger = delegate("delegate one");
    assert_eq!(
        forger.draft(&inputs(&burns)),
        Err(ForgeError::TotalOverflow {
            field: "totalAmount"
        })
    );
    // 2^64 - 2 fits the header's 64 bits, but not the block schema's maximum of 2^63 - 1.
    let two = forger.forge(&inputs(&burns[..2]), Aux::fixed([2; 32]), &milestones);
    assert!(matches!(two, Err(ForgeError::Block(_))), "{two:?}");
    let last = ForgeInputs {
        previous_height: u32::MAX,
        ..inputs(&[])
    };
    assert_eq!(
        forger.draft(&last),
        Err(ForgeError::Height { height: u32::MAX })
    );
}

#[test]
fn a_block_is_signed_with_the_aux_it_is_given() {
    let (_, milestones) = committed_devnet();
    let forger = delegate("delegate one");
    let block = |aux: [u8; 32]| {
        let forged = forger.forge(&inputs(&[]), Aux::fixed(aux), &milestones);
        forged.unwrap().encode()
    };
    assert_eq!(block([3; 32]), block([3; 32]));
    assert_ne!(block([3; 32]), block([4; 32]));
    let random = forger
        .forge(&inputs(&[]), Aux::random(), &milestones)
        .unwrap();
    assert!(random.verify_signature());
    assert_eq!(random.header().generator_public_key, *forger.public_key());
}
