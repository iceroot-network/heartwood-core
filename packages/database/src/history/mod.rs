//! The queries of the served routes, on a [`Reader`]: blocks by id or height, the transactions of
//! the criteria with the reference's orders and pages, a transaction by id, a block's
//! transactions, and a round's rows.
//!
//! A listing counts every match (`totalCount`) and reads the page in one read transaction, so both
//! see the same chain. Ties of the requested order are broken by the unique key (block height and
//! sequence), so a page is the same on every call. A statement interrupted after
//! [`crate::READ_TIMEOUT`] is a [`DatabaseError`], which the API answers with a logged 500.

use heartwood_crypto::{BlockId, TransactionId};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params_from_iter};

use crate::blocks::{self, BlockRow};
use crate::errors::DatabaseError;
use crate::filters::TransactionFilter;
use crate::filters::sql::{AMOUNT, Condition};
use crate::reader::Reader;
use crate::rounds::RoundRow;
use crate::transactions::{self, TransactionRow};
use crate::values::{Columns, int};

/// A block by its id or its height, as the API's `{id}` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSelector {
    /// A full block id.
    Id(BlockId),
    /// A height.
    Height(u32),
}

/// A direction of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Lowest first.
    Asc,
    /// Highest first.
    Desc,
}

/// One key of an order, by the reference's property name (`timestamp`, `sequence`, `amount`,
/// ...), and its direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// The property.
    pub property: String,
    /// The direction.
    pub direction: Direction,
}

/// A page: how many matches to skip, and how many to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// The matches skipped.
    pub offset: u64,
    /// The most matches returned.
    pub limit: u64,
}

/// A page of matches and their total count, as the reference's `ResultsPage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing<T> {
    /// The page's matches, in order.
    pub rows: Vec<T>,
    /// Every match's count.
    pub total_count: u64,
    /// Whether the count is an estimate (`CORE_API_ESTIMATED_TOTAL_COUNT`); never here, where
    /// every count is exact.
    pub total_count_is_estimate: bool,
}

impl<T> Listing<T> {
    /// A listing with no match.
    fn empty() -> Listing<T> {
        Listing {
            rows: Vec::new(),
            total_count: 0,
            total_count_is_estimate: false,
        }
    }
}

/// A stored transaction, as the transaction routes read it. Its row holds its block's id, height
/// and timestamp, and its recipients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRecord {
    /// The transaction's row, with its recipients.
    pub row: TransactionRow,
}

impl Reader<'_> {
    /// The rows of the blocks at `heights` that the store holds, in height order, each once.
    pub fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockRow>, DatabaseError> {
        let mut heights = heights.to_vec();
        heights.sort_unstable();
        heights.dedup();
        self.timed(|connection| {
            let sql = format!("SELECT {} FROM blocks WHERE height = ?1", blocks::COLUMNS);
            let mut statement = connection.prepare_cached(&sql)?;
            let mut out = Vec::with_capacity(heights.len());
            for height in heights {
                let row = statement
                    .query_row([height], |row| Ok(blocks::read_block(row)))
                    .optional()?;
                if let Some(row) = row {
                    out.push(row?);
                }
            }
            Ok(out)
        })
    }

    /// The row of the block `selector` names, if the store holds it.
    pub fn block(&self, selector: BlockSelector) -> Result<Option<BlockRow>, DatabaseError> {
        let (condition, value) = match selector {
            BlockSelector::Id(id) => ("id", SqlValue::Text(id.to_hex())),
            BlockSelector::Height(height) => ("height", SqlValue::Integer(height.into())),
        };
        self.timed(|connection| {
            let sql = format!(
                "SELECT {} FROM blocks WHERE {condition} = ?1",
                blocks::COLUMNS
            );
            let row = connection
                .prepare_cached(&sql)?
                .query_row([value], |row| Ok(blocks::read_block(row)))
                .optional()?;
            row.transpose()
        })
    }

    /// The page `page` of the transactions that `filter` matches, in the order `order`, whose
    /// ties the block height and the sequence break, in the direction of the order's last key
    /// (lowest first without one).
    ///
    /// Texts order by their bytes and `asset` by `jsonb`'s order; `NULL` comes last from the
    /// lowest and first from the highest, as in PostgreSQL, and the amount of a type without one
    /// is `NULL`, as in the reference's table. An order on a property that is no
    /// column (`address`) finds nothing, as the reference's repository answers an empty page
    /// when it cannot find the column.
    pub fn transactions(
        &self,
        filter: &TransactionFilter,
        order: &[Order],
        page: Page,
    ) -> Result<Listing<TransactionRecord>, DatabaseError> {
        let Some(order) = order_clause(order) else {
            return Ok(Listing::empty());
        };
        let condition = Condition::of(filter)?;
        let limit = int("page.limit", page.limit)?;
        let offset = int("page.offset", page.offset)?;
        let connection = self.connection();
        let read = connection.unchecked_transaction()?;
        let rows = self.timed(|connection| {
            let sql = format!(
                "SELECT {} FROM transactions WHERE {} ORDER BY {order} LIMIT ? OFFSET ?",
                transactions::COLUMNS,
                condition.sql
            );
            let params = condition
                .params
                .iter()
                .cloned()
                .chain([SqlValue::Integer(limit), SqlValue::Integer(offset)]);
            select_transactions(connection, &sql, params)
        })?;
        let total: i64 = self.timed(|connection| {
            let sql = format!("SELECT COUNT(*) FROM transactions WHERE {}", condition.sql);
            let mut statement = connection.prepare_cached(&sql)?;
            Ok(statement.query_row(params_from_iter(&condition.params), |row| row.get(0))?)
        })?;
        read.commit()?;
        Ok(Listing {
            rows,
            total_count: u64::try_from(total).unwrap_or_default(),
            total_count_is_estimate: false,
        })
    }

    /// The transaction with id `id`, if the store holds it.
    pub fn transaction(
        &self,
        id: &TransactionId,
    ) -> Result<Option<TransactionRecord>, DatabaseError> {
        self.timed(|connection| {
            let sql = format!(
                "SELECT {} FROM transactions WHERE id = ?1",
                transactions::COLUMNS
            );
            Ok(select_transactions(connection, &sql, [SqlValue::Text(id.to_hex())])?.pop())
        })
    }

    /// The transactions of the block `block`, by sequence.
    pub fn block_transactions(
        &self,
        block: &BlockId,
    ) -> Result<Vec<TransactionRecord>, DatabaseError> {
        self.timed(|connection| {
            let sql = format!(
                "SELECT {} FROM transactions WHERE block_id = ?1 ORDER BY sequence",
                transactions::COLUMNS
            );
            select_transactions(connection, &sql, [SqlValue::Text(block.to_hex())])
        })
    }

    /// The rows of round `round`, in the reference's `getRound` order: by balance from the
    /// highest, then by public key.
    pub fn round(&self, round: u64) -> Result<Vec<RoundRow>, DatabaseError> {
        let round = int("rounds.round", round)?;
        self.timed(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT round, public_key, balance FROM rounds WHERE round = ?1 \
                 ORDER BY balance DESC, public_key COLLATE BINARY ASC",
            )?;
            let mut rows = statement.query([round])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                let columns = Columns::new("rounds", row);
                out.push(RoundRow {
                    round: columns.uint(0)?,
                    public_key: columns.get(1)?,
                    balance: columns.uint(2)?,
                });
            }
            Ok(out)
        })
    }
}

/// The transactions that `sql`, a query of their columns, selects with `params`, each with its
/// recipients.
fn select_transactions(
    connection: &Connection,
    sql: &str,
    params: impl IntoIterator<Item = SqlValue>,
) -> Result<Vec<TransactionRecord>, DatabaseError> {
    let mut statement = connection.prepare_cached(sql)?;
    let mut rows = statement.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(transactions::read(row)?);
    }
    drop(rows);
    out.into_iter()
        .map(|mut row| {
            row.recipients = transactions::recipients(connection, &row.id)?;
            Ok(TransactionRecord { row })
        })
        .collect()
}

/// The `ORDER BY` of `order`, with the unique key after it; `None` when a property is no column.
fn order_clause(order: &[Order]) -> Option<String> {
    let mut keys = Vec::with_capacity(order.len() + 2);
    for key in order {
        let column = sort_column(&key.property)?;
        keys.push(match key.direction {
            Direction::Asc => format!("{column} ASC NULLS LAST"),
            Direction::Desc => format!("{column} DESC NULLS FIRST"),
        });
    }
    let tie = match order.last().map(|key| key.direction) {
        Some(Direction::Desc) => "DESC",
        Some(Direction::Asc) | None => "ASC",
    };
    keys.push(format!("block_height {tie}"));
    keys.push(format!("sequence {tie}"));
    Some(keys.join(", "))
}

/// The expression that sorts by the transaction property `property`, if it is a column.
fn sort_column(property: &str) -> Option<&'static str> {
    Some(match property {
        "id" => "id",
        "version" => "version",
        "blockId" => "block_id",
        "blockHeight" => "block_height",
        "sequence" => "sequence",
        "timestamp" => "timestamp",
        "nonce" => "nonce",
        "senderId" => "sender_id",
        "senderPublicKey" => "sender_public_key",
        "recipientId" => "recipient_id",
        "type" => "type",
        "typeGroup" => "type_group",
        "memo" => "memo",
        "amount" => AMOUNT,
        "fee" => "fee",
        "burnedFee" => "burned_fee",
        "serialised" => "serialised",
        "asset" => "heartwood_jsonb_order(asset)",
        _ => return None,
    })
}
