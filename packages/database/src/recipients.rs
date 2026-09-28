//! The recipients side table, `transaction_recipients`: every recipient of a stored transaction,
//! so the API's address criteria find a transaction by any of its recipients as the reference's
//! containment query on the asset's transfers does.
//!
//! A chunk writes a transaction's recipients with its row ([`crate::TransactionRow::recipients`])
//! and a revert deletes them with it. A store of schema version 1 has no such table: opening it
//! creates the table ([`crate::connection::SCHEMA_2`]) and fills it here from the stored
//! transactions, in the same transaction as the version step.

use rusqlite::{Connection, params};
use serde_json::Value;

use crate::errors::DatabaseError;
use crate::values::Columns;

/// Fill the recipients side table from every stored transaction: the recipients of each transfer
/// (1/6) in its asset's order, each once per transaction, as a chunk would have written them.
pub(crate) fn upgrade(connection: &Connection) -> Result<(), DatabaseError> {
    let mut select = connection.prepare(
        "SELECT id, asset FROM transactions WHERE type_group = 1 AND type = 6 \
         ORDER BY block_height, sequence",
    )?;
    let mut insert = connection.prepare(
        "INSERT OR IGNORE INTO transaction_recipients (transaction_id, recipient_id) \
         VALUES (?1, ?2)",
    )?;
    let mut rows = select.query([])?;
    while let Some(row) = rows.next()? {
        let columns = Columns::new("transactions", row);
        let id: String = columns.get(0)?;
        let asset: Option<String> = columns.get(1)?;
        for recipient in transfer_recipients(asset.as_deref())
            .map_err(|detail| columns.corrupt(1, format_args!("transaction {id}: {detail}")))?
        {
            insert.execute(params![id, recipient])?;
        }
    }
    Ok(())
}

/// The recipients of a transfer whose asset is the JSON text `asset`, in the order of its items.
fn transfer_recipients(asset: Option<&str>) -> Result<Vec<String>, String> {
    let asset: Value = serde_json::from_str(asset.ok_or("a transfer without an asset")?)
        .map_err(|error| error.to_string())?;
    let items = asset
        .get("transfers")
        .and_then(Value::as_array)
        .ok_or("the asset has no transfers")?;
    items
        .iter()
        .map(|item| {
            item.get("recipientId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "a transfer without a recipient".to_owned())
        })
        .collect()
}
