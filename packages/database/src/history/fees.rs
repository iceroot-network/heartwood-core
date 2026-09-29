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
                .collect::<Result<Vec<_>, _>>()?,
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
    let mut statement = connection.prepare_cached(&sql)?;
    let mut rows = statement.query([first, last])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let columns = Columns::new("transactions", row);
        let (sum, burned) = (read_wide(&columns, 5)?, read_wide(&columns, 7)?);
        out.push(row_of(
            columns.uint(0)?,
            columns.uint(1)?,
            columns.uint(2)?,
            (columns.uint(3)?, columns.uint(4)?),
            sum,
            burned,
        )?);
    }
    Ok(out)
}

/// The statistics of one type from its last 20 transactions, with its burned fees over all of
/// them; zeros for a type with none.
fn last_of(
    connection: &Connection,
    type_group: u32,
    kind: u16,
) -> Result<FeeStatisticsRow, DatabaseError> {
    let mut statement = connection.prepare_cached(
        "SELECT fee FROM transactions WHERE type_group = ?1 AND type = ?2 \
         ORDER BY block_height DESC, sequence DESC LIMIT ?3",
    )?;
    let mut rows = statement.query((type_group, kind, LAST))?;
    let mut fees: Vec<u64> = Vec::new();
    while let Some(row) = rows.next()? {
        fees.push(Columns::new("transactions", row).uint(0)?);
    }
    let count = u64::try_from(fees.len()).unwrap_or_default();
    let sum: u128 = fees.iter().map(|fee| u128::from(*fee)).sum();
    let extremes = (
        fees.iter().copied().min().unwrap_or_default(),
        fees.iter().copied().max().unwrap_or_default(),
    );
    let burned = if fees.is_empty() {
        0
    } else {
        let sql = format!(
            "SELECT {} FROM transactions WHERE type_group = ?1 AND type = ?2",
            wide_sum("burned_fee")
        );
        let mut statement = connection.prepare_cached(&sql)?;
        statement.query_row((type_group, kind), |row| {
            Ok(read_wide(&Columns::new("transactions", row), 0))
        })??
    };
    row_of(type_group, kind, count, extremes, sum, burned)
}

/// The row of `count` fees with the extremes `(min, max)`, `sum` and `burned`.
fn row_of(
    type_group: u32,
    kind: u16,
    count: u64,
    (min, max): (u64, u64),
    sum: u128,
    burned: u128,
) -> Result<FeeStatisticsRow, DatabaseError> {
    let avg = i128::try_from(sum)
        .ok()
        .and_then(|sum| avg_int8(sum, count))
        .and_then(|avg| u64::try_from(avg).ok())
        .unwrap_or_default();
    Ok(FeeStatisticsRow {
        type_group,
        kind,
        avg,
        min,
        max,
        sum,
        burned,
    })
}
