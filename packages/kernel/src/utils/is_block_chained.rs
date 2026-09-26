//! Whether a block chains onto the one before it: it names the previous block's id, its height is
//! one more, and its slot is later.

use heartwood_crypto::{Block, BlockHeader, BlockId, Milestones};

use crate::errors::NotChained;

/// What chaining reads of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainLink {
    /// The block's id.
    pub id: BlockId,
    /// The id of the block it follows.
    pub previous_block: BlockId,
    /// The height.
    pub height: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
}

impl ChainLink {
    /// The link of `block`.
    pub fn of(block: &Block) -> ChainLink {
        ChainLink::of_header(block.header(), *block.id())
    }

    /// The link of a block with header `header` and id `id`.
    pub fn of_header(header: &BlockHeader, id: BlockId) -> ChainLink {
        ChainLink {
            id,
            previous_block: BlockId::from_array(header.previous_block),
            height: header.height,
            timestamp: header.timestamp,
        }
    }
}

/// Whether `next` chains onto `previous`; the first check that fails is the error.
pub fn is_block_chained(
    previous: &ChainLink,
    next: &ChainLink,
    milestones: &Milestones,
) -> Result<(), NotChained> {
    let _ = (previous, next, milestones);
    todo!()
}
