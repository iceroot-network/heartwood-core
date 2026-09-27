//! Transaction rows, written with their block (see [`crate::Chunk::save_block`]) and read back
//! with it (see [`crate::StoredBlock`]).

use heartwood_crypto::{BlockId, TransactionId};

/// A row of `transactions`: the reference's columns, as its model converter writes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRow {
    /// The transaction id.
    pub id: TransactionId,
    /// The version.
    pub version: u8,
    /// The id of the block that holds it.
    pub block_id: BlockId,
    /// The height of that block.
    pub block_height: u32,
    /// The position in the block, from 0.
    pub sequence: u32,
    /// The block's timestamp.
    pub timestamp: u32,
    /// The sender's nonce.
    pub nonce: u64,
    /// The sender's address.
    pub sender_id: String,
    /// The sender's public key, lowercase hex.
    pub sender_public_key: String,
    /// The recipient, for the types where the reference sets one.
    pub recipient_id: Option<String>,
    /// The type within its type group.
    pub kind: u16,
    /// The type group.
    pub type_group: u32,
    /// The memo's bytes, if any.
    pub memo: Option<Vec<u8>>,
    /// The top-level amount (0 for a transfer, whose items are in `asset`).
    pub amount: u64,
    /// The fee.
    pub fee: u64,
    /// The burned part of the fee.
    pub burned_fee: u64,
    /// The bytes the block carries.
    pub serialised: Vec<u8>,
    /// The asset as the reference's JSON, if the transaction has one.
    pub asset: Option<String>,
}
