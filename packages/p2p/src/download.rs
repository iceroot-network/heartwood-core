//! A downloaded block, and how it becomes a [`Block`] as the reference's download path makes one.
//!
//! The communicator reads each transaction of a getBlocks reply as the reference's
//! `fromBytesUnsafe` reads it (`Transaction::decode_trusted`: the processor's block verification
//! checks it again), where the reference reads it, so a transaction it refuses fails the download.
//! [`to_block`] then rebuilds the block from the header's fields and those transactions as
//! `BlockFactory.fromData` does (`blocks::factory::from_data`), with the id, payload hash and
//! signature of the canonical bytes.

use std::net::IpAddr;

use heartwood_crypto::{Block, Milestones, Transaction};
use heartwood_p2p_wire::codecs::blocks::ListedBlock;

use crate::errors::AssembleError;

/// A block of a getBlocks reply that passed the reply check.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadedBlock {
    /// The header as the reply check left it (the generator's key lowercased). Its `transactions`
    /// field is not used: the transactions are in [`DownloadedBlock::transactions`].
    pub header: ListedBlock,
    /// The transactions, read as the reference's `fromBytesUnsafe` reads them; `None` for a block
    /// fetched with headers only.
    pub transactions: Option<Vec<Transaction>>,
    /// The address of the peer the block came from.
    pub ip: IpAddr,
}

/// The block of `downloaded` as `BlockFactory.fromData` rebuilds it under `milestones`.
pub fn to_block(
    downloaded: &DownloadedBlock,
    milestones: &Milestones,
) -> Result<Block, AssembleError> {
    let _ = (downloaded, milestones);
    todo!("rebuild the block")
}
