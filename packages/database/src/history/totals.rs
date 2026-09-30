//! The burned totals of `GET /blockchain`, over a range of heights, so that a node can grow them as
//! its tip grows.

use rusqlite::{Connection, Params};

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
        let sql = format!(
            "SELECT {}, {} FROM transactions WHERE block_height > ?1 AND block_height <= ?2",
            wide_sum("burned_fee"),
            wide_sum("CASE WHEN type_group = 2 AND type = 0 THEN amount ELSE 0 END"),
        );
        let [fees, transactions] =
            self.timed(|connection| wide_row(connection, "transactions", &sql, [above, to]))?;
        Ok(BurnedTotals { fees, transactions })
    }

    /// The rewards of the blocks at the heights above `above`, up to and with `to`.
    pub fn rewards(&self, above: u32, to: u32) -> Result<u128, DatabaseError> {
        let sql = format!(
            "SELECT {} FROM blocks WHERE height > ?1 AND height <= ?2",
            wide_sum("reward")
        );
        let [rewards] =
            self.timed(|connection| wide_row(connection, "blocks", &sql, [above, to]))?;
        Ok(rewards)
    }

    /// What the genesis block issued: the amounts of its Core Transfer transactions sent with the
    /// generator's key, `0` in a store without them.
    pub fn issued(&self) -> Result<u128, DatabaseError> {
        self.timed(|connection| {
            amount_sum(
                connection,
                "SELECT CAST(json_extract(j.value, '$.amount') AS TEXT) FROM transactions t \
                 JOIN blocks b ON b.id = t.block_id, json_each(t.asset, '$.transfers') j \
                 WHERE t.block_height = 1 AND t.type_group = 1 AND t.type = 6 \
                 AND t.sender_public_key = b.generator_public_key",
                [],
                "a genesis transfer",
            )
        })
    }
}

/// The sum of the whole amounts that `sql` selects as text, one per row in its first column; a
/// row with anything else is corrupt, and `what` names it.
pub(super) fn amount_sum(
    connection: &Connection,
    sql: &str,
    params: impl Params,
    what: &str,
) -> Result<u128, DatabaseError> {
    let mut statement = connection.prepare_cached(sql)?;
    let mut rows = statement.query(params)?;
    let mut sum: u128 = 0;
    while let Some(row) = rows.next()? {
        let amount = row
            .get::<_, Option<String>>(0)?
            .and_then(|text| text.parse::<u128>().ok());
        sum = sum.saturating_add(amount.ok_or_else(|| DatabaseError::CorruptRow {
            table: "transactions",
            detail: format!("{what} has no whole amount"),
        })?);
    }
    Ok(sum)
}

/// The `N` sums of the one row that `sql` selects, each written by [`wide_sum`] over `table`.
pub(super) fn wide_row<const N: usize>(
    connection: &Connection,
    table: &'static str,
    sql: &str,
    params: impl Params,
) -> Result<[u128; N], DatabaseError> {
    let read = |columns: &Columns<'_, '_>| -> Result<[u128; N], DatabaseError> {
        let mut sums = [0; N];
        for (index, sum) in sums.iter_mut().enumerate() {
            *sum = read_wide(columns, 2 * index)?;
        }
        Ok(sums)
    };
    connection
        .prepare_cached(sql)?
        .query_row(params, |row| Ok(read(&Columns::new(table, row))))?
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
