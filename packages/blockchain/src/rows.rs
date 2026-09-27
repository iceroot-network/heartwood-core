//! A block and its transactions as the store's rows, and a stored block back as a block.
//!
//! The derived columns are those the reference computes: a block's `burned_fee` under the
//! milestone at its height, its `donations` as the reference's object (every entry the block
//! carries, in the milestone's order), the generator's `username` (none at height 1); a
//! transaction's `sequence` in block order, the block's `timestamp`, the derived `sender_id`, the
//! `recipient_id` where the reference sets one, the top-level `amount`, the transaction's
//! `burned_fee`, the bytes the block carries after it was received, and the `asset` as the
//! reference's JSON.

use heartwood_crypto::blocks::serialiser::serialise_header;
use heartwood_crypto::crypto::sig::Signature;
use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_crypto::utils::{burn, hex};
use heartwood_crypto::{Block, BlockHeader, BlockId, Milestones};
use heartwood_database::{BlockRow, MissedRow, RoundRow, StoredBlock, TransactionRow};
use heartwood_state::round_state::{MissedBlock, RoundDelegate};
use serde_json::{Map, Value};

use crate::errors::StoreError;

/// The row of `block`, forged by the delegate `username` (none for the genesis block).
pub fn block_row(
    block: &Block,
    username: Option<&str>,
    milestones: &Milestones,
) -> Result<BlockRow, StoreError> {
    let header = block.header();
    let burned_fee =
        u64::try_from(block.burned_fee(milestones)).map_err(|_| StoreError::Range {
            what: "the burned fee",
            height: header.height,
        })?;
    let donations = block.donations(milestones).into_iter();
    let donations: Map<String, Value> = donations
        .map(|(address, amount)| (address.to_base58(), Value::from(amount.to_string())))
        .collect();
    Ok(BlockRow {
        id: *block.id(),
        height: header.height,
        version: header.version,
        timestamp: header.timestamp,
        previous_block: Some(BlockId::from_array(header.previous_block)),
        number_of_transactions: header.number_of_transactions,
        total_amount: header.total_amount,
        total_fee: header.total_fee,
        burned_fee,
        reward: header.reward,
        donations: Value::Object(donations).to_string(),
        payload_length: header.payload_length,
        payload_hash: hex::encode(&header.payload_hash),
        generator_public_key: header.generator_public_key.to_hex(),
        username: username.map(str::to_owned),
        block_signature: header.block_signature.to_hex(),
    })
}

/// The rows of `block`'s transactions, in block order. None of the transaction types carried
/// has a top-level recipient, so `recipient_id` is empty, as the reference leaves it for them.
pub fn transaction_rows(
    block: &Block,
    milestones: &Milestones,
) -> Result<Vec<TransactionRow>, StoreError> {
    let header = block.header();
    let params = milestones.at(header.height);
    let rows = (0..)
        .zip(block.transactions())
        .map(|(sequence, transaction)| {
            let data = transaction.data();
            let memo = data
                .memo
                .as_ref()
                .map(|memo| memo.as_str().as_bytes().to_vec());
            TransactionRow {
                id: *transaction.id(),
                version: data.effective_version(),
                block_id: *block.id(),
                block_height: header.height,
                sequence,
                timestamp: header.timestamp,
                nonce: data.nonce,
                sender_id: data.sender_address(params).to_base58(),
                sender_public_key: data.sender_public_key.to_hex(),
                recipient_id: None,
                kind: data.kind.type_id(),
                type_group: data.kind.type_group(),
                memo,
                amount: transaction.amount(),
                fee: transaction.fee(),
                burned_fee: burn::burned_fee(transaction.fee(), params),
                serialised: transaction.bytes().to_vec(),
                asset: transaction
                    .to_json(params)
                    .get("asset")
                    .map(Value::to_string),
            }
        });
    Ok(rows.collect())
}

/// The block that `stored` holds, rebuilt from its row and its transactions' bytes, which this
/// node stored itself, so they are decoded without the transaction rules. The rebuilt block must
/// have the stored id.
pub fn stored_block(stored: &StoredBlock, milestones: &Milestones) -> Result<Block, StoreError> {
    let row = &stored.row;
    let decode = |detail: String| StoreError::Decode {
        what: "block",
        detail: format!("height {}: {detail}", row.height),
    };
    let header = BlockHeader {
        version: row.version,
        timestamp: row.timestamp,
        height: row.height,
        previous_block: row.previous_block.map_or([0; 32], |id| *id.as_bytes()),
        number_of_transactions: row.number_of_transactions,
        total_amount: row.total_amount,
        total_fee: row.total_fee,
        reward: row.reward,
        payload_length: row.payload_length,
        payload_hash: hex::decode_array(&row.payload_hash).map_err(|e| decode(e.to_string()))?,
        generator_public_key: (PublicKeyBytes::from_hex(&row.generator_public_key))
            .map_err(|e| decode(e.to_string()))?,
        block_signature: (Signature::from_hex(&row.block_signature))
            .map_err(|e| decode(e.to_string()))?,
    };
    let mut bytes = serialise_header(&header, true);
    for (_, transaction) in &stored.transactions {
        let length = u32::try_from(transaction.len()).map_err(|e| decode(e.to_string()))?;
        bytes.extend_from_slice(&length.to_le_bytes());
    }
    for (_, transaction) in &stored.transactions {
        bytes.extend_from_slice(transaction);
    }
    let block = Block::decode_trusted(&bytes, milestones).map_err(|e| decode(e.to_string()))?;
    if block.id() != &row.id {
        return Err(decode(format!("the block's id is {}", block.id())));
    }
    Ok(block)
}

/// The rows of round `round`, whose delegates are `delegates` in rank order.
pub fn round_rows(round: u64, delegates: &[RoundDelegate]) -> Vec<RoundRow> {
    let row = |delegate: &RoundDelegate| RoundRow {
        round,
        public_key: delegate.public_key.to_hex(),
        balance: delegate.vote_balance,
    };
    delegates.iter().map(row).collect()
}

/// The rows of the slots `missed` before a block.
pub fn missed_rows(missed: &[MissedBlock]) -> Vec<MissedRow> {
    let row = |slot: &MissedBlock| MissedRow {
        timestamp: slot.timestamp,
        height: slot.height,
        username: slot.username.clone(),
    };
    missed.iter().map(row).collect()
}
