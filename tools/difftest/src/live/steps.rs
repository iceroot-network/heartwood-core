//! The step reports of a live run: every scripted step met its expectation, the run's schedule is
//! complete, txgen's own checks passed, run `h`'s submissions and scenario wallets equal run `r`'s, the rate
//! limit's answers, and how fast blocks reached the nodes under test.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::errors::Result;
use crate::live::run::{Step, Window, parse_time, read_json, read_lines};

/// The steps every run must have run, by run: its scripted scenario.
pub fn required(run: &str) -> Vec<String> {
    let common = [
        "generate",
        "up",
        "settle",
        "pause",
        "check-chain",
        "tips",
        "archive",
        "collect",
    ];
    let own: &[&str] = match run {
        "t" | "m" => &[
            "txgen-fund",
            "txgen-once",
            "vote-before-node2",
            "stop-forger-node2",
            "restart-node4",
            "vote-after-node2",
            "peers-node5",
            "batch-node3",
            "restart-node2",
            "fresh-node5",
            "api-corpus",
            "api-pool",
            "api-rate-limit",
        ],
        "f" => &[
            "failover-second",
            "failover-third",
            "failover-return",
            "crash-forger-node2",
            "crash-relay-node3",
            "restart-node3",
            "recover-node4",
            "replay-node5",
            "outage-two",
            "outage-three",
            "resume",
            "fresh-node5",
        ],
        "r" | "h" => &[
            "peers-node4",
            "txgen-fund",
            "txgen-once",
            "batch-node4",
            "vote-before-node4",
            "vote-after-node4",
            "api-pool",
            "api-corpus",
            "api-rate-limit",
        ],
        "m1" => &[
            "txgen-fund",
            "send-txs",
            "kill-node4-INT",
            "kill-node4-KILL",
            "start-node5",
            "kill-during-sync",
            "node5-synced",
            "peers-node5",
            "marker-restart-node4",
            "vote-before-node4",
            "vote-after-node4",
            "batch-node4",
            "api-pool",
            "api-corpus",
            "api-rate-limit",
        ],
        "m2" => &[
            "txgen-fund",
            "send-txs",
            "partition",
            "conflict",
            "minority-pause",
            "archive-minority",
            "heal",
            "converged",
            "conflict-outcome",
        ],
        "mixed-short" | "mixed" | "mixed-split" => &["api-pool", "api-rate-limit"],
        "core" => &["locks", "relay-start-refused", "stop"],
        "churn" => &["final-status", "api-pool"],
        _ => &[],
    };
    // The steps of a mixed run, with their planned rounds, are the schedule's.
    let mixed: Vec<&str> = super::schedule::plan(run).iter().map(|p| p.name).collect();
    // `archive` is node 1's archive. Run s's node 1 is a Heartwood relay, whose files the
    // collection takes (its block files are the run's chain): its archive is the reference
    // observer's, node 4's.
    let archives: &[&str] = if run == "s" { &["archive-node4"] } else { &[] };
    common
        .iter()
        .filter(|s| run != "s" || **s != "archive")
        .chain(own)
        .chain(&mixed)
        .chain(archives)
        .map(|s| (*s).to_owned())
        .collect()
}

/// How many times a required step must have run.
pub fn times(run: &str, step: &str) -> usize {
    if run == "m1" && step == "kill-node4-KILL" {
        // Three kills in round 3, and the one of node 5 during its sync is its own step.
        3
    } else {
        1
    }
}

/// The problems of the steps: a failed step, a required step that did not run (often enough).
pub fn problems(run: &str, steps: &[Step]) -> Vec<String> {
    let mut out = Vec::new();
    if !matches!(
        run,
        "r" | "h"
            | "m1"
            | "m2"
            | "t"
            | "m"
            | "f"
            | "w0"
            | "w"
            | "p"
            | "s"
            | "mixed-short"
            | "mixed"
            | "mixed-split"
            | "churn"
            | "core"
    ) {
        out.push(format!("unknown live run {run}"));
    }
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for s in steps {
        *count.entry(s.name.as_str()).or_default() += 1;
        if !s.ok {
            let detail = s
                .record
                .get("note")
                .or_else(|| s.record.get("detail"))
                .map(Value::to_string)
                .unwrap_or_default();
            out.push(format!(
                "step {} ({}) failed {}",
                s.name,
                s.node,
                truncate(&detail, 300)
            ));
        }
    }
    for name in required(run) {
        let want = times(run, &name);
        let have = count.get(name.as_str()).copied().unwrap_or(0);
        if have < want {
            out.push(format!(
                "step {name} ran {have} times, the schedule has it {want}"
            ));
        }
    }
    out
}

fn truncate(text: &str, n: usize) -> String {
    if text.len() <= n {
        text.to_owned()
    } else {
        let mut end = n;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    }
}

/// The code of a refusal whose message names why a transaction does not decode or fails the schema.
pub const BAD_DATA: &str = "ERR_BAD_DATA";
/// The prefix of such a message in both implementations; the reason text after it differs.
pub const BAD_DATA_PREFIX: &str = "Invalid transaction data";

/// Whether `answer` (`{outcome, code, message}`) refuses a transaction that does not decode or
/// fails the schema, with the message both implementations begin alike.
fn bad_data(answer: &Value) -> bool {
    answer.get("outcome").and_then(Value::as_str) == Some("rejected")
        && answer.get("code").and_then(Value::as_str) == Some(BAD_DATA)
        && answer
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|m| m.starts_with(BAD_DATA_PREFIX))
}

/// txgen's verdict in `dir`: its summary's counts, the scenarios that failed, and those whose only
/// failure is a documented difference.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Txgen {
    /// Submission, pass, fail and skip counts.
    pub text: String,
    /// The failures that stand, each `scenario: why`, and txgen's other unmet conditions.
    pub failures: Vec<String>,
    /// The failures that only show a documented difference, each `scenario: why`.
    pub documented: Vec<String>,
}

/// txgen's verdict in `dir` (its `summary.json`, and the outcome records of its `txgen.jsonl`
/// files). With `decode_documented` (its submissions went to a Heartwood relay), a failed scenario
/// whose expected refusal came with the expected code, `ERR_BAD_DATA`, but whose message did not
/// match the reference's reason text is documented, not failed: Heartwood names its own decoder's
/// reason after the prefix both implementations write.
pub fn txgen(dir: &Path, decode_documented: bool) -> Result<Txgen> {
    let summary = read_json(&dir.join("summary.json"))?;
    let n = |k: &str| summary.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut out = Txgen {
        text: format!(
            "{} submissions: {} passed, {} failed, {} scenarios skipped",
            n("submissions"),
            n("pass"),
            n("fail"),
            n("skipped")
        ),
        ..Txgen::default()
    };
    // The last outcome of each failed scenario, as txgen recorded it.
    let mut outcomes: BTreeMap<String, Value> = BTreeMap::new();
    for file in txgen_files(dir) {
        for e in read_lines(&file)? {
            if e.get("event").and_then(Value::as_str) == Some("outcome")
                && e.get("verdict").and_then(Value::as_str) == Some("fail")
                && let Some(scenario) = e.get("scenario").and_then(Value::as_str)
            {
                outcomes.insert(scenario.to_owned(), e);
            }
        }
    }
    let failures = summary
        .get("failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for f in &failures {
        let scenario = f.get("scenario").and_then(Value::as_str).unwrap_or("?");
        let why = f.get("why").and_then(Value::as_str).unwrap_or("");
        let text = format!("{scenario}: {why}");
        let reason_only = outcomes.get(scenario).is_some_and(|e| {
            e.get("why")
                .and_then(Value::as_str)
                .is_some_and(|w| w.starts_with("message does not match"))
                && e.pointer("/expect/outcome").and_then(Value::as_str) == Some("rejected")
                && e.pointer("/expect/code").and_then(Value::as_str) == Some(BAD_DATA)
                && e.get("response").is_some_and(bad_data)
        });
        if decode_documented && reason_only {
            out.documented.push(text);
        } else {
            out.failures.push(text);
        }
    }
    if usize::try_from(n("fail")).unwrap_or(usize::MAX) > failures.len() {
        out.failures.push(format!(
            "{} failed scenarios, {} of them listed",
            n("fail"),
            failures.len()
        ));
    }
    // txgen's own `ok` also needs every kept type forged and, for a --once run, a doubly signed
    // transaction forged; a failed scenario alone leaves it false.
    let ok = summary.get("ok") == Some(&Value::Bool(true));
    let others = summary
        .get("keptTypesMissing")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
        && (summary.get("mode").and_then(Value::as_str) != Some("once")
            || n("doublySignedForged") > 0);
    if !(ok || (n("fail") > 0 && others)) {
        out.failures.push(format!(
            "txgen's summary is not ok (kept types missing: {}, doubly signed forged: {})",
            summary.get("keptTypesMissing").unwrap_or(&Value::Null),
            n("doublySignedForged")
        ));
    }
    Ok(out)
}

/// txgen's record files in `dir`, oldest first: `txgen.jsonl.4` is the oldest rotated file,
/// `txgen.jsonl` the newest.
fn txgen_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("txgen.jsonl"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p: &std::path::PathBuf| {
        std::cmp::Reverse(
            p.extension()
                .and_then(|e| e.to_str())
                .and_then(|e| e.parse::<u32>().ok())
                .unwrap_or(0),
        )
    });
    files
}

/// Each scripted submission of txgen's records in `dir`: scenario and id, and the node's answer
/// (outcome, code, message), in submission order.
pub fn submissions(dir: &Path) -> Result<Vec<(String, Value)>> {
    let mut out = Vec::new();
    for file in txgen_files(dir) {
        for e in read_lines(&file)? {
            if e.get("event").and_then(Value::as_str) != Some("submitted") {
                continue;
            }
            let key = format!(
                "{} {}",
                e.get("scenario").and_then(Value::as_str).unwrap_or("?"),
                e.get("id").and_then(Value::as_str).unwrap_or("?")
            );
            let r = e.get("response").cloned().unwrap_or(Value::Null);
            let answer = serde_json::json!({
                "outcome": r.get("outcome"),
                "code": r.get("code"),
                "message": r.get("message"),
            });
            out.push((key, answer));
        }
    }
    Ok(out)
}

/// A pool message with every run of digits replaced by `#`: a message that counts the blocks still
/// to come ("105 more blocks must be produced ...") depends on the height the transaction reached
/// the pool at, which the two runs' timing sets.
pub fn without_numbers(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut digits = false;
    for c in message.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

/// How two runs' submissions (`h` against `r`) compare: each scenario and id must get the same
/// answer, in the same order. `differences` fail; `timing` lists the answers whose messages differ
/// only in their numbers, which is recorded; `documented` the `ERR_BAD_DATA` refusals whose
/// messages differ only in the reason text after the prefix both implementations write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Submissions {
    /// Differences in the submissions or their answers.
    pub differences: Vec<String>,
    /// Answers equal but for the numbers of their messages.
    pub timing: Vec<String>,
    /// Refusals of a transaction that does not decode or fails the schema, equal but for the
    /// reason text of their messages.
    pub documented: Vec<String>,
}

/// Compare two runs' submissions.
pub fn compare_submissions(ours: &[(String, Value)], theirs: &[(String, Value)]) -> Submissions {
    let mut out = Submissions::default();
    if ours.len() != theirs.len() {
        out.differences.push(format!(
            "{} submissions here, {} in the pair run",
            ours.len(),
            theirs.len()
        ));
    }
    let masked = |v: &Value| {
        let mut v = v.clone();
        if let Some(m) = v
            .get("message")
            .and_then(Value::as_str)
            .map(without_numbers)
        {
            v["message"] = Value::String(m);
        }
        v
    };
    for (i, (a, b)) in ours.iter().zip(theirs).enumerate() {
        if a.0 != b.0 {
            out.differences.push(format!(
                "submission {}: {} here, {} in the pair run",
                i + 1,
                a.0,
                b.0
            ));
        } else if a.1 == b.1 {
        } else if masked(&a.1) == masked(&b.1) {
            out.timing
                .push(format!("{}: {} here, {} in the pair run", a.0, a.1, b.1));
        } else if bad_data(&a.1) && bad_data(&b.1) {
            out.documented
                .push(format!("{}: {} here, {} in the pair run", a.0, a.1, b.1));
        } else {
            out.differences
                .push(format!("{}: {} here, {} in the pair run", a.0, a.1, b.1));
        }
    }
    out
}

/// The differences between two runs' scenario wallets at the pause (`wallets.json`).
pub fn compare_wallets(ours: &Value, theirs: &Value) -> Vec<String> {
    let by_address = |v: &Value| -> BTreeMap<String, Value> {
        v.get("wallets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|w| Some((w.get("address")?.as_str()?.to_owned(), w.clone())))
            .collect()
    };
    let (a, b) = (by_address(ours), by_address(theirs));
    let mut out = Vec::new();
    for address in a
        .keys()
        .chain(b.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        match (a.get(address), b.get(address)) {
            (Some(x), Some(y)) if x == y => {}
            (Some(x), Some(y)) => out.push(format!("{address}: {x} here, {y} in the pair run")),
            (Some(_), None) => out.push(format!("{address}: only here")),
            (None, Some(_)) => out.push(format!("{address}: only in the pair run")),
            (None, None) => {}
        }
    }
    out
}

/// The rate limit's answers (`rate-limit.jsonl`, sent from an address no node whitelists) of the
/// node under test (`left`) against the control (`right`): for each case and repetition an
/// answer (a request that failed, or an answer without a status, is a problem), the same status,
/// the same 429 body and the same header names; and on each node at least one 429, without
/// which nothing shows that the requests reached the rate limit. `Ok` holds a summary.
pub fn rate_limit(path: &Path) -> Result<std::result::Result<String, Vec<String>>> {
    let answers = read_lines(path)?;
    type Key = (String, u64);
    let side = |name: &str| -> BTreeMap<Key, &Value> {
        answers
            .iter()
            .filter(|a| a.get("node").and_then(Value::as_str) == Some(name))
            .map(|a| {
                let case = a
                    .get("case")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                ((case, a.get("i").and_then(Value::as_u64).unwrap_or(0)), a)
            })
            .collect()
    };
    let (left, right) = (side("left"), side("right"));
    let mut problems = Vec::new();
    if left.is_empty() || right.is_empty() {
        problems.push("no answers recorded".to_owned());
    }
    if left.len() != right.len() {
        problems.push(format!(
            "{} answers of the node under test, {} of the control",
            left.len(),
            right.len()
        ));
    }
    let names = |a: &Value| -> Vec<String> {
        a.get("headers")
            .and_then(Value::as_object)
            .map(|h| h.keys().cloned().collect())
            .unwrap_or_default()
    };
    let status = |a: &Value| a.get("status").and_then(Value::as_u64);
    let failed = |a: &Value| a.get("error").is_some() || status(a).is_none();
    for (who, answers) in [("the node under test", &left), ("the control", &right)] {
        for ((case, i), a) in answers.iter().filter(|(_, a)| failed(a)) {
            let why = a
                .get("error")
                .map_or("no status".to_owned(), Value::to_string);
            problems.push(format!("{case} request {i}: no answer of {who}: {why}"));
        }
    }
    let limited = |answers: &BTreeMap<Key, &Value>| {
        (answers.values())
            .filter(|a| status(a) == Some(429))
            .count()
    };
    for (who, answers) in [("the node under test", &left), ("the control", &right)] {
        if limited(answers) == 0 {
            problems.push(format!("no answer of {who} is a 429"));
        }
    }
    for ((case, i), l) in &left {
        let Some(r) = right.get(&(case.clone(), *i)) else {
            problems.push(format!("{case} request {i}: no answer of the control"));
            continue;
        };
        if failed(l) || failed(r) {
            continue;
        }
        if l.get("status") != r.get("status") {
            problems.push(format!(
                "{case} request {i}: status {} against {}",
                l["status"], r["status"]
            ));
        } else if l.get("body") != r.get("body") {
            problems.push(format!(
                "{case} request {i}: body {} against {}",
                l["body"], r["body"]
            ));
        } else if names(l) != names(r) {
            problems.push(format!(
                "{case} request {i}: headers {:?} against {:?}",
                names(l),
                names(r)
            ));
        }
    }
    Ok(if problems.is_empty() {
        Ok(format!(
            "{} requests each, {} answered 429 on both",
            left.len(),
            limited(&left)
        ))
    } else {
        Err(problems)
    })
}

/// When each node first held each height, from `propagation.jsonl`: node, then height, then the
/// time in milliseconds.
pub fn arrivals(path: &Path) -> Result<BTreeMap<String, BTreeMap<u32, i64>>> {
    let mut out: BTreeMap<String, BTreeMap<u32, i64>> = BTreeMap::new();
    let mut top: BTreeMap<String, u32> = BTreeMap::new();
    for e in read_lines(path)? {
        let (Some(node), Some(h), Some(t)) = (
            e.get("node").and_then(Value::as_str),
            e.get("h")
                .and_then(Value::as_u64)
                .and_then(|h| u32::try_from(h).ok()),
            e.get("t").and_then(Value::as_str).and_then(parse_time),
        ) else {
            continue;
        };
        let heights = out.entry(node.to_owned()).or_default();
        // A jump of several blocks: every height up to the new tip arrived now; the first tip a
        // node shows says nothing of the heights below it.
        let seen = top.entry(node.to_owned()).or_insert(h.saturating_sub(1));
        for height in (*seen + 1)..=h {
            heights.entry(height).or_insert(t);
        }
        *seen = (*seen).max(h);
    }
    Ok(out)
}

/// The block lags of `node` against `control` (seconds, per height), leaving out heights whose
/// arrival at `node` falls in one of its fault windows (the node was down) or before `from`.
pub fn lags(
    arrivals: &BTreeMap<String, BTreeMap<u32, i64>>,
    node: &str,
    control: &str,
    windows: &[Window],
    from: i64,
) -> Vec<(u32, f64)> {
    let (Some(ours), Some(theirs)) = (arrivals.get(node), arrivals.get(control)) else {
        return Vec::new();
    };
    theirs
        .iter()
        .filter_map(|(h, t1)| {
            let t = *ours.get(h)?;
            if t < from
                || windows
                    .iter()
                    .any(|w| w.concerns(node) && (w.contains(t) || w.contains(*t1)))
            {
                return None;
            }
            #[allow(clippy::cast_precision_loss)]
            let lag = (t - t1) as f64 / 1_000.0;
            Some((*h, lag))
        })
        .collect()
}

/// Wallet projection for independently forged runs: delegate metadata and vote weights depend
/// on timing; balances, nonces, votes and the remaining attributes must agree.
pub fn compare_scenario_wallets(ours: &Value, theirs: &Value) -> Vec<String> {
    fn project(v: &Value) -> Value {
        let mut v = v.clone();
        if let Some(wallets) = v["wallets"].as_array_mut() {
            for w in wallets {
                if let Some(o) = w.as_object_mut() {
                    o.remove("delegate");
                    o.remove("voteBalance");
                    o.remove("votingBalance");
                }
                if let Some(o) = w["attributes"].as_object_mut() {
                    o.retain(|k, _| {
                        k != "delegate"
                            && !k.starts_with("delegate.")
                            && k != "voteBalance"
                            && k != "votingBalance"
                    });
                }
            }
        }
        v
    }
    if ours["wallets"].as_array().is_none_or(Vec::is_empty)
        || theirs["wallets"].as_array().is_none_or(Vec::is_empty)
    {
        return vec!["no scenario wallets recorded".into()];
    }
    compare_wallets(&project(ours), &project(theirs))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::vectors::testutil::TestDir;

    #[test]
    fn a_missing_or_failed_step_is_a_problem() {
        let step = |name: &str, ok: bool| Step {
            name: name.into(),
            node: "node4".into(),
            ok,
            start: None,
            record: json!({"step": name, "ok": ok, "note": "why"}),
        };
        let all: Vec<Step> = required("r").iter().map(|n| step(n, true)).collect();
        assert!(problems("r", &all).is_empty());
        let mut some = all.clone();
        some.retain(|s| s.name != "vote-after-node4");
        some.push(step("batch-node4", false));
        let p = problems("r", &some);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].contains("batch-node4") && p[0].contains("why"));
        assert!(p[1].contains("vote-after-node4 ran 0 times"));
        assert!(
            problems("m1", &all)
                .iter()
                .any(|p| p.contains("kill-node4-KILL ran 0 times, the schedule has it 3"))
        );
        // The mixed runs have their schedules' steps; the baseline has no stall.
        assert!(required("w").contains(&"stall-node4".to_owned()));
        assert!(!required("w0").contains(&"stall-node4".to_owned()));
        assert!(required("s").contains(&"restart-node2".to_owned()));
        assert!(
            problems("s", &[])
                .iter()
                .all(|p| !p.contains("unknown live run"))
        );
        // Run s's node 1 is a Heartwood relay: its files are collected, and the archive is the
        // reference observer's.
        let s: Vec<Step> = required("s").iter().map(|n| step(n, true)).collect();
        assert!(problems("s", &s).is_empty(), "{:?}", problems("s", &s));
        assert!(!required("s").contains(&"archive".to_owned()));
        assert!(required("s").contains(&"archive-node4".to_owned()));
        assert!(required("w").contains(&"archive".to_owned()));
        // The mixed forging runs: the control node's archive is `archive`, and each has its own
        // steps beside the common ones.
        for run in ["mixed-short", "mixed", "mixed-split"] {
            let all: Vec<Step> = required(run).iter().map(|n| step(n, true)).collect();
            assert!(
                problems(run, &all).is_empty(),
                "{run}: {:?}",
                problems(run, &all)
            );
            assert!(required(run).contains(&"archive".to_owned()));
            assert!(required(run).contains(&"batch-heartwood".to_owned()));
        }
        assert!(required("mixed").contains(&"kill-forger-node5".to_owned()));
        assert!(required("mixed-split").contains(&"archive-minority".to_owned()));
        assert!(!required("mixed-short").contains(&"partition".to_owned()));
    }

    #[test]
    fn arrivals_fill_jumps_and_lags_skip_fault_windows() {
        let dir = TestDir::new("live-propagation");
        let path = dir.path().join("propagation.jsonl");
        let lines = [
            json!({"t": "1970-01-01T00:00:10.000Z", "node": "node1", "h": 2, "id": "a"}),
            json!({"t": "1970-01-01T00:00:10.500Z", "node": "node4", "h": 2, "id": "a"}),
            json!({"t": "1970-01-01T00:00:18.000Z", "node": "node1", "h": 3, "id": "b"}),
            json!({"t": "1970-01-01T00:00:26.000Z", "node": "node1", "h": 4, "id": "c"}),
            json!({"t": "1970-01-01T00:00:40.000Z", "node": "node4", "h": 4, "id": "c"}),
        ];
        std::fs::write(
            &path,
            lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
        )
        .unwrap();
        let a = arrivals(&path).unwrap();
        assert_eq!(a["node4"][&3], 40_000);
        let l = lags(&a, "node4", "node1", &[], 0);
        assert_eq!(l, vec![(2, 0.5), (3, 22.0), (4, 14.0)]);
        let window = Window {
            fault: "sigkill".into(),
            nodes: vec!["node4".into()],
            start: 15_000,
            end: 45_000,
        };
        assert_eq!(lags(&a, "node4", "node1", &[window], 0), vec![(2, 0.5)]);
    }

    #[test]
    fn rate_limit_answers_must_agree() {
        let dir = TestDir::new("live-rate-limit");
        let path = dir.path().join("answers.jsonl");
        let mut text = String::new();
        for node in ["left", "right"] {
            for i in 1..=3 {
                let status = if i == 3 { 429 } else { 200 };
                text.push_str(&format!(
                    "{}\n",
                    json!({"node": node, "i": i, "status": status, "headers": {"x-block-height": "5"}})
                ));
            }
        }
        std::fs::write(&path, &text).unwrap();
        assert_eq!(
            rate_limit(&path).unwrap(),
            Ok("3 requests each, 1 answered 429 on both".to_owned())
        );
        std::fs::write(&path, text.replacen("\"status\":429", "\"status\":200", 1)).unwrap();
        assert!(rate_limit(&path).unwrap().is_err());
        // Equal answers without a 429 (a whitelisted sender), or requests that failed on both.
        std::fs::write(&path, text.replace("\"status\":429", "\"status\":200")).unwrap();
        let problems = rate_limit(&path).unwrap().unwrap_err();
        assert_eq!(problems.len(), 2, "{problems:?}");
        let failed: String = ["left", "right"]
            .iter()
            .flat_map(|node| (1..=3).map(move |i| (node, i)))
            .map(|(node, i)| format!("{}\n", json!({"node": node, "i": i, "error": "refused"})))
            .collect();
        std::fs::write(&path, failed).unwrap();
        let problems = rate_limit(&path).unwrap().unwrap_err();
        assert_eq!(problems.len(), 8, "{problems:?}");
    }

    #[test]
    fn txgen_s_reason_text_failures_of_a_heartwood_relay_are_documented() {
        let dir = TestDir::new("live-txgen");
        let outcome = |scenario: &str, code: &str, message: &str, why: &str| {
            json!({"event": "outcome", "scenario": scenario, "verdict": "fail", "why": why,
                   "expect": {"outcome": "rejected", "code": "ERR_BAD_DATA", "match": "/Version 2 not supported/"},
                   "response": {"outcome": "rejected", "code": code, "message": message}})
        };
        let events = [
            json!({"event": "submitted", "scenario": "invalid-version-2"}),
            outcome(
                "invalid-version-2",
                "ERR_BAD_DATA",
                "Invalid transaction data: unsupported transaction version 2",
                "message does not match /Version 2 not supported/: Invalid transaction data: unsupported transaction version 2",
            ),
            outcome(
                "invalid-other-code",
                "ERR_APPLY",
                "Cannot apply",
                "expected ERR_BAD_DATA, got ERR_APPLY: Cannot apply",
            ),
        ];
        std::fs::write(
            dir.path().join("txgen.jsonl"),
            events.iter().map(|e| format!("{e}\n")).collect::<String>(),
        )
        .unwrap();
        let summary = |failures: Value| {
            let fail = failures.as_array().map_or(0, Vec::len);
            json!({"mode": "once", "submissions": 3, "pass": 3 - fail, "fail": fail, "skipped": 0,
                   "keptTypesMissing": [], "doublySignedForged": 3, "ok": fail == 0,
                   "failures": failures})
        };
        let write =
            |s: Value| std::fs::write(dir.path().join("summary.json"), s.to_string()).unwrap();
        let reason = json!({"scenario": "invalid-version-2", "why": "message does not match /Version 2 not supported/: Invalid transaction data: unsupported transaction version 2"});
        let code = json!({"scenario": "invalid-other-code", "why": "expected ERR_BAD_DATA, got ERR_APPLY: Cannot apply"});

        write(summary(json!([reason])));
        let heartwood = txgen(dir.path(), true).unwrap();
        assert!(heartwood.failures.is_empty(), "{heartwood:?}");
        assert_eq!(heartwood.documented.len(), 1, "{heartwood:?}");
        // Against a reference relay the reference's own text is expected.
        let reference = txgen(dir.path(), false).unwrap();
        assert_eq!(reference.failures.len(), 1, "{reference:?}");
        assert!(reference.documented.is_empty());
        // Another code is a failure wherever the transaction went.
        write(summary(json!([reason, code])));
        let both = txgen(dir.path(), true).unwrap();
        assert_eq!(both.documented.len(), 1, "{both:?}");
        assert_eq!(both.failures.len(), 1, "{both:?}");
        assert!(both.failures[0].starts_with("invalid-other-code"));
        // txgen's other conditions still count.
        let mut s = summary(json!([reason]));
        s["keptTypesMissing"] = json!(["2/2"]);
        write(s);
        let missing = txgen(dir.path(), true).unwrap();
        assert!(
            missing
                .failures
                .iter()
                .any(|f| f.contains("kept types missing")),
            "{missing:?}"
        );
        write(summary(json!([])));
        let clean = txgen(dir.path(), false).unwrap();
        assert!(
            clean.failures.is_empty() && clean.documented.is_empty(),
            "{clean:?}"
        );
    }

    #[test]
    fn pair_runs_compare_answers_and_wallets() {
        let a = vec![("s1 x".to_owned(), json!({"outcome": "accepted"}))];
        let b = vec![(
            "s1 x".to_owned(),
            json!({"outcome": "rejected", "code": "ERR_X"}),
        )];
        assert_eq!(compare_submissions(&a, &a), Submissions::default());
        assert_eq!(compare_submissions(&a, &b).differences.len(), 1);
        let early = |n: u32| {
            vec![(
                "revoke-too-early x".to_owned(),
                json!({"outcome": "rejected", "code": "ERR_APPLY", "message": format!("because {n} more blocks must be produced")}),
            )]
        };
        let timing = compare_submissions(&early(105), &early(106));
        assert!(
            timing.differences.is_empty() && timing.timing.len() == 1,
            "{timing:?}"
        );
        assert_eq!(
            without_numbers("dA#4 c4dc8a23: 105 more"),
            "dA## c#dc#a#: # more"
        );
        // Two refusals of a transaction that does not decode, each implementation with its reason.
        let decode = |reason: &str| {
            vec![(
                "invalid-version-2 x".to_owned(),
                json!({"outcome": "rejected", "code": "ERR_BAD_DATA", "message": format!("Invalid transaction data: {reason}")}),
            )]
        };
        let d = compare_submissions(
            &decode("unsupported transaction version 2"),
            &decode("Version 2 not supported"),
        );
        assert!(d.differences.is_empty() && d.documented.len() == 1, "{d:?}");
        // Another code, or another verdict, is a difference.
        let mut other = decode("x");
        other[0].1["code"] = json!("ERR_APPLY");
        assert_eq!(
            compare_submissions(&other, &decode("y")).differences.len(),
            1
        );
        let w = json!({"wallets": [{"address": "dA", "balance": "1"}]});
        let v = json!({"wallets": [{"address": "dA", "balance": "2"}, {"address": "dB"}]});
        assert!(compare_wallets(&w, &w).is_empty());
        assert_eq!(compare_wallets(&w, &v).len(), 2);
    }
}
