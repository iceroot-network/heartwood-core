//! The recipients side table, `transaction_recipients`: every recipient of a stored transaction,
//! so the API's address criteria find a transaction by any of its recipients as the reference's
//! containment query on the asset's transfers does.
//!
//! A chunk writes a transaction's recipients with its row ([`crate::TransactionRow::recipients`])
//! and a revert deletes them with it. A store of schema version 1 has no such table: opening it
//! creates the table ([`crate::connection::SCHEMA_2`]) and fills it here from the stored
//! transactions, in the same transaction as the version step.

use rusqlite::Connection;

use crate::errors::DatabaseError;

/// Fill the recipients side table from every stored transaction: the recipients of each transfer
/// (1/6) in its asset's order, each once per transaction, as a chunk would have written them.
pub(crate) fn upgrade(connection: &Connection) -> Result<(), DatabaseError> {
    let _ = connection;
    todo!()
}
