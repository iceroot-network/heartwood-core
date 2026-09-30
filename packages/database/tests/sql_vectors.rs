//! The SQL records of vector class V34, recorded from the reference's PostgreSQL: `sql.round2`
//! (`ROUND(numeric, 2)`) against [`round2`], `sql.avgInt8` (`AVG(bigint)::int8`) against
//! [`avg_int8`], and `sql.productivity` (the productivity query over generated row sets) against
//! the store's own reader over a scratch store holding those rows, rounded by [`hundredths`].
//!
//! The class is read from `tests/vectors`, or from the directory in `DATABASE_VECTORS` (a relative
//! path is taken from the workspace root), such as a local oracle run in `target/vectors/local`.
//! The crate's directory must hold the class with each of the three kinds at least at its minimum
//! (`tools/oracle/vector-minimums.json`), so a missing class or a short kind fails the test; a
//! directory named in the environment may lack the class, and the test then says so and passes.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{block_row, open, transaction_row};
use heartwood_database::{MissedRow, Readers, TransactionRow, avg_int8, hundredths, round2};
use serde_json::{Value, json};

const CLASS: &str = "V34-api-tiers";
const KINDS: [&str; 3] = ["sql.round2", "sql.avgInt8", "sql.productivity"];

/// The store keys blocks and missed slots by their timestamp, so each stored row gets its own: a
/// group's timestamp `t` becomes `(t + 1) × SPREAD` plus the row's place among the rows of that
/// timestamp, and the window's start `since` becomes `(since + 1) × SPREAD`. The block that
/// carries the registrations is stamped 0, before every window.
const SPREAD: i64 = 100_000;

/// The workspace root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The class's SQL records by kind, or `None` when the directory `DATABASE_VECTORS` names lacks
/// the class. The crate's own directory must hold it.
fn records() -> Option<BTreeMap<String, Vec<Value>>> {
    let named = std::env::var_os("DATABASE_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors"),
        |dir| root().join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    let mut by_kind: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for record in lines.filter(|record| KINDS.contains(&record["op"].as_str().unwrap_or(""))) {
        let kind = record["op"].as_str().unwrap().to_owned();
        by_kind.entry(kind).or_default().push(record);
    }
    Some(by_kind)
}

/// `timestamp` spread as [`SPREAD`] says, for the row at `place`.
fn spread(timestamp: i64, place: u64) -> u32 {
    let spread = (timestamp + 1) * SPREAD + i64::try_from(place).unwrap();
    u32::try_from(spread).unwrap()
}

/// The answer of the store to a `sql.productivity` input: its rows, or the error class of a
/// division by zero.
fn productivity(name: &str, input: &Value) -> Value {
    let (mut database, settings) = open(name);
    let registered: Vec<&str> = (input["registered"].as_array().unwrap().iter())
        .map(|name| name.as_str().unwrap())
        .collect();
    let registrations: Vec<TransactionRow> = (0u32..)
        .zip(&registered)
        .map(|(sequence, name)| {
            let mut row = transaction_row(1, sequence);
            (row.type_group, row.kind, row.amount, row.fee, row.timestamp) = (1, 2, 0, 0, 0);
            row.recipient_id = None;
            row.asset = Some(json!({"delegate": {"username": name}}).to_string());
            row
        })
        .collect();
    let mut first = block_row(1, u32::try_from(registrations.len()).unwrap());
    (first.timestamp, first.username) = (0, None);
    let mut blocks = vec![(first, registrations)];
    let mut missed = Vec::new();
    let mut places: BTreeMap<(bool, i64), u64> = BTreeMap::new();
    let groups = |key: &str| input[key].as_array().unwrap().clone();
    let (produced, slots) = (groups("blocks"), groups("missed"));
    let all = (produced.iter().map(|g| (true, g))).chain(slots.iter().map(|g| (false, g)));
    for (is_block, group) in all {
        let (username, timestamp) = (group["username"].as_str(), group["timestamp"].as_i64());
        for _ in 0..group["count"].as_u64().unwrap() {
            let place = places.entry((is_block, timestamp.unwrap())).or_default();
            let timestamp = spread(timestamp.unwrap(), *place);
            *place += 1;
            match (is_block, username) {
                (true, name) => {
                    let mut block = block_row(u32::try_from(blocks.len() + 1).unwrap(), 0);
                    (block.timestamp, block.username) = (timestamp, name.map(str::to_owned));
                    blocks.push((block, Vec::new()));
                }
                (false, Some(name)) => missed.push(MissedRow {
                    timestamp: i64::from(timestamp),
                    height: 1,
                    username: name.to_owned(),
                }),
                // The store, as the reference's table, has no missed slot without a name.
                (false, None) => {}
            }
        }
    }
    let mut chunk = database.begin().unwrap();
    for (block, transactions) in &blocks {
        chunk.save_block(block, transactions).unwrap();
    }
    chunk.save_missed(&missed).unwrap();
    chunk.commit().unwrap();
    let readers = Readers::open(&settings, 1).unwrap();
    let since = i64::from(spread(input["since"].as_i64().unwrap(), 0));
    let Ok(counted) = readers.get().productivity(since) else {
        return json!({"error": "22012"});
    };
    let percent = |hundredths: u64| format!("{}.{:02}", hundredths / 100, hundredths % 100);
    let rows: Vec<Value> = (counted.into_iter())
        .map(|row| {
            let productivity = match (row.produced, row.missed) {
                (None, None) => None,
                (None, Some(_)) => Some("0.00".to_owned()),
                (Some(produced), missed) => hundredths(produced, missed.unwrap_or(0)).map(percent),
            };
            json!({"username": row.username, "missed": row.missed, "productivity": productivity})
        })
        .collect();
    json!({ "rows": rows })
}

#[test]
fn the_sql_records_are_the_store_s_answers() {
    let Some(records) = records() else {
        eprintln!("the directory DATABASE_VECTORS names has no {CLASS}");
        return;
    };
    let minimums: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tools/oracle/vector-minimums.json")).unwrap(),
    )
    .unwrap();
    for kind in KINDS {
        let minimum = usize::try_from(minimums[CLASS][kind].as_u64().unwrap()).unwrap();
        let count = records.get(kind).map_or(0, Vec::len);
        assert!(
            count >= minimum,
            "{kind}: {count} records, the minimum is {minimum}"
        );
    }
    for record in &records["sql.round2"] {
        let value = record["input"]["value"].as_str().unwrap();
        assert_eq!(
            json!({ "value": round2(value) }),
            record["output"],
            "{value}"
        );
    }
    for record in &records["sql.avgInt8"] {
        let values: Vec<i64> = (record["input"]["values"].as_array().unwrap().iter())
            .map(|value| value.as_str().unwrap().parse().unwrap())
            .collect();
        let sum: i128 = values.iter().map(|value| i128::from(*value)).sum();
        let average = avg_int8(sum, u64::try_from(values.len()).unwrap()).map(|a| a.to_string());
        assert_eq!(json!({ "value": average }), record["output"], "{values:?}");
    }
    for (n, record) in records["sql.productivity"].iter().enumerate() {
        let expected = match &record["error"] {
            Value::Null => record["output"].clone(),
            error => json!({ "error": error["class"] }),
        };
        let answer = productivity(&format!("sql-productivity-{n}"), &record["input"]);
        assert_eq!(answer, expected, "{}", record["input"]);
    }
}
