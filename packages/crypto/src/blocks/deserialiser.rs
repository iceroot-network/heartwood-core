//! Bytes to blocks.
//!
//! A block is its header and signature, then, when any bytes follow, one `u32` length per
//! transaction the header counts and the transactions themselves. As in the reference:
//!
//! - bytes that end right after the signature make a block without transactions, whatever the
//!   header counts (the block then fails [`Block::verify`]);
//! - bytes after the last transaction are ignored;
//! - the transactions of a block are decoded under the merged milestone at the block's height.

use crate::blocks::{Block, BlockHeader, serialiser};
use crate::crypto::sig::Signature;
use crate::errors::{BlockDecodeError, DecodeError};
use crate::identities::PublicKeyBytes;
use crate::managers::{Milestones, Params};
use crate::transactions::Transaction;
use crate::utils::bytes::Reader;

/// The largest total amount, total fee and reward the reference's block schema accepts: the
/// largest signed 64-bit integer. The totals of the block at height 1 are exempt.
const MAX_SCHEMA_AMOUNT: u64 = i64::MAX as u64;

/// Read a header and its signature.
pub fn deserialise_header(reader: &mut Reader<'_>) -> Result<BlockHeader, BlockDecodeError> {
    Ok(BlockHeader {
        version: reader.u32_le()?,
        timestamp: reader.u32_le()?,
        height: reader.u32_le()?,
        previous_block: reader.array()?,
        number_of_transactions: reader.u32_le()?,
        total_amount: reader.u64_le()?,
        total_fee: reader.u64_le()?,
        reward: reader.u64_le()?,
        payload_length: reader.u32_le()?,
        payload_hash: reader.array()?,
        generator_public_key: PublicKeyBytes::from_array(reader.array()?),
        block_signature: Signature::from_array(reader.array()?),
    })
}

impl Block {
    /// Decode `bytes`: the header, then each transaction with [`crate::Transaction::decode`] under
    /// the merged milestone at the block's height, then the block rules the reference applies as a
    /// schema.
    pub fn decode(bytes: &[u8], milestones: &Milestones) -> Result<Block, BlockDecodeError> {
        decode_with(bytes, milestones, Transaction::decode)
    }

    /// Decode `bytes` that this node verified before, with
    /// [`crate::Transaction::decode_trusted`] for the transactions, as the reference does for
    /// blocks it reads back from its own store.
    pub fn decode_trusted(
        bytes: &[u8],
        milestones: &Milestones,
    ) -> Result<Block, BlockDecodeError> {
        decode_with(bytes, milestones, Transaction::decode_trusted)
    }

    /// The block's bytes (see [`crate::blocks::serialiser::serialise`]).
    pub fn encode(&self) -> Vec<u8> {
        crate::blocks::serialiser::serialise(self)
    }
}

fn decode_with(
    bytes: &[u8],
    milestones: &Milestones,
    decode_transaction: fn(&[u8], &Params) -> Result<Transaction, DecodeError>,
) -> Result<Block, BlockDecodeError> {
    let mut reader = Reader::new(bytes);
    let header = deserialise_header(&mut reader)?;
    let params = milestones.at(header.height);

    let mut transactions = Vec::new();
    if !reader.is_empty() {
        let count = header.number_of_transactions as usize;
        // Every length is read before any transaction, so a count the bytes cannot hold fails
        // here, before anything is allocated for it.
        let mut lengths = Reader::new(reader.bytes(count.saturating_mul(4))?);
        transactions.reserve(count);
        for index in 0..count {
            let length = lengths.u32_le()?;
            let transaction_bytes = reader.bytes(length as usize)?;
            let transaction = decode_transaction(transaction_bytes, params)
                .map_err(|source| BlockDecodeError::Transaction { index, source })?;
            transactions.push(transaction);
        }
    }

    check_header(&header)?;
    Ok(Block {
        id: serialiser::block_id(&header),
        header,
        transactions,
    })
}

/// The header rules of the reference's block schema that decoded bytes can break.
fn check_header(header: &BlockHeader) -> Result<(), BlockDecodeError> {
    let refuse = |reason| {
        Err(BlockDecodeError::Schema {
            height: header.height,
            reason,
        })
    };
    if header.height < 1 {
        return refuse("the height must be at least 1");
    }
    let first = header.height == 1;
    if header.total_amount > MAX_SCHEMA_AMOUNT && !first {
        return refuse("the total amount must be at most 9223372036854775807");
    }
    if header.total_fee > MAX_SCHEMA_AMOUNT && !first {
        return refuse("the total fee must be at most 9223372036854775807");
    }
    if header.reward > MAX_SCHEMA_AMOUNT {
        return refuse("the reward must be at most 9223372036854775807");
    }
    Ok(())
}
