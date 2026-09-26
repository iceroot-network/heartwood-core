//! A block, its id and its verification rules.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use std::fmt;

use crate::crypto::sig::Signature;
use crate::errors::HexError;
use crate::identities::{Address, PublicKeyBytes};
use crate::managers::Milestones;
use crate::transactions::{Transaction, TransactionId};
use crate::utils::hex;

/// A block id: the SHA-256 of the serialised header, signature included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId([u8; 32]);

impl BlockId {
    /// The id with these 32 bytes.
    pub const fn from_array(bytes: [u8; 32]) -> BlockId {
        BlockId(bytes)
    }

    /// The id in the hex `text` (64 hex digits).
    pub fn from_hex(text: &str) -> Result<BlockId, HexError> {
        hex::decode_array(text).map(BlockId)
    }

    /// The 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, as the reference writes ids.
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A block header, as serialised: 141 bytes, then the 64-byte signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHeader {
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The height.
    pub height: u32,
    /// The previous block's id (32 zero bytes before the first block).
    pub previous_block: [u8; 32],
    /// The number of transactions.
    pub number_of_transactions: u32,
    /// The sum of the transactions' top-level amounts.
    pub total_amount: u64,
    /// The sum of the transactions' fees.
    pub total_fee: u64,
    /// The block reward.
    pub reward: u64,
    /// The payload length: 32 bytes per transaction.
    pub payload_length: u32,
    /// The SHA-256 of the concatenated transaction ids.
    pub payload_hash: [u8; 32],
    /// The generator's public key, as the header holds it.
    pub generator_public_key: PublicKeyBytes,
    /// The generator's BIP340 signature of the header.
    pub block_signature: Signature,
}

/// A block: header, id and transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub(crate) header: BlockHeader,
    pub(crate) id: BlockId,
    pub(crate) transactions: Vec<Transaction>,
}

impl Block {
    /// The header.
    pub fn header(&self) -> &BlockHeader {
        &self.header
    }

    /// The id.
    pub fn id(&self) -> &BlockId {
        &self.id
    }

    /// The height.
    pub fn height(&self) -> u32 {
        self.header.height
    }

    /// The transactions, in block order.
    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }

    /// The ids of the transactions, in block order.
    pub fn transaction_ids(&self) -> impl Iterator<Item = &TransactionId> {
        self.transactions.iter().map(Transaction::id)
    }

    /// The sum of the burned part of every fee, under the merged milestone at the block's height.
    pub fn burned_fee(&self, milestones: &Milestones) -> u64 {
        todo!()
    }

    /// The donations taken from the block reward, under the merged milestone at the block's height,
    /// in file order.
    pub fn donations(&self, milestones: &Milestones) -> Vec<(Address, u64)> {
        todo!()
    }

    /// Check the block as the reference's `Block.verify` does, under the merged milestone at the
    /// block's height. `current_time` is the time in seconds since the network epoch, which a block
    /// timestamp may not exceed by more than one block time.
    pub fn verify(&self, milestones: &Milestones, current_time: i64) -> Verification {
        todo!()
    }

    /// Whether the header signature is a valid BIP340 signature by the generator key.
    pub fn verify_signature(&self) -> bool {
        todo!()
    }

    /// The block as the reference's `toJson` writes it, transactions included.
    pub fn to_json(&self, milestones: &Milestones) -> serde_json::Value {
        todo!()
    }
}

/// One failed block rule, in the order the reference checks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyFailure {
    /// A block above height 1 without a previous block.
    PreviousBlock,
    /// The header signature does not verify.
    Signature,
    /// The version is not the milestone's block version.
    Version,
    /// The timestamp is more than one block time in the future.
    Timestamp,
    /// The serialised block is larger than the milestone's maximum payload.
    PayloadTooLarge {
        /// The block's size in bytes.
        size: usize,
        /// The maximum.
        max: u32,
    },
    /// Transactions whose signature did not verify.
    UnverifiedTransactions(Vec<TransactionId>),
    /// The header's transaction count differs from the transactions.
    TransactionCount,
    /// The payload length is not 32 bytes per transaction.
    PayloadLength,
    /// More transactions than the milestone allows (above height 1).
    TooManyTransactions,
    /// A transaction appears twice.
    DuplicateTransaction(TransactionId),
    /// The total amount differs from the transactions' sum.
    TotalAmount,
    /// The total fee differs from the transactions' sum.
    TotalFee,
    /// The payload hash differs from the hash of the transaction ids.
    PayloadHash,
}

/// The outcome of [`Block::verify`]: every failed rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verification {
    /// The failed rules, in check order.
    pub failures: Vec<VerifyFailure>,
}

impl Verification {
    /// Whether every rule passed.
    pub fn is_verified(&self) -> bool {
        self.failures.is_empty()
    }
}
