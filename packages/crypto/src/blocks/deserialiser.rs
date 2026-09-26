//! Bytes to blocks.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::blocks::{Block, BlockHeader};
use crate::errors::BlockDecodeError;
use crate::managers::Milestones;
use crate::utils::bytes::Reader;

/// Read a header and its signature.
pub fn deserialise_header(reader: &mut Reader<'_>) -> Result<BlockHeader, BlockDecodeError> {
    todo!()
}

impl Block {
    /// Decode `bytes`: the header, then each transaction with [`crate::Transaction::decode`] under
    /// the merged milestone at the block's height, then the block rules the reference applies as a
    /// schema.
    pub fn decode(bytes: &[u8], milestones: &Milestones) -> Result<Block, BlockDecodeError> {
        todo!()
    }

    /// Decode `bytes` that this node verified before, with
    /// [`crate::Transaction::decode_trusted`] for the transactions, as the reference does for
    /// blocks it reads back from its own store.
    pub fn decode_trusted(
        bytes: &[u8],
        milestones: &Milestones,
    ) -> Result<Block, BlockDecodeError> {
        todo!()
    }

    /// The block's bytes (see [`crate::blocks::serialiser::serialise`]).
    pub fn encode(&self) -> Vec<u8> {
        crate::blocks::serialiser::serialise(self)
    }
}
