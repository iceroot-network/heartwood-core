//! The fresh sync of a mixed run (run `w`'s node 5, run `s`'s node 4): a reference relay with an
//! empty store synced from one relay alone while the chain advanced; its block ids, the judge's
//! lines over its chain, and its round and missed-slot rows equal the serving chain's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use heartwood_database::{Database, Settings};

use super::mixed::{self, Poll};
use super::nodes::{MissedKey, Rows, copy_dir, rows_of};
use super::run;
use super::steps;
use super::{Cases, MixedRun};
use crate::corpus::{self, Corpus};
use crate::errors::{Error, Result};
use crate::live::digest::{self, JudgedChain};
use crate::store::{self, ScratchDir};

const SECTION: &str = "fresh";

/// Seconds after the serving relay answered again by which the fresh node must have caught up.
const CATCH_UP_SECONDS: i64 = 60;

/// The rows a chain is compared by: round rows as sets, missed rows sorted.
pub(super) struct Expected {
    /// The round rows, per round.
    pub rounds: BTreeMap<u64, BTreeSet<(String, String)>>,
    /// The missed-slot rows.
    pub missed: Vec<MissedKey>,
    /// The heights whose missed rows are not compared.
    pub refused: BTreeSet<u64>,
    /// Where the rows came from, for the messages.
    pub from: String,
}

/// The rows of the serving chain: the corpus's reference rows when it has them, else the rows of
/// the serving relay's store.
pub(super) fn expected(
    run: &MixedRun<'_>,
    serving: &str,
    dir: &Path,
    tip: u32,
) -> Result<Expected> {
    if dir.join("reference/rounds.jsonl").exists() {
        let Rows {
            rounds,
            missed,
            refused,
            ..
        } = rows_of(dir)?;
        return Ok(Expected {
            rounds,
            missed,
            refused,
            from: "node 1's archive".to_owned(),
        });
    }
    let data = run.run_dir.join("nodes").join(serving).join("data");
    let copy = ScratchDir::new("live-fresh-store")?;
    copy_dir(&data, copy.path())?;
    let database = store::guarded(|| Database::open(&Settings::in_data_dir(copy.path())))
        .map_err(|store::Unbuilt(m)| Error::NotYet(format!("not built yet: {m}")))?
        .map_err(|e| Error::Usage(format!("{serving}'s store does not open: {e}")))?;
    let rows = store::read_rows(
        &database,
        tip,
        store::last_round(tip, run.manifest.round_length),
    )
    .map_err(Error::Usage)?;
    Ok(Expected {
        rounds: rows
            .rounds
            .iter()
            .map(|(r, v)| {
                (
                    *r,
                    v.iter()
                        .map(|x| {
                            (
                                x.public_key.clone(),
                                serde_json::Value::String(x.balance.to_string()).to_string(),
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
        missed: rows
            .missed
            .iter()
            .map(|m| (m.timestamp, u64::from(m.height), m.username.clone()))
            .collect(),
        refused: BTreeSet::new(),
        from: format!("{serving}'s store"),
    })
}

/// How the rows of a node's archive differ from the expected rows: every round the node has rows
/// for equal, and the missed rows of the heights up to `tip` (leaving out the heights either side
/// refused) equal.
pub(super) fn row_problems(ours: &Rows, want: &Expected, tip: u32) -> Vec<String> {
    let mut problems = Vec::new();
    if ours.rounds.is_empty() {
        problems.push("the archive holds no round rows to compare".to_owned());
    }
    for (round, rows) in &ours.rounds {
        match want.rounds.get(round) {
            Some(w) if w == rows => {}
            Some(_) => problems.push(format!(
                "the round rows of round {round} differ from {}",
                want.from
            )),
            None => problems.push(format!("round {round} has rows, {} has none", want.from)),
        }
    }
    let skip: BTreeSet<u64> = ours.refused.union(&want.refused).copied().collect();
    let keep = |rows: &[MissedKey]| {
        let mut r: Vec<_> = rows
            .iter()
            .filter(|r| r.1 <= u64::from(tip) && !skip.contains(&r.1))
            .cloned()
            .collect();
        r.sort();
        r
    };
    if keep(&ours.missed) != keep(&want.missed) {
        problems.push(format!("the missed rows differ from {}", want.from));
    }
    problems
}

/// The fresh sync's checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    mixed::guard(
        cases,
        SECTION,
        "the fresh sync from one relay alone",
        |cases| {
            let m = run.manifest;
            let (Some(fresh), Some(serving)) = (mixed::fresh_node(m), mixed::serving_node(m))
            else {
                cases.record(
                SECTION,
                if m.run == "w0" {
                    "run w0 is the baseline: its node 5 syncs from every peer, as the reference does"
                } else {
                    "this run has no fresh sync"
                },
            );
                return Ok(());
            };
            let label = format!("{fresh}: its archive");
            let dirs = run::archives(&run.run_dir.join("archives").join(fresh))?;
            if dirs.is_empty() {
                cases.fail(SECTION, label, format!("no archive of {fresh}"));
                return Ok(());
            }
            let scratch = ScratchDir::new("live-fresh")?;
            let mine_dir = scratch.path().join(fresh);
            corpus::import(&dirs, &mine_dir, &m.seed).map_err(|e| {
                Error::Usage(format!("the archive of {fresh} does not import: {e}"))
            })?;
            let mine = Corpus::load(&mine_dir)?;
            let chains = mixed::chains(run)?;
            let last = chains.last();
            let theirs = Corpus::load(&last.dir)?;
            // Its block ids: the serving chain's at every height, to the paused tip.
            let first_differing = (1..=mine.tip().max(theirs.tip())).find(|h| {
                mine.block_id(*h).map(|i| i.to_hex()) != theirs.block_id(*h).map(|i| i.to_hex())
            });
            match first_differing {
                Some(h) if h > mine.tip() => cases.fail(
                    SECTION,
                    format!("{fresh}: its block ids"),
                    format!(
                        "its archive ends at height {}, below the serving chain's paused tip {}",
                        mine.tip(),
                        theirs.tip()
                    ),
                ),
                None => cases.pass(
                    SECTION,
                    format!(
                        "{fresh}: its {} block ids equal the serving chain's",
                        mine.tip()
                    ),
                ),
                Some(h) => cases.fail(
                    SECTION,
                    format!("{fresh}: its block ids"),
                    format!(
                        "the chains differ at height {h}: tip {} against the serving chain's {}",
                        mine.tip(),
                        theirs.tip()
                    ),
                ),
            }
            if first_differing.is_some() {
                return Ok(());
            }
            // The judge's lines: the chains are one, so the serving relay's lines are the judge's.
            let judged = JudgedChain {
                name: fresh.to_owned(),
                forward: last
                    .chain
                    .forward
                    .iter()
                    .filter(|(h, _)| **h <= mine.tip())
                    .map(|(h, l)| (*h, l.clone()))
                    .collect(),
            };
            let heartwood = mixed::heartwood(m).iter().any(|n| n.name == serving);
            let result = if heartwood {
                let digest =
                    digest::read(&run.run_dir.join("nodes").join(serving).join("difftest"))?;
                Some(digest::judge(&digest, std::slice::from_ref(&judged)))
            } else {
                cases.record(
                    SECTION,
                    format!("{serving} is a reference relay (a dry run of the recipe): it writes no digest lines to compare"),
                );
                None
            };
            if let Some(result) = result {
                if result.problems.is_empty() {
                    cases.pass(
                    SECTION,
                    format!(
                        "{fresh}: the judge's lines over its chain are {serving}'s ({} compared)",
                        result.compared.values().sum::<usize>()
                    ),
                );
                } else {
                    cases.fail(
                        SECTION,
                        format!("{fresh}: the judge's lines"),
                        result.problems[0].clone(),
                    );
                }
            }
            // Its round and missed rows.
            let want = expected(run, serving, &last.dir, last.tip)?;
            let ours = rows_of(&mine_dir)?;
            let problems = row_problems(&ours, &want, mine.tip());
            if problems.is_empty() {
                cases.pass(
                    SECTION,
                    format!(
                        "{fresh}: its round rows and missed rows equal {}",
                        want.from
                    ),
                );
            } else {
                for p in problems {
                    cases.fail(SECTION, format!("{fresh}: its rows"), p);
                }
            }
            peers(run, cases, fresh, serving)?;
            catch_up(run, cases, fresh, serving)
        },
    )
}

/// Until when the fresh node's peer list must hold the serving relay alone: run `w`'s node 5 for
/// the whole run; run `s`'s node 4 until its routes were opened (`Err` when the run records no
/// such step).
pub(super) fn alone_until(run: &str, steps: &[run::Step]) -> std::result::Result<i64, String> {
    if run != "s" {
        return Ok(i64::MAX);
    }
    mixed::step(steps, "open-routes-node4")
        .and_then(|s| s.start)
        .ok_or_else(|| "the run records no open-routes-node4 step with its start".to_owned())
}

/// The fresh node's peer list holds the serving relay alone until its routes changed.
fn peers(run: &MixedRun<'_>, cases: &mut Cases, fresh: &str, serving: &str) -> Result<()> {
    let label = format!("{fresh}: its peer list");
    let ip = mixed::ip_of(run.manifest, serving).unwrap_or("").to_owned();
    let steps = run::steps(run.run_dir)?;
    let until = match alone_until(&run.manifest.run, &steps) {
        Ok(t) => t,
        Err(why) => {
            cases.fail(SECTION, label, why);
            return Ok(());
        }
    };
    let polls = mixed::polls(run.run_dir)?;
    let mine: Vec<&Poll> = polls
        .iter()
        .filter(|p| p.node == fresh && p.at < until)
        .collect();
    if mine.is_empty() {
        cases.fail(SECTION, label, format!("no poll of {fresh}'s peer list"));
        return Ok(());
    }
    let others: BTreeSet<&str> = mine
        .iter()
        .flat_map(|p| p.peers.iter().map(|e| e.ip.as_str()))
        .filter(|p| *p != ip)
        .collect();
    if others.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "{fresh}: {serving} was its only peer in {} polls",
                mine.len()
            ),
        );
    } else {
        cases.fail(
            SECTION,
            label,
            format!(
                "it also listed {}",
                others.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
    }
    Ok(())
}

/// Run `w`: the fresh node held the control's tip, within one block, a minute after the serving
/// relay answered again.
fn catch_up(run: &MixedRun<'_>, cases: &mut Cases, fresh: &str, serving: &str) -> Result<()> {
    if run.manifest.run != "w" {
        return Ok(());
    }
    let label =
        format!("{fresh}: caught up within {CATCH_UP_SECONDS} s of {serving} answering again");
    let recovered = run
        .windows
        .iter()
        .filter(|w| w.class() == "restart" && w.nodes.iter().any(|n| n == serving))
        .map(|w| w.end.saturating_sub(run.manifest.window_seconds * 1_000))
        .max();
    let Some(recovered) = recovered else {
        cases.record(
            SECTION,
            format!("{serving} was not restarted: no catch-up to time"),
        );
        return Ok(());
    };
    let at = recovered + CATCH_UP_SECONDS * 1_000;
    let arrivals = steps::arrivals(&run.run_dir.join("propagation.jsonl"))?;
    let held = |node: &str| {
        arrivals
            .get(node)
            .and_then(|a| a.iter().filter(|(_, t)| **t <= at).map(|(h, _)| *h).max())
    };
    match (held(fresh), held(&run.manifest.control)) {
        (Some(ours), Some(control)) if ours + 1 >= control => cases.pass(
            SECTION,
            format!("{label}: at height {ours}, the control at {control}"),
        ),
        (Some(ours), Some(control)) => cases.fail(
            SECTION,
            label,
            format!(
                "at {} it held height {ours}, the control {control}",
                run::format_time(at)
            ),
        ),
        _ => cases.fail(
            SECTION,
            label,
            "no block arrival of the node or the control was recorded before then",
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_observer_of_run_s_is_alone_only_until_its_routes_open() {
        assert_eq!(alone_until("w", &[]), Ok(i64::MAX));
        assert!(alone_until("s", &[]).is_err());
        let opened = run::Step {
            name: "open-routes-node4".into(),
            node: "node4".into(),
            ok: true,
            start: Some(9_000),
            record: json!({"step": "open-routes-node4"}),
        };
        assert_eq!(alone_until("s", &[opened]), Ok(9_000));
    }

    #[test]
    fn an_archive_without_round_rows_is_no_evidence() {
        let empty = Rows {
            blocks: BTreeMap::new(),
            meta: BTreeMap::new(),
            rounds: BTreeMap::new(),
            missed: Vec::new(),
            refused: BTreeSet::new(),
        };
        let want = Expected {
            rounds: BTreeMap::new(),
            missed: Vec::new(),
            refused: BTreeSet::new(),
            from: "node 1's archive".into(),
        };
        let found = row_problems(&empty, &want, 10);
        assert!(
            found.iter().any(|p| p.contains("no round rows")),
            "{found:?}"
        );
    }
}
