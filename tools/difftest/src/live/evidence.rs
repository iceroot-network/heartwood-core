//! The route corpora's failed cases of a live run judged again with the run's own evidence:
//!
//! - a listing's tie with the places of the wallets one node of the corpus's pair alone
//!   re-indexed left free inside their ties ([`api::judge::outcome_with_free_places`]), the
//!   parties of the transactions the run's records show sent to one of the two nodes and refused
//!   there by their handler (`nodes::refused_alone`) before the corpus was recorded;
//! - a listing's tie between two reference nodes (`run.json` names both nodes of the corpus
//!   `reference`), with the place of every wallet free inside its tie: each reference node orders
//!   a tie by its own index, so both orders are the reference's;
//! - the answers of `/blocks/missed` of a Heartwood relay whose store differs from the control's
//!   by the rows one of its starts wrote for the slots before block 2 only, each at its slot's
//!   time and charged to the delegate the reference names for the slot (round 1's forging order,
//!   computed from the control's round rows and checked against every block of round 1), judged
//!   as the reference answers over each store's rows ([`api::missed`]): the reference's start
//!   writes the same rows when its store holds no missed-slot row, and the control never wrote
//!   them.
//!
//! A case that passes so is recorded with the wallets and their evidence; every other case stays
//! as it was judged, and evidence that cannot be read, or a corpus whose recording has no time, is
//! a failure.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use super::run::{self, Implementation, Manifest};
use super::{digest, nodes, recovery};
use crate::api;
use crate::api::missed::MissedSlot;
use crate::report::{CaseResult, Outcome, Report};
use crate::store::ScratchDir;

/// The operation of a report's case that holds a note of the evidence ([`Note`]).
pub const EVIDENCE: &str = "evidence";

/// The step of the earlier recipe that records the run's one route corpus, in `api/`, without
/// naming its directory.
const CORPUS_STEP: &str = "api-corpus";

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
/// `manifest`) judged again with the run's evidence, with one case for each note: operation
/// [`EVIDENCE`], the file the run's `run.json`, a record passing (its label `recorded: ...`) and
/// evidence that cannot be read failing.
pub fn judged_with_evidence(run_dir: &Path, manifest: &Manifest, report: Report) -> Report {
    let (mut report, notes) = route_corpora(run_dir, manifest, report);
    let file = run_dir.join("run.json").display().to_string();
    for note in notes {
        let (label, outcome) = match note.failure {
            None => (format!("recorded: {}", note.label), Outcome::Pass),
            Some(why) => (note.label, Outcome::HarnessError(why)),
        };
        report.results.push(CaseResult {
            file: file.clone(),
            line: 0,
            op: EVIDENCE.to_owned(),
            label,
            height: 0,
            outcome,
        });
    }
    report
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
    let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut steps = None;
    for (left, right, dir) in std::iter::once(main).chain(extra) {
        let dir = run_dir.join(dir);
        // The corpus's failed cases, each with its recorded request.
        let failing: Vec<(usize, api::Recorded)> = (0..report.results.len())
            .filter(|&i| {
                let r = &report.results[i];
                r.outcome.is_failure() && Path::new(&r.file).starts_with(&dir)
            })
            .filter_map(|i| recorded_line(&mut files, &report.results[i]).map(|line| (i, line)))
            .collect();
        let corpus = Corpus {
            run_dir,
            manifest,
            pair: (left, right),
            dir: &dir,
        };
        let listings: Vec<&(usize, api::Recorded)> = (failing.iter())
            .filter(|(_, line)| api::judge::is_listing(line))
            .collect();
        if !listings.is_empty() {
            let note = if corpus.runs(left, Implementation::Reference)
                && corpus.runs(right, Implementation::Reference)
            {
                reference_ties(&mut report, &listings, (left, right))
            } else {
                refusal_places(&corpus, &mut steps, &mut report, &listings)
            };
            notes.extend(note);
        }
        let missed: Vec<&(usize, api::Recorded)> = (failing.iter())
            .filter(|(_, line)| api::missed::is_missed_blocks(line))
            .collect();
        if !missed.is_empty() {
            notes.extend(backfilled_rows(&corpus, &mut report, &missed));
        }
    }
    (report, notes)
}

/// A route corpus of the run: the run's directory and `run.json`, its pair of nodes (under test,
/// control) and its directory.
struct Corpus<'a> {
    run_dir: &'a Path,
    manifest: &'a Manifest,
    pair: (&'a str, &'a str),
    dir: &'a Path,
}

impl Corpus<'_> {
    /// Whether the run's node `name` runs `implementation`.
    fn runs(&self, name: &str, implementation: Implementation) -> bool {
        (self.manifest.node(name)).is_some_and(|n| n.implementation == implementation)
    }
}

/// What shows that a Heartwood relay's store differs from the control's by the rows its start
/// wrote for the slots before block 2 only, as the reference's start writes them
/// (`calculateMissedBlocks`) when the store holds no missed-slot row: both stores' rows, the
/// network's epoch, the rows the start wrote and the tip it started at.
struct Backfill {
    ours: Vec<MissedSlot>,
    theirs: Vec<MissedSlot>,
    epoch_ms: i64,
    written: usize,
    start: u32,
}

/// The failed `/blocks/missed` answers `missed` of a corpus whose node under test is a Heartwood
/// relay and whose control is a reference node, judged again as the reference answers over each
/// store's rows ([`api::missed::outcome_over_rows`]), when the relay's store differs from the
/// control's by the rows a start of the relay wrote for the slots before block 2 only
/// ([`backfill_evidence`]); what the evidence showed, if anything.
fn backfilled_rows(
    corpus: &Corpus<'_>,
    report: &mut Report,
    missed: &[&(usize, api::Recorded)],
) -> Option<Note> {
    let (left, right) = corpus.pair;
    if !corpus.runs(left, Implementation::Heartwood)
        || !corpus.runs(right, Implementation::Reference)
    {
        return None;
    }
    let evidence = match backfill_evidence(corpus) {
        Ok(evidence) => evidence?,
        Err(why) => {
            let label = format!("the rows {left}'s start wrote for the slots before block 2");
            return Some(Note::failure(label, why));
        }
    };
    let mut judged = 0;
    for (i, line) in missed {
        let outcome = api::missed::outcome_over_rows(
            line,
            &evidence.ours,
            &evidence.theirs,
            evidence.epoch_ms,
        );
        if outcome == Outcome::Pass {
            report.results[*i].outcome = Outcome::Pass;
            judged += 1;
        }
    }
    (judged > 0).then(|| {
        Note::record(format!(
            "{left} against {right}: {judged} answers of /blocks/missed equal the reference's \
             answers over {left}'s rows, which hold the {} rows its start at {} wrote for the \
             slots before block 2, at the times and with the delegates (round 1's at each slot's \
             place) the reference's start writes when its store holds no missed-slot row \
             ({right}'s store holds none)",
            evidence.written, evidence.start
        ))
    })
}

/// The evidence of a corpus's back-filled rows ([`Backfill`]): the control's rows from its
/// archive (imported anew), the relay's from its store, and the start of the relay that found no
/// missed-slot row ([`nodes::backfilling_start`]). `Ok(None)` when no start of the relay
/// back-filled or the stores differ otherwise; an error when the evidence cannot be read.
fn backfill_evidence(corpus: &Corpus<'_>) -> Result<Option<Backfill>, String> {
    let (left, right) = corpus.pair;
    let archives =
        run::archives(&corpus.run_dir.join("archives").join(right)).map_err(|e| e.to_string())?;
    if archives.is_empty() {
        return Err(format!("no archive of {right}, the control"));
    }
    let scratch = ScratchDir::new("live-backfill").map_err(|e| e.to_string())?;
    let dir = scratch.path().join(right);
    crate::corpus::import(&archives, &dir, &corpus.manifest.seed).map_err(|e| e.to_string())?;
    let control = crate::corpus::Corpus::load(&dir).map_err(|e| e.to_string())?;
    let epoch_ms = (control.meta.pointer("/chain/epoch").and_then(Value::as_str))
        .and_then(run::parse_time)
        .ok_or_else(|| format!("the archive of {right} names no epoch"))?;
    let rows = nodes::rows_of(&dir).map_err(|e| e.to_string())?;
    let expected = recovery::backfill_rows(
        &control,
        corpus.manifest.block_time,
        corpus.manifest.round_length,
    )?;
    let read = digest::read(&corpus.run_dir.join("nodes").join(left).join("difftest"))
        .map_err(|e| e.to_string())?;
    let starts = digest::starts_of(&read);
    let Some(start) = nodes::backfilling_start(&starts, &rows.missed, &rows.refused, false) else {
        return Ok(None);
    };
    let ours = nodes::store_missed(corpus.run_dir, left)?;
    let Some(written) = backfill_of(&ours, &rows.missed, &expected) else {
        return Ok(None);
    };
    let slots = |rows: &[nodes::MissedKey]| -> Vec<MissedSlot> {
        (rows.iter())
            .map(|(timestamp, height, username)| MissedSlot {
                timestamp: *timestamp,
                height: *height,
                username: username.clone(),
            })
            .collect()
    };
    Ok(Some(Backfill {
        ours: slots(&ours),
        theirs: slots(&rows.missed),
        epoch_ms,
        written,
        start,
    }))
}

/// The number of rows `ours` holds beyond `theirs` when they are the rows a start writes for the
/// slots before block 2, `expected` (each slot's time and delegate): every row of `theirs` in
/// `ours`, and each other row at height 2, one at each of `expected`'s times and no other. `None`
/// otherwise, or when `expected` is empty.
fn backfill_of(
    ours: &[nodes::MissedKey],
    theirs: &[nodes::MissedKey],
    expected: &[(i64, String)],
) -> Option<usize> {
    let mut extra: Vec<&nodes::MissedKey> = ours.iter().collect();
    for row in theirs {
        let at = extra.iter().position(|ours| *ours == row)?;
        extra.swap_remove(at);
    }
    let mut written: Vec<(i64, &str)> = (extra.iter())
        .map(|(timestamp, height, name)| (*height == 2).then_some((*timestamp, name.as_str())))
        .collect::<Option<_>>()?;
    written.sort_unstable();
    let mut wanted: Vec<(i64, &str)> = (expected.iter())
        .map(|(time, name)| (*time, name.as_str()))
        .collect();
    wanted.sort_unstable();
    (!wanted.is_empty() && written == wanted).then_some(written.len())
}

/// The failed listings `listings` of `report` judged again with the places of the wallets one
/// node of the corpus's pair alone re-indexed free inside their ties (the run's `steps`, read
/// once): what the evidence showed, if anything.
fn refusal_places(
    corpus: &Corpus<'_>,
    steps: &mut Option<Vec<run::Step>>,
    report: &mut Report,
    listings: &[&(usize, api::Recorded)],
) -> Option<Note> {
    let (left, right) = corpus.pair;
    let label = format!("the wallets {left} or {right} alone re-indexed");
    let steps = match steps {
        Some(steps) => steps,
        None => match run::steps(corpus.run_dir) {
            Ok(read) => steps.insert(read),
            Err(e) => return Some(Note::failure(label, e.to_string())),
        },
    };
    // The submissions before the corpus was recorded: its step names its directory, or, in the
    // earlier recipe, is the one route corpus step, which records `api/`.
    let recorded = (steps.iter())
        .find(|s| match s.text("dir") {
            Some(d) => corpus.run_dir.join(d) == corpus.dir,
            None => {
                s.name == CORPUS_STEP && corpus.dir == corpus.run_dir.join(api::RUN_ANSWERS_DIR)
            }
        })
        .and_then(|s| s.start);
    if recorded.is_none() {
        let message = format!("no step records when {} was recorded", corpus.dir.display());
        return Some(Note::failure(label, message));
    }
    let evidence = match nodes::refused_alone(
        corpus.run_dir,
        corpus.manifest,
        steps,
        corpus.pair,
        recorded,
    ) {
        Ok(evidence) if evidence.is_empty() => return None,
        Ok(evidence) => evidence,
        Err(e) => return Some(Note::failure(label, e.to_string())),
    };
    let free = evidence.keys().cloned().collect();
    let (mut judged, mut moved) = (0, BTreeSet::new());
    for (i, line) in listings {
        let (outcome, freed) = api::judge::outcome_with_free_places(line, &free);
        if outcome == Outcome::Pass {
            report.results[*i].outcome = Outcome::Pass;
            judged += 1;
            moved.extend(freed);
        }
    }
    (judged > 0).then(|| {
        let reasons: Vec<String> = (moved.iter())
            .map(|a| format!("{a}: {}", evidence[a].join("; ")))
            .collect();
        Note::record(format!(
            "{left} against {right}: {judged} listings equal with the places of {} left free \
             inside their ties ({})",
            moved.iter().cloned().collect::<Vec<_>>().join(", "),
            reasons.join(" | ")
        ))
    })
}

/// The failed listings `listings` of `report`, a corpus of two reference nodes (`pair`), judged
/// again with the place of every wallet free inside its tie: both answers are the reference's, so
/// any tie order is the reference's own. Whatever else differs still fails.
fn reference_ties(
    report: &mut Report,
    listings: &[&(usize, api::Recorded)],
    (left, right): (&str, &str),
) -> Option<Note> {
    let mut judged = 0;
    for (i, line) in listings {
        let free = api::judge::listed_wallets(line);
        if api::judge::outcome_with_free_places(line, &free).0 == Outcome::Pass {
            report.results[*i].outcome = Outcome::Pass;
            judged += 1;
        }
    }
    (judged > 0).then(|| {
        Note::record(format!(
            "{left} against {right}: {judged} listings equal with the place of every wallet free \
             inside its tie (both answers are the reference's, and each reference node orders a \
             tie by its own index)"
        ))
    })
}

/// The recorded request of the case `result` (its file's line), read once per file into
/// `files`; `None` when the line does not read as one.
fn recorded_line(
    files: &mut BTreeMap<String, Vec<String>>,
    result: &CaseResult,
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

    /// A handler's refusal, after which a node re-indexed the transaction's parties.
    const HANDLER_TEXT: &str = "G#3 0f32267a DelegateResignation v3 cannot be applied: Failed to apply transaction, because the wallet has not resigned as a delegate";
    /// A refusal at the pool's entry, which the handler never saw.
    const POOL_ENTRY_TEXT: &str =
        "P#2 46f7033b Vote v3 cannot be applied: P already has a vote transaction in the pool";

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
                "outcome": "rejected", "code": "ERR_APPLY", "wallet": "V", "message": HANDLER_TEXT}}),
            // A refusal at the pool's entry, before the handler: it moved nothing.
            json!({"step": "vote-p", "node": "node1", "ok": true,
                "start": "2026-10-02T00:59:40.000Z", "detail": {"id": "tx-p",
                "outcome": "rejected", "code": "ERR_APPLY", "wallet": "P", "message": POOL_ENTRY_TEXT}}),
            json!({"step": "api-corpus", "node": "node1", "ok": true,
                "start": "2026-10-02T01:00:00.000Z", "dir": api::RUN_ANSWERS_DIR}),
            // A refusal after the corpus was recorded moved nothing it holds.
            json!({"step": "vote-b", "node": "node1", "ok": true,
                "start": "2026-10-02T01:00:05.000Z", "detail": {"id": "tx-l",
                "outcome": "rejected", "code": "ERR_APPLY", "wallet": "L", "message": HANDLER_TEXT}}),
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
                json!({"outcome": "rejected", "code": "ERR_APPLY", "message": HANDLER_TEXT}),
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

    /// The route corpora of the run in `run_dir` judged as recorded.
    fn plain(run_dir: &Path) -> Report {
        api::judge_recorded(run_dir, None).unwrap().0
    }

    /// The route corpora of the run in `run_dir` judged as recorded, then with the run's
    /// evidence.
    fn judged(run_dir: &Path) -> (Report, Vec<Note>) {
        let manifest = Manifest::load(run_dir).unwrap();
        route_corpora(run_dir, &manifest, plain(run_dir))
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
                    "P" => ("P", 50),
                    _ => ("L", 50),
                })
                .collect()
        };
        let ours = wallets("A G V B C L D");
        let entry = wallets("A P B D");
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
            // P's vote node 1 alone refused at the pool's entry, before its handler: P was never
            // re-indexed, so its place is not free.
            ("moved-p", &entry, &wallets("A B P D")),
            // G moved and the other wallets out of their order too.
            ("moved-g-and-more", &ours, &wallets("A V C B G L D")),
        ]);
        let run_dir = dir.path();
        let manifest = Manifest::load(run_dir).unwrap();
        let report = plain(run_dir);
        assert_eq!(
            report
                .results
                .iter()
                .filter(|r| r.outcome.is_failure())
                .count(),
            7
        );
        let (report, notes) = route_corpora(run_dir, &manifest, report);
        assert_eq!(outcome(&report, "moved-g"), Outcome::Pass);
        assert_eq!(outcome(&report, "moved-v"), Outcome::Pass);
        for name in [
            "moved-b",
            "moved-c",
            "moved-l",
            "moved-p",
            "moved-g-and-more",
        ] {
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

    /// A run whose route corpora pair node 4 with node 1 (`api/`) and node 6 with node 1
    /// (`api-node6/`), every node of `implementations` (node 1, 4 and 6 in that order), no
    /// transaction refused, and the wallet listings `cases` in node 6's corpus (name, node 6's
    /// page, node 1's page).
    fn pair_run(
        implementations: [&str; 3],
        cases: &[Listing<'_>],
    ) -> crate::vectors::testutil::TestDir {
        let dir = crate::vectors::testutil::TestDir::new("reference-ties");
        let root = dir.path();
        let node = |name: &str, implementation: &str| {
            json!({"name": name, "ip": format!("10.0.0.1{}", &name[4..]),
                "implementation": implementation})
        };
        let run = json!({"format": run::FORMAT, "run": "w0", "network": "n", "seed": "s",
            "nodes": [node("node1", implementations[0]), node("node4", implementations[1]),
                node("node6", implementations[2])],
            "api": {"left": "node4", "right": "node1"},
            "apiExtra": [{"left": "node6", "right": "node1", "dir": "api-node6"}]});
        std::fs::write(root.join("run.json"), run.to_string()).unwrap();
        let steps = [
            json!({"step": "api-corpus", "node": "node4", "ok": true,
                "start": "2026-10-02T01:00:00.000Z", "dir": api::RUN_ANSWERS_DIR}),
            json!({"step": "api-corpus-node6", "node": "node6", "ok": true,
                "start": "2026-10-02T01:00:10.000Z", "dir": "api-node6"}),
        ];
        let lines = |values: &[Value]| values.iter().map(|v| format!("{v}\n")).collect::<String>();
        std::fs::write(root.join("steps.jsonl"), lines(&steps)).unwrap();
        let page = |list: &[(&str, u64)]| {
            let data: Vec<Value> = (list.iter())
                .map(|(a, b)| json!({"address": a, "balance": b.to_string(), "nonce": "0"}))
                .collect();
            json!({"status": 200, "headers": {"connection": "close", "x-block-height": "212"},
                "body": {"meta": {"count": data.len(), "totalCount": data.len()}, "data": data}})
        };
        let answers: Vec<Value> = (cases.iter())
            .map(|(name, left, right)| {
                json!({"file": "wallets-list.json", "name": name,
                    "request": {"method": "GET", "path": "/api/wallets"},
                    "left": page(left), "right": page(right)})
            })
            .collect();
        for corpus in [api::RUN_ANSWERS_DIR, "api-node6"] {
            std::fs::create_dir_all(root.join(corpus)).unwrap();
            let written = if corpus == "api-node6" {
                lines(&answers)
            } else {
                String::new()
            };
            std::fs::write(root.join(corpus).join(api::ANSWERS), written).unwrap();
        }
        dir
    }

    #[test]
    fn a_tie_between_two_reference_nodes_may_fall_in_either_order() {
        let wallets = |order: &str| -> Vec<(&'static str, u64)> {
            (order.split(' '))
                .map(|a| match a {
                    "A" => ("A", 100),
                    "D" => ("D", 1),
                    "P" => ("P", 50),
                    "Z" => ("Z", 50),
                    _ => ("Q", 50),
                })
                .collect()
        };
        let ours = wallets("A P Z Q D");
        let cases: [Listing<'_>; 3] = [
            // Node 6 and node 1 order the wallets of one balance otherwise: both are the
            // reference's own orders.
            ("tied", &ours, &wallets("A Z P Q D")),
            ("tied-three", &ours, &wallets("A Q Z P D")),
            // A wallet across another balance is no tie.
            ("across", &ours, &wallets("A P Z D Q")),
        ];
        let dir = pair_run(["reference", "reference", "reference"], &cases);
        let (report, notes) = judged(dir.path());
        assert_eq!(outcome(&report, "tied"), Outcome::Pass);
        assert_eq!(outcome(&report, "tied-three"), Outcome::Pass);
        assert!(outcome(&report, "across").is_failure());
        assert!(
            notes.iter().any(|n| n.failure.is_none()
                && n.label.contains("node6 against node1: 2 listings equal")
                && n.label.contains("both answers are the reference's")),
            "{notes:?}"
        );
        // A value that differs is a difference whatever the order.
        let richer = [("A", 100), ("Z", 51), ("P", 50), ("Q", 50), ("D", 1)];
        let dir = pair_run(
            ["reference", "reference", "reference"],
            &[("richer", &ours, &richer)],
        );
        let (report, _) = judged(dir.path());
        assert!(outcome(&report, "richer").is_failure());
        // A Heartwood node against the reference keeps its tie order, without evidence.
        let dir = pair_run(["reference", "reference", "heartwood"], &cases);
        let (report, _) = judged(dir.path());
        for name in ["tied", "tied-three", "across"] {
            assert!(outcome(&report, name).is_failure(), "{name}");
        }
    }

    /// An observer run of the earlier recipe: txgen's submissions to node 4 alone (one refused by
    /// its handler for wallet G), and a route corpus whose step names no directory, then the
    /// listing `ours` (node 4's) against `theirs` (node 1's).
    fn observer_run(
        ours: &[(&str, u64)],
        theirs: &[(&str, u64)],
    ) -> crate::vectors::testutil::TestDir {
        let dir = crate::vectors::testutil::TestDir::new("observer-evidence");
        let root = dir.path();
        let run = json!({"format": run::FORMAT, "run": "h", "network": "n", "seed": "s",
            "nodes": [
                {"name": "node1", "ip": "10.0.0.11", "implementation": "reference"},
                {"name": "node4", "ip": "10.0.0.14", "implementation": "heartwood",
                    "underTest": true}],
            "api": {"left": "node4", "right": "node1"}});
        std::fs::write(root.join("run.json"), run.to_string()).unwrap();
        let steps = [
            json!({"step": "txgen-once", "node": "node4", "ok": true, "read": "node1",
                "submit": "node4", "start": "2026-10-02T00:58:00.000Z"}),
            json!({"step": "api-corpus", "node": "node4", "ok": true, "left": "node4",
                "right": "node1", "start": "2026-10-02T01:00:00.000Z"}),
        ];
        let lines = |values: &[Value]| values.iter().map(|v| format!("{v}\n")).collect::<String>();
        std::fs::write(root.join("steps.jsonl"), lines(&steps)).unwrap();
        let txgen = [
            json!({"event": "start", "api": "http://127.0.0.1:7991"}),
            json!({"event": "submitted", "id": "tx-g", "sender": "G",
                "ts": "2026-10-02T00:59:00.000Z", "tx": {"type": 7},
                "response": {"outcome": "rejected", "code": "ERR_APPLY", "message": HANDLER_TEXT}}),
        ];
        std::fs::create_dir_all(root.join("txgen/once")).unwrap();
        std::fs::write(root.join("txgen/once/txgen.jsonl"), lines(&txgen)).unwrap();
        let page = |list: &[(&str, u64)]| {
            let data: Vec<Value> = (list.iter())
                .map(|(a, b)| json!({"address": a, "balance": b.to_string()}))
                .collect();
            json!({"status": 200, "headers": {"connection": "close", "x-block-height": "159"},
                "body": {"meta": {"count": data.len(), "totalCount": data.len()}, "data": data}})
        };
        let answer = json!({"file": "wallets-list.json", "name": "wallets/list/all",
            "request": {"method": "GET", "path": "/api/wallets"},
            "left": page(ours), "right": page(theirs)});
        std::fs::create_dir_all(root.join(api::RUN_ANSWERS_DIR)).unwrap();
        std::fs::write(
            root.join(api::RUN_ANSWERS_DIR).join(api::ANSWERS),
            lines(&[answer]),
        )
        .unwrap();
        dir
    }

    #[test]
    fn a_run_s_recorded_answers_are_judged_with_its_evidence_by_api_compare() {
        // Node 4 alone refused G's transaction and moved G to the end of its tie.
        let ours = [("A", 100), ("B", 50), ("C", 50), ("G", 50), ("D", 1)];
        let theirs = [("A", 100), ("G", 50), ("B", 50), ("C", 50), ("D", 1)];
        let dir = observer_run(&ours, &theirs);
        let report = api::compare(&api::ApiCompareArgs::Recorded {
            run: dir.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(
            outcome(&report, "wallets/list/all"),
            Outcome::Pass,
            "{report}"
        );
        assert!(
            report.results.iter().any(|r| r.op == EVIDENCE
                && r.outcome == Outcome::Pass
                && r.label.contains("places of G left free")
                && r.label.contains(
                    "transaction tx-g refused by node4 (ERR_APPLY), never sent to node1"
                )),
            "{report}"
        );
        // Another wallet moved: a failure, whatever the evidence.
        let moved = [("A", 100), ("C", 50), ("B", 50), ("G", 50), ("D", 1)];
        let dir = observer_run(&moved, &theirs);
        let report = api::compare(&api::ApiCompareArgs::Recorded {
            run: dir.path().to_path_buf(),
        })
        .unwrap();
        assert!(outcome(&report, "wallets/list/all").is_failure());
        // A recording without a time frees nothing, and says why.
        let dir = observer_run(&ours, &theirs);
        let steps = dir.path().join("steps.jsonl");
        let text = std::fs::read_to_string(&steps).unwrap();
        std::fs::write(
            &steps,
            text.replace(r#""start":"2026-10-02T01:00:00.000Z""#, r#""x":0"#),
        )
        .unwrap();
        let report = api::compare(&api::ApiCompareArgs::Recorded {
            run: dir.path().to_path_buf(),
        })
        .unwrap();
        assert!(outcome(&report, "wallets/list/all").is_failure());
        assert!(
            report
                .results
                .iter()
                .any(|r| r.op == EVIDENCE && r.outcome.is_failure()),
            "{report}"
        );
    }

    #[test]
    fn a_store_differs_by_the_back_fill_only_when_every_extra_row_is_its() {
        let row = |timestamp: i64, height: u64, username: &str| -> nodes::MissedKey {
            (timestamp, height, username.to_owned())
        };
        let theirs = vec![row(1224, 151, "tx1n2290"), row(1704, 210, "tx1n2290")];
        let times: Vec<(i64, String)> = [(8, "genesis_41"), (16, "genesis_44"), (24, "genesis_51")]
            .map(|(time, name)| (time, name.to_owned()))
            .to_vec();
        let times = times.as_slice();
        let backfill = [
            row(8, 2, "genesis_41"),
            row(16, 2, "genesis_44"),
            row(24, 2, "genesis_51"),
        ];
        let ours: Vec<nodes::MissedKey> = theirs.iter().chain(&backfill).cloned().collect();
        assert_eq!(backfill_of(&ours, &theirs, times), Some(3));
        // A back-filled slot charged to another delegate than the reference's start names.
        let mut renamed = ours.clone();
        renamed[3].2 = "genesis_45".to_owned();
        assert_eq!(backfill_of(&renamed, &theirs, times), None);
        let mut nobody = ours.clone();
        for row in &mut nobody[2..] {
            row.2 = "nobody".to_owned();
        }
        assert_eq!(backfill_of(&nobody, &theirs, times), None);
        // Without the back-fill, or with a slot of it missing, one twice or one more.
        assert_eq!(backfill_of(&theirs, &theirs, times), None);
        assert_eq!(backfill_of(&ours[..4], &theirs, times), None);
        let mut twice = ours.clone();
        twice.push(row(16, 2, "genesis_44"));
        assert_eq!(backfill_of(&twice, &theirs, times), None);
        let mut more = ours.clone();
        more.push(row(32, 2, "genesis_53"));
        assert_eq!(backfill_of(&more, &theirs, times), None);
        // A row at another height, or a row of the control's that the store lacks.
        let mut higher = ours.clone();
        higher[2].1 = 3;
        assert_eq!(backfill_of(&higher, &theirs, times), None);
        assert_eq!(backfill_of(&ours[1..], &theirs, times), None);
        // No slot before block 2: nothing to write.
        assert_eq!(backfill_of(&theirs, &theirs, &[]), None);
    }
}
