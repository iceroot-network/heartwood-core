//! The fee statistics of `GET /node/fees`.
//!
//! The reference's `AVG(fee)::int8` rounds half away from zero; [`avg_int8`] does the same with
//! integers, and a sum is read as two partial sums so that it never overflows SQLite's integers.

use heartwood_crypto::TransactionType;
use rusqlite::Connection;

use super::totals::{read_wide, wide_sum};
use crate::errors::DatabaseError;
use crate::reader::Reader;
use crate::values::Columns;

/// The most transactions of a type that its statistics read when no window is given.
const LAST: i64 = 20;

/// The fees of one (type group, type) as the reference's query aggregates them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeStatisticsRow {
    /// The type group.
    pub type_group: u32,
    /// The type.
    pub kind: u16,
    /// `AVG(fee)::int8`.
    pub avg: u64,
    /// The lowest fee.
    pub min: u64,
    /// The highest fee.
    pub max: u64,
    /// The fees' sum.
    pub sum: u128,
    /// The burned fees' sum.
    pub burned: u128,
}

/// `numerator / denominator` rounded half away from zero, as PostgreSQL rounds a `numeric` when
/// it is cast to `int8` or given to `ROUND`; `None` for a zero denominator or a result that does
/// not fit.
pub fn round_div(numerator: i128, denominator: u128) -> Option<i128> {
    let denominator = i128::try_from(denominator).ok().filter(|d| *d > 0)?;
    let (quotient, remainder) = (numerator / denominator, numerator % denominator);
    // The remainder has the numerator's sign; a half or more of the denominator rounds away.
    let doubled = remainder.checked_abs()?.checked_mul(2)?;
    Some(match doubled >= denominator {
        true if numerator < 0 => quotient.checked_sub(1)?,
        true => quotient.checked_add(1)?,
        false => quotient,
    })
}

/// `AVG(x)::int8` of `count` values of `x` whose sum is `sum`; `None` for no values.
pub fn avg_int8(sum: i128, count: u64) -> Option<i64> {
    i64::try_from(round_div(sum, u128::from(count))?).ok()
}

impl Reader<'_> {
    /// With a window (its first and last timestamp, since the network's epoch): one row per
    /// (type group, type) present in it, ordered by type group and type. Without: one row per kept
    /// type from its last 20 transactions, zeros where it has none, with its burned sum over
    /// every block.
    pub fn fee_statistics(
        &self,
        window: Option<(i64, i64)>,
    ) -> Result<Vec<FeeStatisticsRow>, DatabaseError> {
        let read = self.connection().unchecked_transaction()?;
        let rows = match window {
            Some((first, last)) => self.timed(|c| in_window(c, first, last))?,
            None => TransactionType::ALL
                .iter()
                .map(|kind| self.timed(|c| last_of(c, kind.type_group(), kind.type_id())))
                .collect::<Result<Vec<_>, _>>()?
                .concat(),
        };
        read.commit()?;
        Ok(rows)
    }
}

/// The statistics of every type with a transaction whose timestamp is in the window.
fn in_window(
    connection: &Connection,
    first: i64,
    last: i64,
) -> Result<Vec<FeeStatisticsRow>, DatabaseError> {
    let sql = format!(
        "SELECT type_group, type, COUNT(*), MIN(fee), MAX(fee), {}, {} FROM transactions \
         WHERE timestamp BETWEEN ?1 AND ?2 GROUP BY type_group, type ORDER BY type_group, type",
        wide_sum("fee"),
        wide_sum("burned_fee")
    );
    rows_of(connection, &sql, [first, last])
}

/// The statistics of one type from its last 20 transactions, with its burned fees over all of
/// them; zeros for a type with none.
fn last_of(
    connection: &Connection,
    type_group: u32,
    kind: u16,
) -> Result<Vec<FeeStatisticsRow>, DatabaseError> {
    let burned = |sum: &str| {
        format!(
            "(SELECT COALESCE(SUM(burned_fee {sum}), 0) FROM transactions \
             WHERE type_group = ?1 AND type = ?2)"
        )
    };
    let sql = format!(
        "SELECT ?1, ?2, COUNT(*), COALESCE(MIN(fee), 0), COALESCE(MAX(fee), 0), {}, {}, {} \
         FROM (SELECT fee FROM transactions WHERE type_group = ?1 AND type = ?2 \
               ORDER BY block_height DESC, sequence DESC LIMIT ?3)",
        wide_sum("fee"),
        burned(">> 32"),
        burned("& 4294967295")
    );
    rows_of(connection, &sql, (type_group, kind, LAST))
}

/// The rows that `sql` selects: the type group, the type, the count, the lowest and highest fee,
/// the fees' sum and the burned fees' sum, each sum as [`wide_sum`] selects it.
fn rows_of(
    connection: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<FeeStatisticsRow>, DatabaseError> {
    let mut statement = connection.prepare_cached(sql)?;
    let mut rows = statement.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_of(&Columns::new("transactions", row))?);
    }
    Ok(out)
}

/// The row that `rows_of` reads from `columns`.
fn row_of(columns: &Columns<'_, '_>) -> Result<FeeStatisticsRow, DatabaseError> {
    let (sum, count): (u128, u64) = (read_wide(columns, 5)?, columns.uint(2)?);
    let avg = i128::try_from(sum)
        .ok()
        .and_then(|sum| avg_int8(sum, count))
        .and_then(|avg| u64::try_from(avg).ok())
        .unwrap_or_default();
    Ok(FeeStatisticsRow {
        type_group: columns.uint(0)?,
        kind: columns.uint(1)?,
        avg,
        min: columns.uint(3)?,
        max: columns.uint(4)?,
        sum,
        burned: read_wide(columns, 7)?,
    })
}
