//! The reference's `getBlockProductivity`: each delegate's produced and missed blocks since the
//! start of the window, as its full outer join gives them, before any rounding.
//!
//! The reference rounds in PostgreSQL, `ROUND(produced / (produced + missed) * 100, 2)` over
//! `numeric`, which rounds half away from zero on the exact quotient; [`hundredths`] does the same
//! with integers, and [`round2`] rounds any decimal text so, so that no float is ever involved.

use super::fees::round_div;
use crate::errors::DatabaseError;
use crate::reader::Reader;

/// One delegate's counts in the window: the produced blocks and the missed slots, each `None` where
/// the join has no row on its side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductivityRow {
    /// The delegate's name.
    pub username: String,
    /// The blocks it produced in the window.
    pub produced: Option<u64>,
    /// The slots it missed in the window.
    pub missed: Option<u64>,
}

/// The registered names, each with the blocks it produced and the slots it missed since `?1`.
const QUERY: &str = "SELECT r.username, \
    (SELECT COUNT(*) FROM blocks b WHERE b.username = r.username AND b.timestamp >= ?1), \
    (SELECT COUNT(*) FROM missed_blocks m WHERE m.username = r.username AND m.timestamp >= ?1) \
    FROM (SELECT DISTINCT CAST(json_extract(asset, '$.delegate.username') AS TEXT) AS username \
          FROM transactions WHERE type_group = 1 AND type = 2) r \
    WHERE r.username IS NOT NULL ORDER BY r.username COLLATE BINARY";

/// A block of the window with no generator name (the genesis block): the reference's join keeps it
/// as a produced row of no name and no count, and its division of zero by zero fails the query.
const NAMELESS: &str =
    "SELECT EXISTS (SELECT 1 FROM blocks WHERE username IS NULL AND timestamp >= ?1)";

impl Reader<'_> {
    /// Every delegate's produced and missed counts since `since_epoch_seconds` (seconds since the
    /// network's epoch), by name.
    ///
    /// A block of the window without a generator name (the genesis block, while the window
    /// reaches back to it) fails the reference's query with a division by zero; here it is an
    /// error too. Blocks and missed slots of a name that no delegate registration holds are not
    /// listed: the reference's join gives them no name.
    pub fn productivity(
        &self,
        since_epoch_seconds: i64,
    ) -> Result<Vec<ProductivityRow>, DatabaseError> {
        let read = self.connection().unchecked_transaction()?;
        let nameless = self.timed(|connection| {
            let mut statement = connection.prepare_cached(NAMELESS)?;
            Ok(statement.query_row([since_epoch_seconds], |row| row.get::<_, bool>(0))?)
        })?;
        if nameless {
            return Err(DatabaseError::Sqlite("division by zero".to_owned()));
        }
        let rows = self.timed(|connection| {
            let mut statement = connection.prepare_cached(QUERY)?;
            let mut rows = statement.query([since_epoch_seconds])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                let columns = crate::values::Columns::new("blocks", row);
                let side = |index| -> Result<Option<u64>, DatabaseError> {
                    Ok(Some(columns.uint::<u64>(index)?).filter(|count| *count > 0))
                };
                out.push(ProductivityRow {
                    username: columns.get(0)?,
                    produced: side(1)?,
                    missed: side(2)?,
                });
            }
            Ok(out)
        })?;
        read.commit()?;
        Ok(rows)
    }
}

/// `ROUND(produced / (produced + missed) * 100, 2)` as a whole number of hundredths of a percent
/// (`9950` is `99.50`), rounded half away from zero on the exact quotient; `None` when both are
/// zero, where PostgreSQL's division fails.
pub fn hundredths(produced: u64, missed: u64) -> Option<u64> {
    let total = u128::from(produced) + u128::from(missed);
    let scaled = i128::try_from(u128::from(produced).checked_mul(10_000)?).ok()?;
    u64::try_from(round_div(scaled, total)?).ok()
}

/// `ROUND(value, 2)` of a decimal number written as `[-]digits[.digits]`, in PostgreSQL's text
/// (two decimals, no negative zero): halves round away from zero. `None` when `value` is not such
/// a number or has more than 30 digits.
pub fn round2(value: &str) -> Option<String> {
    let (negative, unsigned) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value.strip_prefix('+').unwrap_or(value)),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() && fraction.is_empty()
        || !digits(whole)
        || !digits(fraction)
        || whole.len() + fraction.len() > 30
    {
        return None;
    }
    let mut kept = format!("{whole}{}", fraction.get(..2).unwrap_or(fraction));
    for _ in fraction.len()..2 {
        kept.push('0');
    }
    let mut hundredths: u128 = kept.parse().ok()?;
    if fraction
        .as_bytes()
        .get(2)
        .is_some_and(|digit| *digit >= b'5')
    {
        hundredths += 1;
    }
    let sign = if negative && hundredths != 0 { "-" } else { "" };
    Some(format!(
        "{sign}{}.{:02}",
        hundredths / 100,
        hundredths % 100
    ))
}
