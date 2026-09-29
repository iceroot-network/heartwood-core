//! The burned totals of `GET /blockchain`, over a range of heights, so that a node can grow them as
//! its tip grows.

use crate::errors::DatabaseError;
use crate::reader::Reader;
use crate::values::Columns;

/// What the blocks of a range of heights burned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BurnedTotals {
    /// The fees the blocks burned.
    pub fees: u128,
    /// The amounts of the blocks' burn transactions.
    pub transactions: u128,
}

impl Reader<'_> {
    /// What the blocks at the heights above `above`, up to and with `to`, burned.
    pub fn burned_totals(&self, above: u32, to: u32) -> Result<BurnedTotals, DatabaseError> {
        self.timed(|connection| {
            let sql = format!(
                "SELECT {}, {} FROM transactions WHERE block_height > ?1 AND block_height <= ?2",
                wide_sum("burned_fee"),
                wide_sum("CASE WHEN type_group = 2 AND type = 0 THEN amount ELSE 0 END"),
            );
            connection
                .prepare_cached(&sql)?
                .query_row([above, to], |row| {
                    let columns = Columns::new("transactions", row);
                    Ok((read_wide(&columns, 0), read_wide(&columns, 2)))
                })
                .map_err(DatabaseError::from)
                .and_then(|(fees, transactions)| {
                    Ok(BurnedTotals {
                        fees: fees?,
                        transactions: transactions?,
                    })
                })
        })
    }
}

impl Reader<'_> {
    /// The rewards of the blocks at the heights above `above`, up to and with `to`.
    pub fn rewards(&self, above: u32, to: u32) -> Result<u128, DatabaseError> {
        self.timed(|connection| {
            let sql = format!(
                "SELECT {} FROM blocks WHERE height > ?1 AND height <= ?2",
                wide_sum("reward")
            );
            connection
                .prepare_cached(&sql)?
                .query_row([above, to], |row| {
                    Ok(read_wide(&Columns::new("blocks", row), 0))
                })?
        })
    }

    /// What the genesis block issued: the amounts of its Core Transfer transactions sent with the
    /// generator's key, `0` in a store without them.
    pub fn issued(&self) -> Result<u128, DatabaseError> {
        self.timed(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT json_extract(j.value, '$.amount') FROM transactions t \
                 JOIN blocks b ON b.id = t.block_id, json_each(t.asset, '$.transfers') j \
                 WHERE t.block_height = 1 AND t.type_group = 1 AND t.type = 6 \
                 AND t.sender_public_key = b.generator_public_key",
            )?;
            let mut rows = statement.query([])?;
            let mut sum: u128 = 0;
            while let Some(row) = rows.next()? {
                let amount = match row.get::<_, rusqlite::types::Value>(0)? {
                    rusqlite::types::Value::Text(text) => text.parse::<u128>().ok(),
                    rusqlite::types::Value::Integer(number) => u128::try_from(number).ok(),
                    _ => None,
                };
                sum = sum.saturating_add(amount.ok_or_else(|| DatabaseError::CorruptRow {
                    table: "transactions",
                    detail: "a genesis transfer has no whole amount".to_owned(),
                })?);
            }
            Ok(sum)
        })
    }
}

/// The two partial sums of `expression` (a non-negative integer) over the selected rows: its
/// high and low 32 bits, each of which fits SQLite's integers however many rows there are.
pub(super) fn wide_sum(expression: &str) -> String {
    format!("COALESCE(SUM(({expression}) >> 32), 0), COALESCE(SUM(({expression}) & 4294967295), 0)")
}

/// The sum that [`wide_sum`] selected in columns `index` and `index + 1`.
pub(super) fn read_wide(columns: &Columns<'_, '_>, index: usize) -> Result<u128, DatabaseError> {
    let high: u64 = columns.uint(index)?;
    let low: u64 = columns.uint(index + 1)?;
    Ok((u128::from(high) << 32) + u128::from(low))
}
