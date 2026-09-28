//! The node's side of its own forger's blocks: the two lateness checks of a block that came from
//! the node's forger.
//!
//! - At receipt ([`crate::Blockchain::handle_incoming_block`], before the future-slot check): a
//!   block in another slot than now, or with less than 2,000 ms left in its slot, is discarded:
//!   `Discarded block <h> because it was received too late`.
//! - At the job's end, when its last result allows a broadcast and the machine is in `newBlock`:
//!   a block in another slot than now, or with less than 2,000 ms left, logs `Discarded block <h>
//!   because it was processed too late` and is marked as forked with one block to roll back,
//!   which removes it through the fork recovery, as the reference removes it; otherwise it is
//!   broadcast.

use std::sync::Arc;

use heartwood_crypto::Block;

use crate::blockchain::{Blockchain, Queue};
use crate::network::Network;
use crate::work::IncomingBlock;

impl<N: Network, Q: Queue> Blockchain<N, Q> {
    /// Whether `block`, from the node's forger, came too late to be processed; logged when it
    /// did.
    pub(crate) fn received_too_late(&self, block: &IncomingBlock) -> bool {
        let _ = block;
        todo!("the receipt lateness of a forger's block")
    }

    /// Whether `block`, from the node's forger, was processed too late to be broadcast; when it
    /// was, it is logged and marked as forked with one block to roll back.
    pub(crate) fn processed_too_late(self: &Arc<Self>, block: &Arc<Block>) -> bool {
        let _ = block;
        todo!("the processing lateness of a forger's block")
    }
}
