//! Making blocks: signing a new header, and reading the reference's block JSON.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::crypto::sig::Aux;
use crate::errors::BlockError;
use crate::identities::KeyPair;
use crate::managers::Milestones;
use crate::transactions::Transaction;

use super::Block;

/// The header fields of a block to sign; the generator key comes from the signing keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDraft {
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The height.
    pub height: u32,
    /// The previous block's id (32 zero bytes before the first block).
    pub previous_block: [u8; 32],
    /// The number of transactions, as the header states it.
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
}

/// Sign `draft` with `keys` (BIP340, `aux`) and make the block with `transactions`, as the
/// reference's `BlockFactory.make` does: the header is signed as given, and the result is decoded
/// again under the merged milestone at its height.
pub fn make(
    draft: BlockDraft,
    transactions: Vec<Transaction>,
    keys: &KeyPair,
    aux: Aux,
    milestones: &Milestones,
) -> Result<Block, BlockError> {
    todo!()
}

/// The block in the reference's JSON form, as `BlockFactory.fromJson` reads it.
pub fn from_json(json: &serde_json::Value, milestones: &Milestones) -> Result<Block, BlockError> {
    todo!()
}
