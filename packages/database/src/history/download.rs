//! The reads of the peer-to-peer server: the blocks a peer downloads, and the blocks among a
//! peer's ids, on a [`Reader`].

use heartwood_crypto::BlockId;

use crate::blocks::BlockRow;
use crate::errors::DatabaseError;
use crate::reader::Reader;

/// A block for a peer's download: its row and, unless the peer asked for headers only, its
/// transactions' bytes in block order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadBlockRow {
    /// The block's row.
    pub row: BlockRow,
    /// The transactions' bytes; `None` for headers only.
    pub transactions: Option<Vec<Vec<u8>>>,
}

impl Reader<'_> {
    /// At most `limit` stored blocks from height `from`, in height order, with their
    /// transactions unless `headers_only`: the reference's `getBlocksForDownload`.
    pub fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<DownloadBlockRow>, DatabaseError> {
        let _ = (from, limit, headers_only);
        todo!("the blocks for a download")
    }

    /// The rows of the stored blocks among `ids`, in height order: the reference's
    /// `getBlocksByIds`, for getCommonBlocks.
    pub fn blocks_by_ids(&self, ids: &[BlockId]) -> Result<Vec<BlockRow>, DatabaseError> {
        let _ = ids;
        todo!("the blocks by id")
    }
}
