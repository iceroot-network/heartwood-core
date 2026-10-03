//! The reference's answer to `GET /blocks/missed` over the missed-slot rows of a store, for an
//! answer of a node under test whose store holds rows the control's does not: the rows a start
//! writes for the slots before block 2 when the store holds no missed-slot row, as the
//! reference's start does too ([`outcome_over_rows`]).
//!
//! The answer is the reference's listing over its `missed_blocks` table, whose key is the slot's
//! timestamp: the criteria `height` and `timestamp` (a value, a comma list, or a `from`/`to`
//! range with both ends included) and `username` (a value or a comma list); the order `orderBy`
//! (`timestamp:desc` by default); the page (`page` from 1, `limit` 100 by default, an `offset`
//! above 0 in place of the page's); and the pagination plugin's `meta`: `totalCountIsEstimate`
//! as the repository gives it, the page's `count`, `pageCount` (one when no row matches),
//! `totalCount`, `next` below the page count, `previous` from page 2 up to one page past the
//! last, and `self`, `first` and `last`, each the request's own link with its page. A row is
//! `{height, timestamp: {epoch, unix, human}, username}`, its times the slot's from the network's
//! epoch.

use heartwood_api::RouteId;
use heartwood_api::schemas::parse_query;
use serde_json::Value;

use super::http::Reply;
use super::judge;
use super::record::Recorded;
use crate::compare;
use crate::report::Outcome;

/// A missed-slot row: the slot's first second since the network epoch, the height of the block
/// forged after the slot, and the delegate that missed it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MissedSlot {
    /// The slot's first second since the network epoch, the table's key.
    pub timestamp: i64,
    /// The height of the block forged after the slot.
    pub height: u64,
    /// The delegate that missed the slot.
    pub username: String,
}

/// The reference's answer to the request of `query` (as [`parse_query`] reads the request's)
/// over `rows`, with the network's epoch `epoch_ms` (milliseconds since the Unix epoch), the
/// request's own link `self_link` (the control's `meta.self`) and the repository's
/// `totalCountIsEstimate`. `None` when the query holds what this model does not read, or when
/// the order leaves two rows tied.
pub fn answer(
    rows: &[MissedSlot],
    query: &Value,
    epoch_ms: i64,
    self_link: &str,
    estimate: &Value,
) -> Option<Value> {
    let criteria = query.as_object()?;
    let known = [
        "page",
        "limit",
        "offset",
        "orderBy",
        "height",
        "timestamp",
        "username",
    ];
    if criteria.keys().any(|key| !known.contains(&key.as_str())) {
        return None;
    }
    let mut matched = Vec::new();
    for row in rows {
        let height = i64::try_from(row.height).ok()?;
        let meets = [
            criteria.get("height").map(|c| meets_number(c, height)),
            criteria
                .get("timestamp")
                .map(|c| meets_number(c, row.timestamp)),
            criteria
                .get("username")
                .map(|c| meets_text(c, &row.username)),
        ];
        let mut all = true;
        for meet in meets.into_iter().flatten() {
            all &= meet?;
        }
        if all {
            matched.push(row);
        }
    }
    let order = order(query)?;
    if order.iter().any(|(property, _)| property == "username") && !bytes_order_text(&matched) {
        return None;
    }
    matched.sort_by(|a, b| compare_rows(a, b, &order));
    if matched
        .windows(2)
        .any(|pair| compare_rows(pair[0], pair[1], &order).is_eq())
    {
        return None;
    }
    let read = |key: &str, default: i64| criteria.get(key).map_or(Some(default), number);
    let (page, limit) = (read("page", 1)?, read("limit", 100)?);
    let offset = match read("offset", 0)? {
        offset if offset > 0 => offset,
        _ => (page - 1).checked_mul(limit)?,
    };
    if page < 1 || limit < 1 || offset < 0 {
        return None;
    }
    let total = i64::try_from(matched.len()).ok()?;
    let start = usize::try_from(offset.min(total)).ok()?;
    let end = usize::try_from(offset.saturating_add(limit).min(total)).ok()?;
    let data: Vec<Value> = (matched[start..end].iter())
        .map(|row| render(row, epoch_ms))
        .collect();
    let page_count = if total > 0 {
        total / limit + i64::from(total % limit != 0)
    } else {
        1
    };
    let link = |page: i64| with_page(self_link, page).map(Value::String);
    let next = if total > 0 && page < page_count {
        link(page + 1)?
    } else {
        Value::Null
    };
    let previous = if total > 0 && page > 1 && page <= page_count + 1 {
        link(page - 1)?
    } else {
        Value::Null
    };
    Some(serde_json::json!({
        "meta": {
            "totalCountIsEstimate": estimate,
            "count": data.len(),
            "pageCount": page_count,
            "totalCount": total,
            "next": next,
            "previous": previous,
            "self": link(page)?,
            "first": link(1)?,
            "last": link(page_count)?,
        },
        "data": data,
    }))
}

/// A whole number of the query, as text or as a JSON number.
fn number(value: &Value) -> Option<i64> {
    match value {
        Value::String(text) => text.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// Whether `value` meets a numeric criterion: a value, a list of values (any of them), or a
/// range of `from` and `to` (both included). `None` for a criterion of another form.
fn meets_number(criterion: &Value, value: i64) -> Option<bool> {
    match criterion {
        Value::Array(items) => {
            let values: Vec<i64> = items.iter().map(number).collect::<Option<_>>()?;
            Some(values.contains(&value))
        }
        Value::Object(range) => {
            let mut within = true;
            for (key, bound) in range {
                let bound = number(bound)?;
                within &= match key.as_str() {
                    "from" => value >= bound,
                    "to" => value <= bound,
                    _ => return None,
                };
            }
            Some(within)
        }
        other => Some(number(other)? == value),
    }
}

/// Whether `value` meets a text criterion: a value or a list of values (any of them).
fn meets_text(criterion: &Value, value: &str) -> Option<bool> {
    match criterion {
        Value::String(text) => Some(text == value),
        Value::Array(items) => {
            let texts: Vec<&str> = items.iter().map(Value::as_str).collect::<Option<_>>()?;
            Some(texts.contains(&value))
        }
        _ => None,
    }
}

/// The order of the query's `orderBy` (`timestamp:desc` by default): each property and whether it
/// descends. `None` for a property the route does not order by, or a direction it refuses.
fn order(query: &Value) -> Option<Vec<(String, bool)>> {
    let entries: Vec<&str> = match query.get("orderBy") {
        None => vec!["timestamp:desc"],
        Some(Value::String(text)) => vec![text.as_str()],
        Some(Value::Array(items)) => items.iter().map(Value::as_str).collect::<Option<_>>()?,
        Some(_) => return None,
    };
    entries
        .into_iter()
        .map(|entry| {
            let (property, direction) = entry.split_once(':')?;
            if !matches!(property, "height" | "timestamp" | "username") {
                return None;
            }
            let descending = match direction {
                "asc" => false,
                "desc" => true,
                _ => return None,
            };
            Some((property.to_owned(), descending))
        })
        .collect()
}

/// Two rows in `order`.
fn compare_rows(a: &MissedSlot, b: &MissedSlot, order: &[(String, bool)]) -> std::cmp::Ordering {
    order
        .iter()
        .map(|(property, descending)| {
            let ordering = match property.as_str() {
                "height" => a.height.cmp(&b.height),
                "timestamp" => a.timestamp.cmp(&b.timestamp),
                _ => a.username.as_bytes().cmp(b.username.as_bytes()),
            };
            if *descending {
                ordering.reverse()
            } else {
                ordering
            }
        })
        .find(|ordering| ordering.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// Whether the store's text order of the usernames of `rows` is their byte order whatever its
/// collation: every two distinct names differ first at a letter or a digit in both, after the
/// same characters, and compare alike with or without their case and other characters.
fn bytes_order_text(rows: &[&MissedSlot]) -> bool {
    let folded = |name: &str| -> String {
        (name.chars())
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    rows.iter().all(|a| {
        rows.iter().all(|b| {
            let (x, y) = (&a.username, &b.username);
            if x == y {
                return true;
            }
            let first = x.bytes().zip(y.bytes()).find(|(p, q)| p != q);
            let alike =
                first.is_some_and(|(p, q)| p.is_ascii_alphanumeric() && q.is_ascii_alphanumeric());
            alike && folded(x).cmp(&folded(y)) == x.as_bytes().cmp(y.as_bytes())
        })
    })
}

/// A row as the route lists it: its timestamp with the network's epoch added (`unix`, seconds;
/// `human`, the instant in ISO 8601 with milliseconds).
fn render(row: &MissedSlot, epoch_ms: i64) -> Value {
    let at = epoch_ms.saturating_add(row.timestamp.saturating_mul(1_000));
    serde_json::json!({
        "height": row.height,
        "timestamp": {
            "epoch": row.timestamp,
            "unix": at.div_euclid(1_000),
            "human": crate::live::run::format_time(at),
        },
        "username": row.username,
    })
}

/// The link `link` (a path and its query) with its `page` set to `page`; `None` without one.
fn with_page(link: &str, page: i64) -> Option<String> {
    let (path, query) = link.split_once('?')?;
    let mut found = false;
    let parts: Vec<String> = query
        .split('&')
        .map(|part| match part.split_once('=') {
            Some(("page", _)) => {
                found = true;
                format!("page={page}")
            }
            _ => part.to_owned(),
        })
        .collect();
    found.then(|| format!("{path}?{}", parts.join("&")))
}

/// The outcome of a recorded `/blocks/missed` request whose node under test's store holds the
/// rows `ours` and the control's the rows `theirs`: [`judge::outcome`] after the control's answer
/// is replaced by the reference's answer over `ours` ([`answer`]), which is allowed only when the
/// reference's answer over `theirs`, built the same way, is the control's answer exactly. Any
/// other request, or a model that does not give the control's own answer, is judged as without
/// it.
pub fn outcome_over_rows(
    line: &Recorded,
    ours: &[MissedSlot],
    theirs: &[MissedSlot],
    epoch_ms: i64,
) -> Outcome {
    let plain = judge::outcome(line);
    if !plain.is_failure() || !is_missed_blocks(line) {
        return plain;
    }
    let (
        Some(request),
        Some(Reply::Answer { status: 200, .. }),
        Some(Reply::Answer {
            status: 200,
            headers,
            body: right,
        }),
    ) = (&line.request, &line.left, &line.right)
    else {
        return plain;
    };
    let query = parse_query(&request.query);
    let Some(self_link) = right.pointer("/meta/self").and_then(Value::as_str) else {
        return plain;
    };
    let estimate = (right.pointer("/meta/totalCountIsEstimate").cloned()).unwrap_or(Value::Null);
    let model = |rows: &[MissedSlot]| answer(rows, &query, epoch_ms, self_link, &estimate);
    // The model must give the control's own answer over the control's own rows.
    match model(theirs) {
        Some(theirs) if compare::diff(&theirs, right).is_empty() => {}
        _ => return plain,
    }
    let Some(expected) = model(ours) else {
        return plain;
    };
    let mut judged = line.clone();
    judged.right = Some(Reply::Answer {
        status: 200,
        headers: headers.clone(),
        body: expected,
    });
    match judge::outcome(&judged) {
        Outcome::Pass => Outcome::Pass,
        _ => plain,
    }
}

/// Whether `line` requests `GET /blocks/missed`.
pub fn is_missed_blocks(line: &Recorded) -> bool {
    let route = (line.request.as_ref()).and_then(|r| judge::route_of(&r.method, &r.path));
    route == Some(RouteId::BlocksMissed)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A recorded run's answers to `/blocks/missed` with a status of 200 on both nodes: node 4,
    /// a Heartwood relay whose start back-filled the slots before block 2, against node 1, the
    /// reference control, which never restarted.
    fn recorded() -> Vec<Recorded> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/blocks-missed/backfill.jsonl");
        let text = std::fs::read_to_string(path).unwrap();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn slot(timestamp: i64, height: u64, username: &str) -> MissedSlot {
        MissedSlot {
            timestamp,
            height,
            username: username.to_owned(),
        }
    }

    /// The control's rows: two slots missed in rounds 3 and 4.
    fn theirs() -> Vec<MissedSlot> {
        vec![slot(1224, 151, "tx1n2290"), slot(1704, 210, "tx1n2290")]
    }

    /// The node under test's rows: the control's and the three slots before block 2.
    fn ours() -> Vec<MissedSlot> {
        let mut rows = theirs();
        rows.extend([
            slot(8, 2, "genesis_41"),
            slot(16, 2, "genesis_44"),
            slot(24, 2, "genesis_51"),
        ]);
        rows
    }

    /// The network's epoch of the recorded run: 2026-10-03T02:58:15Z.
    const EPOCH_MS: i64 = 1_790_996_295_000;

    #[test]
    fn the_back_filled_rows_answer_as_the_reference_answers_over_them() {
        let lines = recorded();
        let failed: Vec<&str> = (lines.iter())
            .filter(|l| judge::outcome(l).is_failure())
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(failed.len(), 19, "{failed:?}");
        for line in &lines {
            assert!(is_missed_blocks(line));
            assert_eq!(
                outcome_over_rows(line, &ours(), &theirs(), EPOCH_MS),
                Outcome::Pass,
                "{}",
                line.name
            );
        }
    }

    #[test]
    fn rows_the_back_fill_does_not_explain_still_fail() {
        let lines = recorded();
        let failing = |ours: &[MissedSlot], theirs: &[MissedSlot], epoch: i64| {
            (lines.iter())
                .filter(|l| outcome_over_rows(l, ours, theirs, epoch).is_failure())
                .count()
        };
        // Without the back-filled rows, or with another delegate's, another height's or one
        // slot's less: the answers are not the reference's over the store.
        assert_eq!(failing(&theirs(), &theirs(), EPOCH_MS), 19);
        let mut renamed = ours();
        renamed[2].username = "genesis_42".into();
        assert!(failing(&renamed, &theirs(), EPOCH_MS) > 0);
        let mut higher = ours();
        higher[3].height = 3;
        assert!(failing(&higher, &theirs(), EPOCH_MS) > 0);
        let fewer = &ours()[..4];
        assert!(failing(fewer, &theirs(), EPOCH_MS) > 0);
        // The control's rows must give the control's answers: a row it does not hold, or another
        // epoch, and nothing is excused.
        let mut more = theirs();
        more.push(slot(32, 3, "genesis_53"));
        assert_eq!(failing(&ours(), &more, EPOCH_MS), 19);
        // Another epoch renders every listed row otherwise: only the pages that list none pass.
        let listed = |line: &Recorded| {
            [&line.left, &line.right]
                .into_iter()
                .any(|reply| match reply {
                    Some(Reply::Answer { body, .. }) => {
                        body["data"].as_array().is_some_and(|rows| !rows.is_empty())
                    }
                    _ => false,
                })
        };
        let shown = (lines.iter())
            .filter(|l| judge::outcome(l).is_failure() && listed(l))
            .count();
        assert_eq!(shown, 15);
        assert_eq!(failing(&ours(), &theirs(), EPOCH_MS + 1_000), shown);
    }

    #[test]
    fn the_model_reads_only_what_it_knows() {
        let estimate = Value::Bool(false);
        let link = "/blocks/missed?page=1&limit=100";
        let rows = ours();
        let read = |query: &str| answer(&rows, &parse_query(query), EPOCH_MS, link, &estimate);
        assert!(read("").is_some());
        assert!(read("height.from=2&height.to=100").is_some());
        // An unknown key or order, a direction the route refuses, and a range of text.
        assert!(read("reward=0").is_none());
        assert!(read("orderBy=reward:asc").is_none());
        assert!(read("orderBy=height:DESC").is_none());
        assert!(read("username.from=a").is_none());
        // An order that leaves rows tied: the store's tie order is not the model's to give.
        assert!(read("orderBy=height:asc").is_none());
        // A link without its page cannot be paged.
        assert!(answer(&rows, &Value::Null, EPOCH_MS, "/blocks/missed", &estimate).is_none());
    }
}
