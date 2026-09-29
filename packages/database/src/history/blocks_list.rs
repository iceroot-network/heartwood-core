//! The block listing of `GET /blocks` and `GET /delegates/{id}/blocks`: the blocks of the block
//! criteria with the reference's orders and pages, and, for a transformed answer, the amounts of
//! each block's Core Transfer payments, which the resource adds to `forged.amount`.

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Row, params_from_iter};

use super::totals::amount_sum;
use super::{Direction, Listing, Order, Page};
use crate::blocks::{self, BlockRow};
use crate::errors::DatabaseError;
use crate::filters::BlockFilter;
use crate::filters::sql::Condition;
use crate::reader::Reader;
use crate::values::int;

/// The value of `totalCountIsEstimate` the reference reports with a listing: the count itself is
/// always exact here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountFlag {
    /// The reference reports an estimate (`true`).
    Estimate,
    /// The reference reports an exact count (`false`).
    Exact,
}

/// A listed block: its row, and the sum of its Core Transfer payments when the listing was asked
/// for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedBlock {
    /// The block's row.
    pub row: BlockRow,
    /// The amounts of the block's Core Transfer payments, summed; `None` unless asked for.
    pub transferred: Option<u128>,
}

impl Reader<'_> {
    /// The page `page` of the blocks that `filter` matches, in the order `order`, with every
    /// match's count and `count` as its estimate flag; `transfers` also sums each block's Core
    /// Transfer payments.
    ///
    /// Ties of the order are broken by the height, in the direction of the order's last key. An
    /// order on a property that is no column finds nothing, as the reference's repository
    /// answers an empty page when it cannot find the column.
    pub fn blocks(
        &self,
        filter: &BlockFilter,
        order: &[Order],
        page: Page,
        transfers: bool,
        count: CountFlag,
    ) -> Result<Listing<ListedBlock>, DatabaseError> {
        let estimate = count == CountFlag::Estimate;
        let Some(order) = order_by(order, block_column, "height") else {
            return Ok(Listing {
                total_count_is_estimate: estimate,
                ..Listing::empty()
            });
        };
        let condition = filter.condition()?;
        let connection = self.connection();
        let read = connection.unchecked_transaction()?;
        let (rows, total_count) = self.page(
            ("blocks", blocks::COLUMNS),
            (&condition, &order, page),
            blocks::read_block,
        )?;
        let rows = self.timed(|connection| {
            rows.into_iter()
                .map(|row| {
                    let transferred = transfers
                        .then(|| transferred(connection, row.height))
                        .transpose()?;
                    Ok(ListedBlock { row, transferred })
                })
                .collect::<Result<Vec<_>, DatabaseError>>()
        })?;
        read.commit()?;
        Ok(Listing {
            rows,
            total_count,
            total_count_is_estimate: estimate,
        })
    }
}

impl Reader<'_> {
    /// The rows of `table` that `condition` matches, `columns` of them in the order `order`, the
    /// page `page`, read with `read`, and every match's count. It runs two statements: the caller
    /// wraps them in one read transaction.
    pub(super) fn page<T>(
        &self,
        (table, columns): (&str, &str),
        (condition, order, page): (&Condition, &str, Page),
        read: impl Fn(&Row<'_>) -> Result<T, DatabaseError>,
    ) -> Result<(Vec<T>, u64), DatabaseError> {
        let limit = int("page.limit", page.limit)?;
        let offset = int("page.offset", page.offset)?;
        let rows = self.timed(|connection| {
            let sql = format!(
                "SELECT {columns} FROM {table} WHERE {} ORDER BY {order} LIMIT ? OFFSET ?",
                condition.sql
            );
            let params = (condition.params.iter().cloned())
                .chain([SqlValue::Integer(limit), SqlValue::Integer(offset)]);
            let mut statement = connection.prepare_cached(&sql)?;
            let mut rows = statement.query(params_from_iter(params))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(read(row)?);
            }
            Ok(out)
        })?;
        let total: i64 = self.timed(|connection| {
            let sql = format!("SELECT COUNT(*) FROM {table} WHERE {}", condition.sql);
            let mut statement = connection.prepare_cached(&sql)?;
            Ok(statement.query_row(params_from_iter(&condition.params), |row| row.get(0))?)
        })?;
        Ok((rows, u64::try_from(total).unwrap_or_default()))
    }
}

/// The `ORDER BY` terms of `order`, whose properties `column` names, with `tie` after them in the
/// direction of the last key (ascending without one); `None` when a property is no column.
pub(super) fn order_by(
    order: &[Order],
    column: fn(&str) -> Option<&'static str>,
    tie: &str,
) -> Option<String> {
    let mut keys = Vec::with_capacity(order.len() + 1);
    for key in order {
        let column = column(&key.property)?;
        keys.push(match key.direction {
            Direction::Asc => format!("{column} ASC NULLS LAST"),
            Direction::Desc => format!("{column} DESC NULLS FIRST"),
        });
    }
    let direction = match order.last().map(|key| key.direction) {
        Some(Direction::Desc) => "DESC",
        Some(Direction::Asc) | None => "ASC",
    };
    keys.push(format!("{tie} {direction}"));
    Some(keys.join(", "))
}

/// The sort expression of the block property `property`, if it is a column.
fn block_column(property: &str) -> Option<&'static str> {
    Some(match property {
        "id" => "id",
        "version" => "version",
        "timestamp" => "timestamp",
        "previousBlock" => "previous_block",
        "height" => "height",
        "numberOfTransactions" => "number_of_transactions",
        "totalAmount" => "total_amount",
        "totalFee" => "total_fee",
        "burnedFee" => "burned_fee",
        "reward" => "reward",
        "donations" => "heartwood_jsonb_order(donations)",
        "payloadLength" => "payload_length",
        "payloadHash" => "payload_hash",
        "generatorPublicKey" => "generator_public_key",
        "username" => "username",
        "blockSignature" => "block_signature",
        _ => return None,
    })
}

/// The sum of the amounts in the `asset.transfers` of the Core Transfer transactions (type group
/// 1, type 6) of the block at `height`.
fn transferred(connection: &Connection, height: u32) -> Result<u128, DatabaseError> {
    amount_sum(
        connection,
        "SELECT CAST(json_extract(j.value, '$.amount') AS TEXT) FROM transactions t, \
         json_each(t.asset, '$.transfers') j \
         WHERE t.block_height = ?1 AND t.type_group = 1 AND t.type = 6",
        [height],
        &format!("a transfer of block {height}"),
    )
}
