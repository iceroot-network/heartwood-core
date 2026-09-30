//! The fee statistics resource of `GET /node/fees`: the window of the `days` query, and the
//! statistics grouped by type group and keyed by the type's handler key.

use std::collections::BTreeMap;

use heartwood_crypto::TransactionType;
use heartwood_database::FeeStatisticsRow;
use serde_json::{Map, Value, json};

use heartwood_crypto::crypto::slots;

/// Milliseconds in a day.
const DAY_MS: i64 = 86_400_000;

/// The window of the last `days` days at `now_ms` (Unix milliseconds), as timestamps in seconds
/// since the network's epoch (`epoch`, its Unix milliseconds): from `now - days` on, with no end.
/// The reference asks only for `timestamp >= age`, so a block from the future still counts.
/// `None` when `days` is out of the range where the arithmetic is exact, which the query's schema
/// already refuses.
pub fn window(days: i64, now_ms: i64, epoch: i64) -> Option<(i64, i64)> {
    let age = now_ms.checked_sub(days.checked_mul(DAY_MS)?)?;
    Some((slots::epoch_time(age, epoch), i64::MAX))
}

/// The types this implementation carries, in the order of the reference's handler registry (the
/// order its statistics list them in, and its OpenAPI document's type list).
pub const REGISTERED: [TransactionType; 6] = [
    TransactionType::DelegateRegistration,
    TransactionType::DelegateResignation,
    TransactionType::Transfer,
    TransactionType::SecondSignature,
    TransactionType::Burn,
    TransactionType::Vote,
];

/// The rows of a query without a window in the order of the reference's handler registry, which
/// is the order it asks for them and lists them in; a row of a type that is not registered goes
/// last. A window's rows keep the store's order (by type group, then type).
pub fn in_registry_order(mut rows: Vec<FeeStatisticsRow>) -> Vec<FeeStatisticsRow> {
    let place = |row: &FeeStatisticsRow| {
        REGISTERED
            .iter()
            .position(|kind| kind.type_group() == row.type_group && kind.type_id() == row.kind)
            .unwrap_or(REGISTERED.len())
    };
    rows.sort_by_key(place);
    rows
}

/// The response of `GET /node/fees`: `{ meta, data }`, where `meta` holds `days` when the query
/// gave it, and `data` maps each type group (in ascending order, as a JavaScript object lists its
/// integer keys) to its types' `{ avg, burned, max, min, sum }`, every value a decimal string.
/// The types come in the order of the rows, a later row of a type replacing an earlier one where
/// that one stood, as the reference's assignments do. A row of a type this implementation does
/// not carry is left out.
pub fn response(days: Option<i64>, rows: &[FeeStatisticsRow]) -> Value {
    let mut groups: BTreeMap<u32, Map<String, Value>> = BTreeMap::new();
    for row in rows {
        let carried = REGISTERED
            .iter()
            .find(|kind| kind.type_group() == row.type_group && kind.type_id() == row.kind);
        if let Some(kind) = carried {
            groups
                .entry(kind.type_group())
                .or_default()
                .insert(kind.key().to_owned(), statistics(row));
        }
    }
    let groups: Map<String, Value> = groups
        .into_iter()
        .map(|(group, types)| (group.to_string(), Value::Object(types)))
        .collect();
    let meta = match days {
        Some(days) => json!({ "days": days }),
        None => json!({}),
    };
    json!({ "meta": meta, "data": groups })
}

/// One type's statistics.
fn statistics(row: &FeeStatisticsRow) -> Value {
    json!({
        "avg": row.avg.to_string(),
        "burned": row.burned.to_string(),
        "max": row.max.to_string(),
        "min": row.min.to_string(),
        "sum": row.sum.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(type_group: u32, kind: u16, fee: u64) -> FeeStatisticsRow {
        FeeStatisticsRow {
            type_group,
            kind,
            avg: fee,
            min: fee,
            max: fee,
            sum: u128::from(fee) * 2,
            burned: u128::from(fee) / 2,
        }
    }

    #[test]
    fn the_window_starts_days_before_now_and_has_no_end() {
        let epoch = 1_000_000;
        let now = epoch + 40 * DAY_MS + 999;
        assert_eq!(window(1, now, epoch), Some((39 * 86_400, i64::MAX)));
        assert_eq!(window(30, now, epoch), Some((10 * 86_400, i64::MAX)));
        // Before the epoch, the seconds are negative, as the reference's slot time is.
        assert_eq!(window(30, epoch, epoch), Some((-30 * 86_400, i64::MAX)));
        assert_eq!(window(i64::MAX, now, epoch), None);
    }

    #[test]
    fn the_statistics_group_by_type_group_and_key() {
        let rows = [row(1, 6, 10), row(1, 2, 20), row(2, 0, 30)];
        let out = response(Some(3), &rows);
        assert_eq!(out["meta"], json!({ "days": 3 }));
        assert_eq!(
            out["data"]["1"]["transfer"],
            json!({ "avg": "10", "burned": "5", "max": "10", "min": "10", "sum": "20" })
        );
        assert_eq!(out["data"]["1"]["delegateRegistration"]["avg"], "20");
        assert_eq!(out["data"]["2"]["burn"]["sum"], "60");
        assert_eq!(response(None, &rows)["meta"], json!({}));
    }

    #[test]
    fn the_types_follow_the_rows_and_a_later_row_replaces_an_earlier_one() {
        let rows = [
            row(2, 2, 1),
            row(2, 0, 1),
            row(1, 1, 1),
            row(1, 6, 1),
            row(1, 2, 1),
            row(1, 6, 7),
        ];
        let out = response(Some(1), &rows);
        let groups: Vec<&String> = out["data"].as_object().unwrap().keys().collect();
        assert_eq!(groups, ["1", "2"]);
        let types: Vec<&String> = out["data"]["1"].as_object().unwrap().keys().collect();
        assert_eq!(
            types,
            ["secondSignature", "transfer", "delegateRegistration"]
        );
        assert_eq!(out["data"]["1"]["transfer"]["avg"], "7");
    }

    #[test]
    fn a_query_without_a_window_lists_the_types_in_the_registry_order() {
        let rows = vec![
            row(2, 2, 1),
            row(1, 1, 1),
            row(2, 0, 1),
            row(1, 6, 1),
            row(1, 2, 1),
            row(1, 7, 1),
        ];
        let out = response(None, &in_registry_order(rows));
        let types: Vec<&String> = out["data"]["1"].as_object().unwrap().keys().collect();
        assert_eq!(
            types,
            [
                "delegateRegistration",
                "delegateResignation",
                "transfer",
                "secondSignature"
            ]
        );
        let types: Vec<&String> = out["data"]["2"].as_object().unwrap().keys().collect();
        assert_eq!(types, ["burn", "vote"]);
    }

    #[test]
    fn a_type_this_node_does_not_carry_is_left_out() {
        let out = response(None, &[row(1, 8, 1), row(3, 0, 1)]);
        assert_eq!(out["data"], json!({}));
    }
}
