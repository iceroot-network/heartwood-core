//! Blocks (vector class V05): made with the factory and decoded again, byte for byte and verdict
//! for verdict, plus the decoder's behaviour on bytes the vectors do not cover.

#[macro_use]
mod common;
mod blocks_common;

use blocks_common::{
    DEVNET, NOW, Tally, check_devnet, decode_record, make_record, transaction_record,
};
use heartwood_crypto::Block;
use heartwood_crypto::blocks::VerifyFailure;
use heartwood_crypto::blocks::factory;
use heartwood_crypto::blocks::serialiser::{self, HEADER_SIZE, SIGNATURE_SIZE};
use heartwood_crypto::errors::BlockDecodeError;
use heartwood_crypto::utils::hex;
use proptest::prelude::*;
use serde_json::Value;

#[test]
fn v05_blocks() {
    let file = vector_file!("V05-blocks");
    check_devnet(&file);
    let mut tally = Tally::default();
    for record in &file.records {
        let outcome = match record.op.as_str() {
            "block.make" => make_record(record),
            "block.fromBytes" => decode_record(record),
            "tx.fromBytes" => transaction_record(record),
            other => Err(format!("unknown operation {other}")),
        };
        tally.check(record.label(&file.class), outcome);
    }
    tally.finish(&file);
}

/// The bytes of the V05 block named `name`, from its decode record.
fn block_bytes(name: &str) -> Vec<u8> {
    let file = vector_file!("V05-blocks");
    let record = file
        .op("block.fromBytes")
        .find(|record| record.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no block {name:?}"));
    hex::decode(record.input["hex"].as_str().unwrap()).unwrap()
}

fn decode(bytes: &[u8]) -> Result<Block, BlockDecodeError> {
    Block::decode(bytes, &DEVNET.milestones)
}

#[test]
fn timestamps_may_not_run_ahead_by_more_than_a_block_time() {
    let block = decode(&block_bytes("one transfer")).unwrap();
    let timestamp = i64::from(block.header().timestamp);
    let block_time = i64::try_from(DEVNET.milestones.at(block.height()).block_time()).unwrap();
    assert!(
        block
            .verify(&DEVNET.milestones, timestamp - block_time)
            .is_verified()
    );
    assert_eq!(
        block
            .verify(&DEVNET.milestones, timestamp - block_time - 1)
            .failures,
        [VerifyFailure::Timestamp]
    );
    assert!(block.verify(&DEVNET.milestones, NOW).is_verified());
}

#[test]
fn truncated_and_trailing_bytes() {
    let bytes = block_bytes("one transfer");
    let block = decode(&bytes).unwrap();
    assert_eq!(block.encode(), bytes);

    // Bytes that end right after the signature make a block without transactions, which then
    // fails verification.
    let header_only = decode(&bytes[..HEADER_SIZE + SIGNATURE_SIZE]).unwrap();
    assert_eq!(header_only.id(), block.id());
    assert!(header_only.transactions().is_empty());
    assert_eq!(
        header_only.verify(&DEVNET.milestones, NOW).failures,
        [
            VerifyFailure::TransactionCount,
            VerifyFailure::TotalFee,
            VerifyFailure::PayloadHash
        ]
    );
    for length in (0..bytes.len()).filter(|length| *length != HEADER_SIZE + SIGNATURE_SIZE) {
        assert!(
            matches!(
                decode(&bytes[..length]),
                Err(BlockDecodeError::Truncated(_))
            ),
            "{length} bytes"
        );
    }

    // Bytes after the last transaction are ignored.
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0xee; 7]);
    let again = decode(&longer).unwrap();
    assert_eq!(again, block);
    assert_eq!(again.encode(), bytes);
}

#[test]
fn a_count_the_bytes_cannot_hold_is_refused_before_allocation() {
    let mut bytes = block_bytes("empty");
    // numberOfTransactions sits after version, timestamp, height and the previous block.
    bytes[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes.push(0);
    assert!(matches!(
        decode(&bytes),
        Err(BlockDecodeError::Truncated(_))
    ));
}

#[test]
fn header_rules_of_the_schema() {
    let empty = block_bytes("empty");
    let with = |offset: usize, field: &[u8]| {
        let mut bytes = empty.clone();
        bytes[offset..offset + field.len()].copy_from_slice(field);
        decode(&bytes)
    };
    const HEIGHT: usize = 8;
    const TOTAL_AMOUNT: usize = 48;
    const TOTAL_FEE: usize = 56;
    const REWARD: usize = 64;
    let over = (i64::MAX as u64 + 1).to_le_bytes();
    let max = (i64::MAX as u64).to_le_bytes();

    assert!(matches!(
        with(HEIGHT, &0_u32.to_le_bytes()),
        Err(BlockDecodeError::Schema { height: 0, .. })
    ));
    for offset in [TOTAL_AMOUNT, TOTAL_FEE, REWARD] {
        assert!(with(offset, &max).is_ok());
        assert!(matches!(
            with(offset, &over),
            Err(BlockDecodeError::Schema { height: 2, .. })
        ));
    }
    // At height 1 the totals are exempt, the reward is not.
    let first = |offset: usize, field: &[u8]| {
        let mut bytes = empty.clone();
        bytes[HEIGHT..HEIGHT + 4].copy_from_slice(&1_u32.to_le_bytes());
        bytes[offset..offset + field.len()].copy_from_slice(field);
        decode(&bytes)
    };
    assert!(first(TOTAL_AMOUNT, &u64::MAX.to_le_bytes()).is_ok());
    assert!(first(TOTAL_FEE, &u64::MAX.to_le_bytes()).is_ok());
    assert!(matches!(
        first(REWARD, &over),
        Err(BlockDecodeError::Schema { height: 1, .. })
    ));
}

#[test]
fn a_changed_header_breaks_the_signature_and_the_id() {
    let bytes = block_bytes("the kept types");
    let block = decode(&bytes).unwrap();
    assert!(block.verify_signature());
    let mut changed = bytes.clone();
    changed[4] ^= 1;
    let changed = decode(&changed).unwrap();
    assert_ne!(changed.id(), block.id());
    assert!(!changed.verify_signature());
    assert_eq!(
        changed.verify(&DEVNET.milestones, NOW).failures,
        [VerifyFailure::Signature]
    );
    assert_eq!(
        serialiser::block_id(block.header()),
        *block.id(),
        "the id is the hash of the signed header"
    );
}

#[test]
fn a_repeated_transaction_is_reported_each_time() {
    let block = decode(&block_bytes("one transfer")).unwrap();
    let transaction = block.transactions()[0].clone();
    let repeated = vec![transaction.clone(), transaction.clone(), transaction];
    let mut header = block.header().clone();
    header.number_of_transactions = 3;
    header.payload_length = 96;
    header.total_fee *= 3;
    let with_three = factory::from_data(header, &repeated, &DEVNET.milestones).unwrap();
    let id = *with_three.transactions()[0].id();
    assert_eq!(
        with_three.verify(&DEVNET.milestones, NOW).failures,
        [
            VerifyFailure::Signature,
            VerifyFailure::DuplicateTransaction(id),
            VerifyFailure::DuplicateTransaction(id),
            VerifyFailure::PayloadHash
        ]
    );
}

#[test]
fn the_genesis_block_json_rebuilds_the_genesis_id() {
    let file = vector_file!("V10-genesis");
    for record in file.op("genesis.generate") {
        let output = record.expected.as_ref().unwrap();
        let text = output["files"]["crypto/genesisBlock.json"]
            .as_str()
            .unwrap();
        let milestones_text = output["files"]["crypto/milestones.json"].as_str().unwrap();
        let milestones =
            heartwood_crypto::Milestones::load(milestones_text, &DEVNET.network).unwrap();
        let json: Value = serde_json::from_str(text).unwrap();
        let block = factory::from_json(&json, &milestones).unwrap();
        assert_eq!(block.id().to_hex(), output["genesisId"].as_str().unwrap());
        assert_eq!(
            hex::encode(&block.header().payload_hash),
            output["nethash"].as_str().unwrap()
        );
        assert_eq!(
            block.transactions().len() as u64,
            output["transactions"].as_u64().unwrap()
        );
        assert!(block.verify(&milestones, 0).is_verified());
        assert_eq!(decode_with(&block.encode(), &milestones), block);
    }
}

fn decode_with(bytes: &[u8], milestones: &heartwood_crypto::Milestones) -> Block {
    Block::decode(bytes, milestones).unwrap()
}

#[test]
fn block_json_is_refused_when_malformed() {
    let block = decode(&block_bytes("one transfer")).unwrap();
    let json = block.to_json(&DEVNET.milestones);
    assert_eq!(
        factory::from_json(&json, &DEVNET.milestones).unwrap(),
        block
    );
    for (key, value) in [
        ("id", Value::from("00")),
        ("height", Value::from(-1)),
        ("height", Value::from(1.5)),
        ("totalFee", Value::from("-1")),
        ("totalFee", Value::from("0x10")),
        ("reward", Value::from("18446744073709551616")),
        ("payloadHash", Value::from("ab")),
        ("blockSignature", Value::Null),
        ("numberOfTransactions", Value::from(2)),
        ("transactions", Value::from("none")),
    ] {
        let mut changed = json.clone();
        changed[key] = value.clone();
        assert!(
            factory::from_json(&changed, &DEVNET.milestones).is_err(),
            "{key}: {value}"
        );
    }
}

proptest! {
    // An integration test has no source file for proptest to record failures next to.
    #![proptest_config(ProptestConfig {
        cases: 512,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let _ = decode(&bytes);
    }

    #[test]
    fn mutated_blocks_never_panic(position in any::<usize>(), value in any::<u8>(), cut in any::<usize>()) {
        let mut bytes = block_bytes("the kept types");
        let position = position % bytes.len();
        bytes[position] = value;
        bytes.truncate(cut % (bytes.len() + 1));
        if let Ok(block) = decode(&bytes) {
            let _ = block.verify(&DEVNET.milestones, NOW);
            let _ = block.to_json(&DEVNET.milestones);
            let again = decode(&block.encode());
            prop_assert_eq!(again.as_ref().map(Block::id), Ok(block.id()));
        }
    }
}
