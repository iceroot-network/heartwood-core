//! Blocks to bytes.
//!
//! Header layout (141 bytes, little-endian): version (`u32`), timestamp (`u32`), height (`u32`),
//! previous block (32 bytes), number of transactions (`u32`), total amount, total fee and reward
//! (`u64` each), payload length (`u32`), payload hash (32 bytes), generator key (33 bytes); then
//! the 64-byte signature. A full block follows the header with one `u32` length per transaction and
//! then the transactions' bytes.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::blocks::{Block, BlockHeader, BlockId};

/// Size of the header without the signature.
pub const HEADER_SIZE: usize = 141;

/// The header's bytes, with or without the signature.
pub fn serialise_header(header: &BlockHeader, include_signature: bool) -> Vec<u8> {
    todo!()
}

/// The SHA-256 of the header without its signature: the hash the generator signs.
pub fn signing_hash(header: &BlockHeader) -> [u8; 32] {
    todo!()
}

/// The id of a header: the SHA-256 of its bytes with the signature.
pub fn block_id(header: &BlockHeader) -> BlockId {
    todo!()
}

/// The full block: header, signature, transaction lengths and transactions.
pub fn serialise(block: &Block) -> Vec<u8> {
    todo!()
}

/// The block's size as the payload rule counts it: header, signature, and 4 bytes plus the bytes
/// of each transaction.
pub fn size(block: &Block) -> usize {
    todo!()
}
