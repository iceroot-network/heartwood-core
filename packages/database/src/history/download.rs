//! The reads of the peer-to-peer server: the blocks a peer downloads, and the blocks among a
//! peer's ids, on a [`Reader`].

use heartwood_crypto::BlockId;
use rusqlite::OptionalExtension;

use crate::blocks::{self, BlockRow};
use crate::errors::DatabaseError;
use crate::reader::Reader;
use crate::values::Columns;

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
    /// transactions unless `headers_only`: the reference's `getBlocksForDownload`, which reads the
    /// heights from `from` to `from + limit - 1` in one read transaction.
    pub fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<DownloadBlockRow>, DatabaseError> {
        let Some(to) = from.saturating_add(limit).checked_sub(1) else {
            return Ok(Vec::new());
        };
        let read = self.connection().unchecked_transaction()?;
        let rows = self.timed(|connection| {
            let sql = format!(
                "SELECT {} FROM blocks WHERE height BETWEEN ?1 AND ?2 ORDER BY height",
                blocks::COLUMNS
            );
            let mut statement = connection.prepare_cached(&sql)?;
            let mut rows = statement.query([from, to])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(blocks::read_block(row)?);
            }
            Ok(out)
        })?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let transactions = match headers_only {
                true => None,
                false => Some(self.serialised(&row.id)?),
            };
            out.push(DownloadBlockRow { row, transactions });
        }
        read.commit()?;
        Ok(out)
    }

    /// The rows of the stored blocks among `ids`, in height order, each once: the reference's
    /// `getBlocksByIds`, for getCommonBlocks.
    pub fn blocks_by_ids(&self, ids: &[BlockId]) -> Result<Vec<BlockRow>, DatabaseError> {
        let mut out = self.timed(|connection| {
            let sql = format!("SELECT {} FROM blocks WHERE id = ?1", blocks::COLUMNS);
            let mut statement = connection.prepare_cached(&sql)?;
            let mut out: Vec<BlockRow> = Vec::new();
            for id in ids {
                let row = statement
                    .query_row([id.to_hex()], |row| Ok(blocks::read_block(row)))
                    .optional()?;
                if let Some(row) = row.transpose()? {
                    out.push(row);
                }
            }
            Ok(out)
        })?;
        out.sort_by_key(|row| row.height);
        out.dedup_by_key(|row| row.height);
        Ok(out)
    }

    /// The serialised transactions of the block `id`, by sequence.
    fn serialised(&self, id: &BlockId) -> Result<Vec<Vec<u8>>, DatabaseError> {
        self.timed(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT serialised FROM transactions WHERE block_id = ?1 ORDER BY sequence",
            )?;
            let mut rows = statement.query([id.to_hex()])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(Columns::new("transactions", row).get(0)?);
            }
            Ok(out)
        })
    }
}
