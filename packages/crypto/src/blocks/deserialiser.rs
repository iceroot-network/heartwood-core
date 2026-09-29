//! Bytes to blocks.
//!
//! A block is its header and signature, then, when any bytes follow, one `u32` length per
//! transaction the header counts and the transactions themselves. As in the reference:
//!
//! - bytes that end right after the signature make a block without transactions, whatever the
//!   header counts (the block then fails [`Block::verify`]);
//! - bytes after the last transaction are ignored;
//! - the transactions of a block are decoded under the merged milestone at the block's height.
//!
//! There are three entry points:
//!
//! - [`Block::receive`] for a block from another node. It is the one a node uses for blocks from
//!   peers, whether pushed or downloaded;
//! - [`Block::decode`], the decoder alone, for the configured genesis block and for tests;
//! - [`Block::decode_trusted`] for blocks this node verified before.

use crate::blocks::{Block, BlockHeader, factory, serialiser};
use crate::crypto::sig::Signature;
use crate::errors::{BlockDecodeError, BlockError, DecodeError};
use crate::identities::PublicKeyBytes;
use crate::managers::{Milestones, Params};
use crate::transactions::Transaction;
use crate::utils::bytes::Reader;
use crate::validation::Mode;

/// The largest total amount, total fee and reward the reference's block schema accepts: the
/// largest signed 64-bit integer. The totals of the block at height 1 are exempt.
const MAX_SCHEMA_AMOUNT: u64 = i64::MAX as u64;

/// The rules the transactions of a block are decoded under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rules {
    /// The strict transaction rules, and at height 1 the rules for the configured genesis block's
    /// transactions ([`Mode::Genesis`]).
    Decode,
    /// The strict transaction rules at every height.
    Received,
    /// No transaction rules: bytes this node verified before.
    Trusted,
}

impl Rules {
    fn decode_transaction(
        self,
        bytes: &[u8],
        params: &Params,
        height: u32,
    ) -> Result<Transaction, DecodeError> {
        match self {
            Rules::Decode if height == 1 => Transaction::decode_with(bytes, params, Mode::Genesis),
            Rules::Decode | Rules::Received => {
                Transaction::decode_with(bytes, params, Mode::Strict)
            }
            Rules::Trusted => Transaction::decode_trusted(bytes, params),
        }
    }
}

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
    /// Decode `bytes` as the reference's `BlockFactory.fromBytes` does: the header, then each
    /// transaction as [`crate::Transaction::decode`] does under the merged milestone at the
    /// block's height, then the block rules the reference applies as a schema. The ids, the
    /// payload hash and the size that [`Block::verify`] checks are those of the bytes as received.
    ///
    /// The transactions of a block at height 1 are decoded under the rules for the configured
    /// genesis block's transactions ([`Mode::Genesis`]: a zero fee passes). The reference exempts
    /// only the transaction ids that its configured genesis block lists, so this decoder is for
    /// that block, read from the chain definition, and for tests. A block from another node goes
    /// through [`Block::receive`] instead.
    pub fn decode(bytes: &[u8], milestones: &Milestones) -> Result<Block, BlockDecodeError> {
        decode_with(bytes, milestones, Rules::Decode)
    }

    /// Accept `bytes` from another node, as the reference does with a block a peer sends or a
    /// node downloads: decode the block, then build it again from the decoded fields as
    /// `BlockFactory.fromData` does, which serialises every transaction again from its fields
    /// (after the transaction rules have changed them) and decodes the result. The block that
    /// [`Block::verify`] then checks has the canonical bytes, and its ids, payload hash and size
    /// are those of the canonical bytes.
    ///
    /// The two differ from [`Block::decode`] when a transaction's wire form is not the canonical
    /// one: a vote name that the rules remove changes the transaction's id once the vote is
    /// serialised again, so a payload hash over the ids of the wire form fails here, as it does in
    /// the reference; and a stray byte or a repeated vote name counts in the size of the wire form
    /// but not in the size of the canonical one.
    ///
    /// Every transaction is decoded under the strict transaction rules, at height 1 too: a node
    /// reads its genesis block from the chain definition and never receives it.
    ///
    /// A transaction whose memo is not valid UTF-8 refuses the block
    /// ([`crate::errors::DecodeError::InvalidMemo`]). The reference decodes such a memo lossily
    /// and accepts the block, and a copy of it whose memo holds the replacement character instead
    /// has the same block id and is valid: a refusal for this reason says nothing about the block
    /// id, and a node must not mark the id as invalid because of it.
    pub fn receive(bytes: &[u8], milestones: &Milestones) -> Result<Block, BlockError> {
        let decoded = decode_with(bytes, milestones, Rules::Received)?;
        factory::assemble(
            decoded.header,
            Some(&decoded.transactions),
            milestones,
            Rules::Received,
        )
    }

    /// Decode `bytes` that this node verified before, with
    /// [`crate::Transaction::decode_trusted`] for the transactions, as the reference does for
    /// blocks it reads back from its own store.
    pub fn decode_trusted(
        bytes: &[u8],
        milestones: &Milestones,
    ) -> Result<Block, BlockDecodeError> {
        decode_with(bytes, milestones, Rules::Trusted)
    }

    /// The block's bytes (see [`crate::blocks::serialiser::serialise`]).
    pub fn encode(&self) -> Vec<u8> {
        crate::blocks::serialiser::serialise(self)
    }
}

/// Decode `bytes` with the transactions under `rules`.
pub(crate) fn decode_with(
    bytes: &[u8],
    milestones: &Milestones,
    rules: Rules,
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
        transactions.reserve(capacity_for(count, reader.remaining()));
        for index in 0..count {
            let length = lengths.u32_le()?;
            let transaction_bytes = reader.bytes(length as usize)?;
            let transaction = rules
                .decode_transaction(transaction_bytes, params, header.height)
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

/// The fewest bytes a serialised transaction takes: its signature alone.
const MIN_TRANSACTION_BYTES: usize = 64;

/// The transactions to make room for when a block states `count` of them and `remaining` bytes
/// follow its lengths: no more than those bytes can hold, so a stated count does not reserve
/// about 90 times the bytes it came in.
fn capacity_for(count: usize, remaining: usize) -> usize {
    count.min(remaining.checked_div(MIN_TRANSACTION_BYTES).unwrap_or(0))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_room_made_for_transactions_is_what_the_bytes_can_hold() {
        assert_eq!(capacity_for(500_000, 0), 0);
        assert_eq!(capacity_for(500_000, 6_400), 100);
        assert_eq!(capacity_for(3, 1_000_000), 3);
    }
}
