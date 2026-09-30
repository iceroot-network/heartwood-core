//! The SQL records of the store's arithmetic, recorded from the reference's PostgreSQL:
//! `sql.round2` (`ROUND(numeric, 2)`), `sql.avgInt8` (`AVG(bigint)::int8`), `sql.productivity`
//! (the productivity query over generated row sets, answered by the store's own reader over a
//! scratch store holding those rows, and rounded by the store's own functions) and `sql.meta`
//! (the recording's provenance); and `api.productivityWindow`, the first second of the window the
//! reference's `updateProductivity` asks that query for, answered by the served table's own
//! [`window_start`].

use heartwood_api::productivity::window_start;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::{BlockId, TransactionId};
use heartwood_database::{
    BlockRow, Database, DatabaseError, MissedRow, Readers, Settings, TransactionRow, avg_int8,
    hundredths, round2,
};
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, rejected, text, unclassified};
use crate::store::ScratchDir;
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("sql.round2", round2_op),
    ("sql.avgInt8", avg_int8_op),
    ("sql.productivity", productivity),
    ("sql.meta", meta),
    ("api.productivityWindow", productivity_window),
];

/// PostgreSQL's error class for a division by zero.
const DIVISION_BY_ZERO: &str = "22012";

fn round2_op(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let value = text(case.input, "value")?;
    Ok(match round2(value) {
        Some(rounded) => Verdict::Output(json!({ "value": rounded })),
        None => unclassified(format!("not a decimal of at most 30 digits: {value}")),
    })
}

fn avg_int8_op(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let values = case.input["values"]
        .as_array()
        .ok_or_else(|| failed("input.values is not a list"))?;
    let mut sum: i128 = 0;
    for value in values {
        let number: i64 = value
            .as_str()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| failed("input.values holds a value that is not an int8 in text"))?;
        sum += i128::from(number);
    }
    let average = avg_int8(sum, u64::try_from(values.len()).unwrap_or(u64::MAX))
        .map(|average| average.to_string());
    Ok(Verdict::Output(json!({ "value": average })))
}

/// One group of rows of a case: `count` rows of one name at one timestamp.
struct Group<'a> {
    name: Option<&'a str>,
    timestamp: i64,
    count: u64,
}

fn groups<'a>(input: &'a Value, key: &str) -> Outcome<Vec<Group<'a>>> {
    let list = input[key]
        .as_array()
        .ok_or_else(|| failed(format!("input.{key} is not a list")))?;
    list.iter()
        .map(|group| {
            Ok(Group {
                name: group["username"].as_str(),
                timestamp: group["timestamp"]
                    .as_i64()
                    .ok_or_else(|| failed(format!("input.{key}: a group has no timestamp")))?,
                count: group["count"]
                    .as_u64()
                    .ok_or_else(|| failed(format!("input.{key}: a group has no count")))?,
            })
        })
        .collect()
}

/// The store keys blocks and missed slots by their timestamp, so each stored row gets its own: a
/// group's timestamp `t` becomes `(t + 1) × SPREAD` plus the row's place among the rows of that
/// timestamp, and the window's start `since` becomes `(since + 1) × SPREAD`, which keeps every
/// row on its side of the start. The block that carries the registrations is stamped 0, before
/// every window.
const SPREAD: i64 = 100_000;

/// `timestamp` spread as [`SPREAD`] says, for the row at `place`.
fn spread(timestamp: i64, place: u64) -> Outcome<i64> {
    timestamp
        .checked_add(1)
        .and_then(|t| t.checked_mul(SPREAD))
        .zip(i64::try_from(place).ok().filter(|place| *place < SPREAD))
        .map(|(t, place)| t + place)
        .ok_or_else(|| failed("a timestamp or a count is too large to store"))
}

/// A scratch store holding the case's registrations, blocks and missed slots.
struct Rows<'a> {
    database: Database,
    blocks: Vec<(BlockRow, Vec<TransactionRow>)>,
    missed: Vec<MissedRow>,
    registered: &'a [&'a str],
}

impl Rows<'_> {
    /// Add a block stamped `timestamp` of `name`, with the registrations if it is the first.
    fn block(&mut self, timestamp: i64, name: Option<&str>) -> Outcome<()> {
        let height = u32::try_from(self.blocks.len() + 1).map_err(|_| failed("too many blocks"))?;
        let id = |height: u32| {
            let mut bytes = [0; 32];
            bytes[..4].copy_from_slice(&height.to_be_bytes());
            BlockId::from_array(bytes)
        };
        let registrations = if height == 1 { self.registered } else { &[] };
        let transactions = (0..)
            .zip(registrations)
            .map(|(sequence, name)| {
                let mut bytes = [0x77; 32];
                bytes[..4].copy_from_slice(&u32::to_be_bytes(sequence));
                TransactionRow {
                    id: TransactionId::from_array(bytes),
                    version: 3,
                    block_id: id(height),
                    block_height: height,
                    sequence,
                    timestamp: 0,
                    nonce: u64::from(sequence) + 1,
                    sender_id: "sender".into(),
                    sender_public_key: format!("03{sequence:064x}"),
                    recipient_id: None,
                    kind: 2,
                    type_group: 1,
                    memo: None,
                    amount: 0,
                    fee: 0,
                    burned_fee: 0,
                    serialised: vec![0],
                    asset: Some(json!({"delegate": {"username": name}}).to_string()),
                    recipients: Vec::new(),
                }
            })
            .collect::<Vec<_>>();
        let row = BlockRow {
            id: id(height),
            height,
            version: 0,
            timestamp: u32::try_from(timestamp).map_err(|_| failed("a timestamp is too large"))?,
            previous_block: (height > 1).then(|| id(height - 1)),
            number_of_transactions: u32::try_from(transactions.len())
                .map_err(|_| failed("too many registrations"))?,
            total_amount: 0,
            total_fee: 0,
            burned_fee: 0,
            reward: 0,
            donations: "{}".into(),
            payload_length: 0,
            payload_hash: "00".repeat(32),
            generator_public_key: format!("02{height:064x}"),
            username: name.map(str::to_owned),
            block_signature: "00".repeat(64),
        };
        self.blocks.push((row, transactions));
        Ok(())
    }

    /// Store every row.
    fn save(&mut self) -> Result<(), DatabaseError> {
        let mut chunk = self.database.begin()?;
        for (block, transactions) in &self.blocks {
            chunk.save_block(block, transactions)?;
        }
        chunk.save_missed(&self.missed)?;
        chunk.commit()
    }
}

/// `hundredths` of a percent as PostgreSQL writes a `numeric` of two decimals.
fn percent(hundredths: u64) -> String {
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

fn productivity(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let since = case.input["since"]
        .as_i64()
        .ok_or_else(|| failed("input.since is not an integer"))?;
    let registered: Vec<&str> = case.input["registered"]
        .as_array()
        .ok_or_else(|| failed("input.registered is not a list"))?
        .iter()
        .map(|name| {
            name.as_str()
                .ok_or_else(|| failed("a registered name is not text"))
        })
        .collect::<Outcome<_>>()?;
    let blocks = groups(case.input, "blocks")?;
    let missed = groups(case.input, "missed")?;

    let database_error = |error: DatabaseError| failed(format!("the scratch store: {error}"));
    let dir = ScratchDir::new("sql-productivity").map_err(|e| failed(e.to_string()))?;
    let settings = Settings::in_data_dir(dir.path());
    let mut rows = Rows {
        database: Database::open(&settings).map_err(database_error)?,
        blocks: Vec::new(),
        missed: Vec::new(),
        registered: &registered,
    };
    rows.block(0, None)?;
    let mut places = std::collections::HashMap::<(bool, i64), u64>::new();
    for (is_block, group) in
        (blocks.iter().map(|g| (true, g))).chain(missed.iter().map(|g| (false, g)))
    {
        for _ in 0..group.count {
            let place = places.entry((is_block, group.timestamp)).or_default();
            let timestamp = spread(group.timestamp, *place)?;
            *place += 1;
            match (is_block, group.name) {
                (true, name) => rows.block(timestamp, name)?,
                (false, Some(name)) => rows.missed.push(MissedRow {
                    timestamp,
                    height: 1,
                    username: name.to_owned(),
                }),
                // The store, as the reference's table, has no missed slot without a name; a
                // nameless slot counts for no delegate.
                (false, None) => {}
            }
        }
    }
    rows.save().map_err(database_error)?;
    let readers = Readers::open(&settings, 1).map_err(database_error)?;
    let counted = match readers.get().productivity(spread(since, 0)?) {
        Ok(counted) => counted,
        Err(DatabaseError::Sqlite(message)) if message == "division by zero" => {
            return Ok(rejected(DIVISION_BY_ZERO));
        }
        Err(error) => return Err(database_error(error)),
    };
    let rows = counted
        .into_iter()
        .map(|row| {
            let productivity = match (row.produced, row.missed) {
                (None, None) => None,
                (None, Some(_)) => Some("0.00".to_owned()),
                (Some(produced), missed) => hundredths(produced, missed.unwrap_or(0)).map(percent),
            };
            json!({ "username": row.username, "missed": row.missed, "productivity": productivity })
        })
        .collect::<Vec<_>>();
    Ok(Verdict::Output(json!({ "rows": rows })))
}

/// The first second of the productivity window at a tip stamped `tipTimestamp` on a network whose
/// first milestone's epoch is `epoch`, with the lookback `lookback`, as the served table takes it.
fn productivity_window(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let tip = (input["tipTimestamp"].as_u64())
        .and_then(|tip| u32::try_from(tip).ok())
        .ok_or_else(|| failed("input.tipTimestamp is not a timestamp"))?;
    let epoch = slots::epoch_unix_ms(text(input, "epoch")?)
        .ok_or_else(|| failed("input.epoch is not an epoch"))?;
    let lookback =
        (input["lookback"].as_i64()).ok_or_else(|| failed("input.lookback is not an integer"))?;
    let first = window_start(tip, epoch, lookback);
    Ok(Verdict::Output(json!({ "firstSecond": first })))
}

/// The provenance record: both PostgreSQL versions, the recording script's and the inputs' hashes.
/// It checks the record's shape and answers that the two versions agreed.
fn meta(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let hash = |value: &Value| {
        value
            .as_str()
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    let version = |value: &Value, major: &str| {
        value
            .as_str()
            .is_some_and(|text| text.starts_with(&format!("PostgreSQL {major}.")))
    };
    let input = case.input;
    let complete = version(&input["reference"]["postgres"], "14")
        && version(&input["toolchain"]["postgres"], "16")
        && input["reference"]["image"].is_string()
        && hash(&input["script"]["sha256"])
        && hash(&input["inputs"]["sqlSha256"])
        && input["inputs"]["records"].is_object();
    Ok(if complete {
        Verdict::Output(json!({ "agreed": true }))
    } else {
        unclassified("the meta record does not name both versions, the script and the inputs")
    })
}
