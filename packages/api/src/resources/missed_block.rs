//! The missed-block resource of `GET /blocks/missed` and `GET /delegates/{id}/blocks/missed`: the
//! height of the slot, its time as `{epoch, unix, human}`, and the delegate that missed it.

use heartwood_crypto::Milestones;
use heartwood_database::MissedRow;
use heartwood_kernel::timestamp::format_timestamp;
use serde_json::{Value, json};

/// The missed slot `row` as the routes show it; the time is `null` when a JavaScript date cannot
/// hold it.
pub fn missed_block(row: &MissedRow, milestones: &Milestones) -> Value {
    let timestamp = match format_timestamp(row.timestamp, milestones.at(1).epoch()) {
        Ok(stamp) => json!({ "epoch": stamp.epoch, "unix": stamp.unix, "human": stamp.human }),
        Err(_) => Value::Null,
    };
    json!({ "height": row.height, "timestamp": timestamp, "username": row.username })
}
