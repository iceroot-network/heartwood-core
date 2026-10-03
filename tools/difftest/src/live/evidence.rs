//! The route corpora's failed cases of a live run judged again with the run's own evidence: a
//! listing's tie with the places of the wallets one node of the corpus's pair alone re-indexed
//! left free inside their ties ([`api::judge::outcome_with_free_places`]), the parties of the
//! transactions the run's records show sent to one of the two nodes and refused there by their
//! handler ([`nodes::refused_alone`]) before the corpus was recorded.
//!
//! A case that passes so is recorded with the wallets and their evidence; every other case stays
//! as it was judged, and evidence that cannot be read, or a corpus whose recording has no time, is
//! a failure.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::nodes;
use super::run::{self, Manifest};
use crate::api;
use crate::report::{Outcome, Report};

/// What the evidence of one route corpus showed: the record of the cases it passed, or why it
/// could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// What it concerns.
    pub label: String,
    /// Why the evidence could not be read; `None` for a record.
    pub failure: Option<String>,
}

impl Note {
    fn record(label: String) -> Note {
        Note {
            label,
            failure: None,
        }
    }

    fn failure(label: String, why: impl Into<String>) -> Note {
        Note {
            label,
            failure: Some(why.into()),
        }
    }
}

/// The failed cases of `report` (the route corpora of the run in `run_dir`, whose `run.json` is
/// `manifest`) judged again with the run's evidence, and what the evidence showed.
pub fn route_corpora(
    run_dir: &Path,
    manifest: &Manifest,
    mut report: Report,
) -> (Report, Vec<Note>) {
    let mut notes = Vec::new();
    let Some((left, right)) = &manifest.api else {
        return (report, notes);
    };
    let main = (left.as_str(), right.as_str(), api::RUN_ANSWERS_DIR);
    let extra =
        (manifest.api_extra.iter()).map(|e| (e.left.as_str(), e.right.as_str(), e.dir.as_str()));
    let mut steps = None;
    for (left, right, dir) in std::iter::once(main).chain(extra) {
        let dir = run_dir.join(dir);
        let failing: Vec<usize> = (report.results.iter().enumerate())
            .filter(|(_, r)| r.outcome.is_failure() && Path::new(&r.file).starts_with(&dir))
            .map(|(i, _)| i)
            .collect();
        if failing.is_empty() {
            continue;
        }
        let label = format!("the wallets {left} or {right} alone re-indexed");
        let steps = match &steps {
            Some(steps) => steps,
            None => match run::steps(run_dir) {
                Ok(read) => steps.insert(read),
                Err(e) => {
                    notes.push(Note::failure(label, e.to_string()));
                    continue;
                }
            },
        };
        // The submissions before the corpus was recorded: its step names its directory.
        let recorded = (steps.iter())
            .find(|s| s.text("dir").is_some_and(|d| run_dir.join(d) == dir))
            .and_then(|s| s.start);
        if recorded.is_none() {
            let message = format!("no step records when {} was recorded", dir.display());
            notes.push(Note::failure(label, message));
            continue;
        }
        let evidence = match nodes::refused_alone(run_dir, manifest, steps, (left, right), recorded)
        {
            Ok(evidence) if evidence.is_empty() => continue,
            Ok(evidence) => evidence,
            Err(e) => {
                notes.push(Note::failure(label, e.to_string()));
                continue;
            }
        };
        let free = evidence.keys().cloned().collect();
        let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let (mut judged, mut moved) = (0, BTreeSet::new());
        for i in failing {
            let Some(line) = recorded_line(&mut files, &report.results[i]) else {
                continue;
            };
            let (outcome, freed) = api::judge::outcome_with_free_places(&line, &free);
            if outcome == Outcome::Pass {
                report.results[i].outcome = Outcome::Pass;
                judged += 1;
                moved.extend(freed);
            }
        }
        if judged > 0 {
            let reasons: Vec<String> = (moved.iter())
                .map(|a| format!("{a}: {}", evidence[a].join("; ")))
                .collect();
            notes.push(Note::record(format!(
                "{left} against {right}: {judged} listings equal with the places of {} left free \
                 inside their ties ({})",
                moved.iter().cloned().collect::<Vec<_>>().join(", "),
                reasons.join(" | ")
            )));
        }
    }
    (report, notes)
}

/// The recorded request of the case `result` (its file's line), read once per file into
/// `files`; `None` when the line does not read as one.
fn recorded_line(
    files: &mut BTreeMap<String, Vec<String>>,
    result: &crate::report::CaseResult,
) -> Option<api::Recorded> {
    let lines = files.entry(result.file.clone()).or_insert_with(|| {
        let text = std::fs::read_to_string(&result.file).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    });
    let text = lines.get(result.line.checked_sub(1)?)?;
    serde_json::from_str::<api::Recorded>(text).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// A listing case: its name, node 1's page and node 3's (each wallet an address and a
    /// balance).
    type Listing<'a> = (&'a str, &'a [(&'a str, u64)], &'a [(&'a str, u64)]);

    /// A run whose route corpus pairs node 1 (under test) with node 3, with txgen's submissions to
    /// node 3, a vote node 1 alone refused, and the listings `cases` (name, node 1's page, node
    /// 3's page; each wallet an address and a balance).
    fn listing_run(cases: &[Listing<'_>]) -> crate::vectors::testutil::TestDir {
        let dir = crate::vectors::testutil::TestDir::new("free-places");
        let root = dir.path();
        let run = json!({"format": run::FORMAT, "run": "mixed-split", "network": "n", "seed": "4",
            "nodes": [
                {"name": "node1", "ip": "10.0.0.11", "implementation": "heartwood"},
                {"name": "node3", "ip": "10.0.0.13", "implementation": "reference"}],
            "api": {"left": "node1", "right": "node3"}});
        std::fs::write(root.join("run.json"), run.to_string()).unwrap();
        let steps = [
            json!({"step": "txgen-once", "node": "node3", "ok": true, "submit": "node3"}),
            json!({"step": "vote-a", "node": "node1", "ok": true,
                "start": "2026-10-02T00:59:30.000Z", "detail": {"id": "tx-v",
                "outcome": "rejected", "code": "ERR_APPLY", "wallet": "V"}}),
            json!({"step": "api-corpus", "node": "node1", "ok": true,
                "start": "2026-10-02T01:00:00.000Z", "dir": api::RUN_ANSWERS_DIR}),
            // A refusal after the corpus was recorded moved nothing it holds.
            json!({"step": "vote-b", "node": "node1", "ok": true,
                "start": "2026-10-02T01:00:05.000Z", "detail": {"id": "tx-l",
                "outcome": "rejected", "code": "ERR_APPLY", "wallet": "L"}}),
        ];
        let lines = |values: &[Value]| values.iter().map(|v| format!("{v}\n")).collect::<String>();
        std::fs::write(root.join("steps.jsonl"), lines(&steps)).unwrap();
        let submitted = |id: &str, sender: &str, response: Value, to: &[&str]| {
            json!({"event": "submitted", "id": id, "sender": sender, "response": response,
                "ts": "2026-10-02T00:59:00.000Z",
                "tx": {"asset": {"transfers": to.iter().map(|t| json!({"recipientId": t})).collect::<Vec<_>>()}}})
        };
        let txgen = [
            json!({"event": "start", "api": "http://10.0.0.13:6003"}),
            submitted(
                "tx-g",
                "G",
                json!({"outcome": "rejected", "code": "ERR_APPLY"}),
                &["R"],
            ),
            submitted(
                "tx-b",
                "B",
                json!({"outcome": "rejected", "code": "ERR_LOW_FEE"}),
                &[],
            ),
            submitted(
                "tx-c",
                "C",
                json!({"outcome": "accepted", "broadcast": true}),
                &[],
            ),
        ];
        std::fs::create_dir_all(root.join("txgen/once")).unwrap();
        std::fs::write(root.join("txgen/once/txgen.jsonl"), lines(&txgen)).unwrap();
        let page = |list: &[(&str, u64)]| {
            let data: Vec<Value> = (list.iter())
                .map(|(a, b)| json!({"address": a, "balance": b.to_string()}))
                .collect();
            json!({"status": 200, "headers": {"connection": "close", "x-block-height": "265"},
                "body": {"meta": {"count": data.len(), "totalCount": data.len()}, "data": data}})
        };
        let answers: Vec<Value> = (cases.iter())
            .map(|(name, left, right)| {
                json!({"file": "05-wallets.json", "name": name,
                    "request": {"method": "GET", "path": "/api/wallets"},
                    "left": page(left), "right": page(right)})
            })
            .collect();
        std::fs::create_dir_all(root.join(api::RUN_ANSWERS_DIR)).unwrap();
        std::fs::write(
            root.join(api::RUN_ANSWERS_DIR).join(api::ANSWERS),
            lines(&answers),
        )
        .unwrap();
        dir
    }

    /// The route corpora of the run in `run_dir` judged plainly, then with the run's evidence.
    fn judged(run_dir: &Path) -> (Report, Vec<Note>) {
        let manifest = Manifest::load(run_dir).unwrap();
        let report = api::compare(&api::ApiCompareArgs::Recorded {
            run: run_dir.to_path_buf(),
        })
        .unwrap();
        route_corpora(run_dir, &manifest, report)
    }

    fn outcome(report: &Report, name: &str) -> Outcome {
        let r = report.results.iter().find(|r| r.label == name).unwrap();
        r.outcome.clone()
    }

    #[test]
    fn a_tie_place_one_node_s_own_refusal_moved_is_free_and_nothing_else_is() {
        let wallets = |order: &str| -> Vec<(&'static str, u64)> {
            (order.split(' '))
                .map(|a| match a {
                    "A" => ("A", 100),
                    "D" => ("D", 1),
                    "G" => ("G", 50),
                    "V" => ("V", 50),
                    "B" => ("B", 50),
                    "C" => ("C", 50),
                    _ => ("L", 50),
                })
                .collect()
        };
        let ours = wallets("A G V B C L D");
        let dir = listing_run(&[
            // G's transfer node 3 alone refused: its place in the tie is free.
            ("moved-g", &ours, &wallets("A V B C G L D")),
            // V's vote node 1 alone refused: so is its place.
            ("moved-v", &ours, &wallets("A G B C V L D")),
            // B's refusal came before the handler: its place is not free.
            ("moved-b", &ours, &wallets("A G V C B L D")),
            // C's transaction was accepted and broadcast: neither is its place.
            ("moved-c", &ours, &wallets("A C G V B L D")),
            // L's refusal came after the corpus was recorded.
            ("moved-l", &ours, &wallets("A L G V B C D")),
            // G moved and the other wallets out of their order too.
            ("moved-g-and-more", &ours, &wallets("A V C B G L D")),
        ]);
        let run_dir = dir.path();
        let manifest = Manifest::load(run_dir).unwrap();
        let report = api::compare(&api::ApiCompareArgs::Recorded {
            run: run_dir.to_path_buf(),
        })
        .unwrap();
        assert_eq!(
            report
                .results
                .iter()
                .filter(|r| r.outcome.is_failure())
                .count(),
            6
        );
        let (report, notes) = route_corpora(run_dir, &manifest, report);
        assert_eq!(outcome(&report, "moved-g"), Outcome::Pass);
        assert_eq!(outcome(&report, "moved-v"), Outcome::Pass);
        for name in ["moved-b", "moved-c", "moved-l", "moved-g-and-more"] {
            assert!(outcome(&report, name).is_failure(), "{name}");
        }
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].failure.is_none());
        assert!(
            notes[0]
                .label
                .contains("2 listings equal with the places of G, V left free")
                && notes[0]
                    .label
                    .contains("transaction tx-g refused by node3 (ERR_APPLY), never sent to node1")
                && notes[0]
                    .label
                    .contains("transaction tx-v refused by node1 (ERR_APPLY), never sent to node3"),
            "{notes:?}"
        );
        // Evidence that cannot be read fails rather than frees: no node for txgen's submissions.
        std::fs::write(run_dir.join("steps.jsonl"), "").unwrap();
        std::fs::write(
            run_dir.join("txgen/once/txgen.jsonl"),
            "{\"event\":\"submitted\",\"id\":\"tx-g\"}\n",
        )
        .unwrap();
        let (report, notes) = judged(run_dir);
        assert!(report.results.iter().all(|r| r.outcome.is_failure()));
        assert!(notes.iter().any(|n| {
            n.failure.is_some()
                && n.label
                    .contains("the wallets node1 or node3 alone re-indexed")
        }));
    }

    #[test]
    fn a_refusal_whose_time_or_whose_corpus_s_is_unknown_frees_nothing() {
        let wallets = |order: &str| -> Vec<(&'static str, u64)> {
            (order.split(' '))
                .map(|a| match a {
                    "A" => ("A", 100),
                    "D" => ("D", 1),
                    "G" => ("G", 50),
                    "T" => ("T", 50),
                    "B" => ("B", 50),
                    _ => ("L", 50),
                })
                .collect()
        };
        let ours = wallets("A G B L T D");
        let dir = listing_run(&[
            ("moved-g", &ours, &wallets("A B L G T D")),
            ("moved-l", &ours, &wallets("A L G B T D")),
            ("moved-t", &ours, &wallets("A T G B L D")),
        ]);
        let run_dir = dir.path();
        // The recipient a transaction names outside its transfers is a party too: T's place is
        // free with G's.
        let txgen = run_dir.join("txgen/once/txgen.jsonl");
        let text = std::fs::read_to_string(&txgen).unwrap();
        std::fs::write(
            &txgen,
            text.replace(
                r#""asset":{"transfers":["#,
                r#""recipientId":"T","asset":{"transfers":["#,
            ),
        )
        .unwrap();
        let (report, _) = judged(run_dir);
        assert_eq!(outcome(&report, "moved-g"), Outcome::Pass);
        assert_eq!(outcome(&report, "moved-t"), Outcome::Pass);
        assert!(outcome(&report, "moved-l").is_failure());
        // A submission without its time is no evidence that it came before the corpus.
        let text = std::fs::read_to_string(&txgen).unwrap();
        std::fs::write(
            &txgen,
            text.replace(r#""ts":"2026-10-02T00:59:00.000Z","#, ""),
        )
        .unwrap();
        let (report, _) = judged(run_dir);
        assert!(outcome(&report, "moved-g").is_failure());
        // A corpus whose recording has no time: no refusal can be placed before it, and the
        // evidence fails rather than freeing the refusal after it.
        let steps = run_dir.join("steps.jsonl");
        let text = std::fs::read_to_string(&steps).unwrap();
        std::fs::write(
            &steps,
            text.replace(r#""start":"2026-10-02T01:00:00.000Z","#, ""),
        )
        .unwrap();
        let (report, notes) = judged(run_dir);
        assert!(outcome(&report, "moved-l").is_failure());
        assert!(
            notes.iter().any(|n| n.failure.is_some()
                && n.label
                    .contains("the wallets node1 or node3 alone re-indexed")),
            "{notes:?}"
        );
    }
}
