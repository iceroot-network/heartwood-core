//! Conversions between the values of the rows and SQLite's column values.
//!
//! SQLite stores signed 64-bit integers. Every unsigned value is written through [`int`], which
//! refuses a value above `i64::MAX` ([`DatabaseError::Range`]), and read back through
//! [`Columns`], which refuses a negative or out-of-range value, a value of another type, and an
//! id or digest that is not hex of its length ([`DatabaseError::CorruptRow`]).

use heartwood_crypto::utils::hex;
use heartwood_crypto::{BlockId, TransactionId};
use rusqlite::Row;
use rusqlite::types::FromSql;

use crate::errors::DatabaseError;

/// `value` as the signed 64-bit integer that SQLite stores; `column` names it in the error.
pub(crate) fn int(column: &'static str, value: u64) -> Result<i64, DatabaseError> {
    i64::try_from(value).map_err(|_| DatabaseError::Range {
        column,
        value: u128::from(value),
    })
}

/// The columns of one row of `table`, read with the types of the row structs.
pub(crate) struct Columns<'r, 's> {
    table: &'static str,
    row: &'r Row<'s>,
}

impl<'r, 's> Columns<'r, 's> {
    /// The columns of `row`, a row of `table`.
    pub(crate) fn new(table: &'static str, row: &'r Row<'s>) -> Columns<'r, 's> {
        Columns { table, row }
    }

    /// A row of this table that cannot be read.
    pub(crate) fn corrupt(&self, index: usize, detail: impl std::fmt::Display) -> DatabaseError {
        let name = self.row.as_ref().column_name(index).unwrap_or("?");
        DatabaseError::CorruptRow {
            table: self.table,
            detail: format!("{name}: {detail}"),
        }
    }

    /// Column `index` as a value of exactly its SQLite type; another type is a corrupt row.
    pub(crate) fn get<T: FromSql>(&self, index: usize) -> Result<T, DatabaseError> {
        self.row.get(index).map_err(|error| match error {
            rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
            | rusqlite::Error::Utf8Error(..) => self.corrupt(index, error),
            other => DatabaseError::from(other),
        })
    }

    /// An integer column read as `T`, which must hold it.
    pub(crate) fn uint<T: TryFrom<i64>>(&self, index: usize) -> Result<T, DatabaseError> {
        let value: i64 = self.get(index)?;
        T::try_from(value).map_err(|_| self.corrupt(index, format_args!("{value} is out of range")))
    }

    /// A nullable text column of a hex id, digest or key of `N` bytes.
    fn bytes<const N: usize>(&self, index: usize) -> Result<Option<[u8; N]>, DatabaseError> {
        let Some(text) = self.get::<Option<String>>(index)? else {
            return Ok(None);
        };
        hex::decode_array(&text)
            .map(Some)
            .map_err(|error| self.corrupt(index, error))
    }

    /// A text column of a hex digest of 32 bytes.
    pub(crate) fn digest(&self, index: usize) -> Result<[u8; 32], DatabaseError> {
        self.bytes(index)?
            .ok_or_else(|| self.corrupt(index, "the value is NULL"))
    }

    /// A nullable text column of a block id.
    pub(crate) fn block_id_or_null(&self, index: usize) -> Result<Option<BlockId>, DatabaseError> {
        Ok(self.bytes(index)?.map(BlockId::from_array))
    }

    /// A text column of a block id.
    pub(crate) fn block_id(&self, index: usize) -> Result<BlockId, DatabaseError> {
        self.digest(index).map(BlockId::from_array)
    }

    /// A text column of a transaction id.
    pub(crate) fn transaction_id(&self, index: usize) -> Result<TransactionId, DatabaseError> {
        self.digest(index).map(TransactionId::from_array)
    }
}
