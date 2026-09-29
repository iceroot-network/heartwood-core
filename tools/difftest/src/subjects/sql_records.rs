//! The SQL records of the store's arithmetic, recorded from the reference's PostgreSQL:
//! `sql.round2` (`ROUND(numeric, 2)`), `sql.avgInt8` (`AVG(bigint)::int8`), `sql.productivity`
//! (the productivity query over generated row sets, its counting modelled here and its rounding
//! done by the store's own functions) and `sql.meta` (the recording's provenance).

use heartwood_database::{avg_int8, hundredths, round2};
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, rejected, text, unclassified};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("sql.round2", round2_op),
    ("sql.avgInt8", avg_int8_op),
    ("sql.productivity", productivity),
    ("sql.meta", meta),
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

/// The rows of `groups` from `since` on that carry `name`.
fn counted(groups: &[Group<'_>], name: &str, since: i64) -> u64 {
    groups
        .iter()
        .filter(|group| group.name == Some(name) && group.timestamp >= since)
        .map(|group| group.count)
        .sum()
}

/// A count as the reference's outer join gives it: absent where no row of the name is in the
/// window.
fn side(count: u64) -> Option<u64> {
    Some(count).filter(|count| *count > 0)
}

/// `hundredths` of a percent as PostgreSQL writes a `numeric` of two decimals.
fn percent(hundredths: u64) -> String {
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

fn productivity(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let since = case.input["since"]
        .as_i64()
        .ok_or_else(|| failed("input.since is not an integer"))?;
    let mut registered: Vec<&str> = case.input["registered"]
        .as_array()
        .ok_or_else(|| failed("input.registered is not a list"))?
        .iter()
        .map(|name| {
            name.as_str()
                .ok_or_else(|| failed("a registered name is not text"))
        })
        .collect::<Outcome<_>>()?;
    let blocks = groups(case.input, "blocks")?;
    let missed_groups = groups(case.input, "missed")?;

    // A block of no name in the window is a row of no name and no count: its quotient is 0 / 0.
    if blocks
        .iter()
        .any(|group| group.name.is_none() && group.timestamp >= since && group.count > 0)
    {
        return Ok(rejected(DIVISION_BY_ZERO));
    }
    // Rows are ordered by name's bytes and a name registered twice is one row, as the store lists.
    registered.sort_unstable();
    registered.dedup();
    let rows = registered
        .into_iter()
        .map(|name| {
            let (produced, missed) = (
                side(counted(&blocks, name, since)),
                side(counted(&missed_groups, name, since)),
            );
            let productivity = match (produced, missed) {
                (None, None) => None,
                (None, Some(_)) => Some("0.00".to_owned()),
                (Some(produced), missed) => hundredths(produced, missed.unwrap_or(0)).map(percent),
            };
            json!({ "username": name, "missed": missed, "productivity": productivity })
        })
        .collect::<Vec<_>>();
    Ok(Verdict::Output(json!({ "rows": rows })))
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
