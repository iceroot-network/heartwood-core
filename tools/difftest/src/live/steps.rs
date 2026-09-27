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
        "r" | "h" => &[
            "peers-node4",
            "txgen-fund",
            "txgen-once",
            "batch-node4",
            "vote-before-node4",
            "vote-after-node4",
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
        _ => &[],
    };
    common.iter().chain(own).map(|s| (*s).to_owned()).collect()
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

/// txgen's own verdict in `dir` (its `summary.json`): pass, fail and submission counts.
pub fn txgen(dir: &Path) -> Result<std::result::Result<String, String>> {
    let summary = read_json(&dir.join("summary.json"))?;
    let n = |k: &str| summary.get(k).and_then(Value::as_u64).unwrap_or(0);
    let text = format!(
        "{} submissions: {} passed, {} failed, {} scenarios skipped",
        n("submissions"),
        n("pass"),
        n("fail"),
        n("skipped")
    );
    Ok(
        if summary.get("ok") == Some(&Value::Bool(true)) && n("fail") == 0 {
            Ok(text)
        } else {
            Err(text)
        },
    )
}

/// Each scripted submission of txgen's records in `dir`: scenario and id, and the node's answer
/// (outcome, code, message), in submission order.
pub fn submissions(dir: &Path) -> Result<Vec<(String, Value)>> {
    let mut out = Vec::new();
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
    // txgen.jsonl.4 is the oldest rotated file, txgen.jsonl the newest.
    files.sort_by_key(|p: &std::path::PathBuf| {
        std::cmp::Reverse(
            p.extension()
                .and_then(|e| e.to_str())
                .and_then(|e| e.parse::<u32>().ok())
                .unwrap_or(0),
        )
    });
    for file in files {
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
/// only in their numbers, which is recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Submissions {
    /// Differences in the submissions or their answers.
    pub differences: Vec<String>,
    /// Answers equal but for the numbers of their messages.
    pub timing: Vec<String>,
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
/// node under test (`left`) against the control (`right`): for each case and repetition the same
/// status, the same 429 body and the same header names. `Ok` holds a summary.
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
    for ((case, i), l) in &left {
        let Some(r) = right.get(&(case.clone(), *i)) else {
            problems.push(format!("{case} request {i}: no answer of the control"));
            continue;
        };
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
    let limited = left
        .values()
        .filter(|a| a.get("status").and_then(Value::as_u64) == Some(429))
        .count();
    Ok(if problems.is_empty() {
        Ok(format!(
            "{} requests each, {limited} answered 429 on both",
            left.len()
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
        let w = json!({"wallets": [{"address": "dA", "balance": "1"}]});
        let v = json!({"wallets": [{"address": "dA", "balance": "2"}, {"address": "dB"}]});
        assert!(compare_wallets(&w, &w).is_empty());
        assert_eq!(compare_wallets(&w, &v).len(), 2);
    }
}
