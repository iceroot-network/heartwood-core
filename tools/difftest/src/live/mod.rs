//! `difftest live check`: the comparisons of a live run's directory (the devnet tooling writes it):
//! node 1's archive as a corpus judged by the oracle, each Heartwood node's digest lines against
//! the judge's (a torn last line dropped), the store anchors, the step reports, the recorded API
//! answers, the ban scan with its fault windows, and the invariants.
//!
//! The run directory's layout is [`run`]'s. The checks, in the report's order:
//!
//! 1. `run`: the relay the run tested is the release binary of the tree being judged (the SHA-256
//!    `run.json` records for the image it started); the run finished, and every scripted step of
//!    its schedule ran and met its expectation
//!    ([`steps::required`]); txgen's own checks passed; for run `h`, each submission got the answer it
//!    got in run `r` and the scenario wallets at the pause equal run `r`'s (the run beside it, `../r`);
//! 2. `judge`: every chain of the run (node 1's archive, and the minority's archive from before a
//!    heal) imported under the corpus root (`target/corpus` of the current directory, as the
//!    Makefile's `CORPUS`) and judged (the oracle of `DIFFTEST_IMAGE` or `SOLAR_DIR` runs only when
//!    a corpus has no judge's lines yet); the judge's anchors hold;
//! 3. `nodes`: each node under test: a Heartwood relay's digest lines, store and logs
//!    (`nodes`), its starts against the run's scripted restarts of it, and that a kill during its
//!    initial sync ended its first start below node 1's height at its start; a reference relay's
//!    archive against node 1's;
//! 4. `api`: the route corpus's answers, judged by `api compare` ([`crate::api`]), and the rate
//!    limit's answers from an address that no node whitelists;
//! 5. `bans`: every ban of a node under test's address ([`bans`]);
//! 6. `invariants`: one tip at the pause, at or below the run's stop height; node 1's chain checked
//!    by the devnet tooling (every generator the scheduled one, the supply; not in runs `m` and `f`,
//!    whose relays do not serve the route the tooling reads); every block reached
//!    the nodes under test within one block time (node 4 of runs `r`, `h` and `m1`), outside their fault
//!    windows;
//! 7. recorded, not gated (outcome `Pass`, label `recorded: ...`): block lags, the joining node's
//!    sync time, the transaction propagation, socket-close bans inside fault windows, each node's
//!    refused and undone blocks.
//!
//! The runs of a forging devnet (`t`, the reference's; `m` and `f`, Heartwood's relays and forgers
//! on loopback addresses) have these checks too, and more:
//!
//! - `run`: a dry run of the recipe counts as no live evidence; runs `t` and `m` name their route
//!   corpus's nodes and hold both txgen summaries; run `m` (and run `t` with the variable
//!   [`COMPARE`]) compares with the other of the two beside it: complete, of the same seed and
//!   schedule, the funding and scenario submissions answered alike, the two scripted votes answered alike,
//!   and the scenario wallets equal but for their delegate attributes and vote weights;
//! - `judge`: the chain of runs `m` and `f` is node 1's block files, imported through
//!   [`crate::corpus_from_blocks`]; the reference's processor accepted every block of every chain;
//! - `invariants`: the one tip is the stop height, and every block reached every relay but node 1
//!   within one block time outside its fault windows;
//! - `forging` (runs `m` and `f`, [`forging`]): one applied chain on every relay, the processes and
//!   their listeners, the status polls, the attestations before every block, no slot with two
//!   blocks of one key, and run `f`'s faults.
//!
//! The runs of reference nodes beside Heartwood relays ([`MIXED_RUNS`]: `w`, `p` and `s`, and `w0`,
//! their reference baseline) have the checks of `mixed_checks` too: one chain, the judge, the
//! acceptance of the Heartwood relays, the reference and Heartwood logs per fault window's class
//! against the baseline, the fresh sync, the propagation, the fork of run `p`, the attestations of
//! run `s`, the recovery commands, AwaitBlock, the processes, the records and the schedule. The
//! baseline is the run `--compare` names, or [`COMPARE`]'s run beside this one.
//!
//! Besides the report, the check writes every case with its outcome to `live-check.txt` in the run
//! directory.

mod attest;
pub mod await_block;
pub mod bans;
mod corpora;
pub mod digest;
pub mod forging;
mod fresh;
mod mixed;
mod nodes;
mod recovery;
pub mod run;
mod schedule;
pub mod steps;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::api::{self, ApiCompareArgs};
use crate::cli::OracleArgs;
use crate::errors::{Error, Result};
use crate::oracle::Oracle;
use crate::report::{CaseResult, Outcome, Report};
use crate::store;
use crate::subject::Documented;
use run::{Implementation, Manifest};

/// Where the run's corpora go, relative to the current directory: the Makefile's `CORPUS`.
pub const CORPUS_ROOT: &str = "target/corpus";
/// The time a block may take to reach a node under test: one block time.
pub const LAG_LIMIT_SECONDS: f64 = 8.0;
/// The variable naming the run a check of run `t` or `m` compares with (the other of the two), as
/// `make live-check RUN=t COMPARE=m` gives it: make passes a variable of its command line to the
/// environment of its recipes. Run `m`'s `run.json` names run `t` as its pair already. Runs `w`,
/// `p` and `s` compare with their baseline `w0`.
pub const COMPARE: &str = "COMPARE";
/// The runs of reference nodes beside Heartwood relays, `w0` being their reference baseline.
pub const MIXED_RUNS: [&str; 4] = ["w0", "w", "p", "s"];

/// What a live check reads besides the run directory.
pub struct LiveOptions<'a> {
    /// Where the run's corpora are imported and judged.
    pub corpus_root: PathBuf,
    /// The oracle, asked for only when a corpus has not been judged yet.
    pub oracle: &'a dyn Fn() -> Result<Oracle>,
    /// The release binary of the tree being judged, which must be the relay the run tested.
    pub relay_binary: PathBuf,
    /// The run the run is compared with (`--compare`): the other of runs `t` and `m`, or the
    /// baseline `w0` of a mixed run.
    pub compare: Option<PathBuf>,
}

/// Check the live run in `run_dir`, with the corpora under [`CORPUS_ROOT`], the oracle of the
/// environment (`DIFFTEST_IMAGE` or `SOLAR_DIR`), the tree's release binary at
/// [`run::RELAY_BINARY`] and the run `compare` to compare with.
pub fn check(run_dir: &Path, compare: Option<&Path>) -> Result<Report> {
    let oracle = || OracleArgs::default().resolve(|name: &str| std::env::var(name).ok());
    check_with(
        run_dir,
        &LiveOptions {
            corpus_root: PathBuf::from(CORPUS_ROOT),
            oracle: &oracle,
            relay_binary: PathBuf::from(run::RELAY_BINARY),
            compare: compare.map(Path::to_path_buf),
        },
    )
}

/// What the checks of a mixed run ([`MIXED_RUNS`]) read.
pub struct MixedRun<'a> {
    /// The run's directory.
    pub run_dir: &'a Path,
    /// Its `run.json`.
    pub manifest: &'a Manifest,
    /// Its fault windows.
    pub windows: &'a [run::Window],
    /// The baseline run's directory, when the check compares with one.
    pub baseline: Option<&'a Path>,
    /// Where the run's corpora are imported and judged.
    pub corpus_root: &'a Path,
    /// The oracle, asked for only when a check needs it.
    pub oracle: &'a dyn Fn() -> Result<Oracle>,
}

/// The checks of a mixed run, in the report's order.
fn mixed_checks(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    mixed::one_chain(run, cases)?;
    mixed::judge(run, cases)?;
    mixed::acceptance(run, cases)?;
    mixed::reference_logs(run, cases)?;
    mixed::heartwood_logs(run, cases)?;
    fresh::check(run, cases)?;
    mixed::propagation(run, cases)?;
    mixed::fork(run, cases)?;
    attest::check(run, cases)?;
    recovery::check(run, cases)?;
    await_block::gate(run, cases)?;
    mixed::processes(run, cases)?;
    mixed::recorded(run, cases)?;
    schedule::check(run, cases)
}

/// The cases of a check, by section.
struct Cases {
    run: String,
    results: Vec<CaseResult>,
}

impl Cases {
    fn push(&mut self, section: &str, label: impl Into<String>, height: u32, outcome: Outcome) {
        self.results.push(CaseResult {
            file: format!("live-{}/{section}", self.run),
            line: 0,
            op: section.to_owned(),
            label: label.into(),
            height: u64::from(height),
            outcome,
        });
    }

    fn pass(&mut self, section: &str, label: impl Into<String>) {
        self.push(section, label, 0, Outcome::Pass);
    }

    fn fail(&mut self, section: &str, label: impl Into<String>, message: impl Into<String>) {
        self.push(section, label, 0, Outcome::HarnessError(message.into()));
    }

    /// A check that has no Rust implementation yet: a strict run fails on it.
    fn not_built(&mut self, section: &str, label: &str) {
        self.push(
            section,
            format!("{label} (not built)"),
            0,
            Outcome::NoSubject,
        );
    }

    fn record(&mut self, section: &str, label: impl AsRef<str>) {
        self.push(
            section,
            format!("recorded: {}", label.as_ref()),
            0,
            Outcome::Pass,
        );
    }
}

/// Check the live run in `run_dir` with `options`.
pub fn check_with(run_dir: &Path, options: &LiveOptions<'_>) -> Result<Report> {
    let mut manifest = Manifest::load(run_dir)?;
    let named = (options.compare.as_deref())
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned());
    let pair = named.or_else(|| std::env::var(COMPARE).ok().filter(|p| !p.is_empty()));
    // A mixed run's baseline is the run `--compare` names: its manifest's `pair` does not make it
    // a pair of runs `h` and `r`, whose submissions and wallets are compared one for one.
    if MIXED_RUNS.contains(&manifest.run.as_str()) {
        manifest.pair = None;
    }
    let mut baseline = None;
    if let Some(pair) = pair {
        match (manifest.run.as_str(), pair.as_str()) {
            ("t", "m") | ("m", "t") => manifest.pair = Some(pair),
            ("w" | "p" | "s", "w0") => {
                let beside = || run_dir.parent().unwrap_or(Path::new(".")).join(&pair);
                baseline = Some(options.compare.clone().unwrap_or_else(beside));
            }
            _ => {
                return Err(Error::Usage(format!(
                    "{COMPARE}={pair}: run {} compares with run t or m (T against M), or runs w, p and s with run w0",
                    manifest.run
                )));
            }
        }
    }
    if matches!(manifest.run.as_str(), "t" | "m" | "f") {
        let expected = if manifest.run == "f" { 7 } else { 5 };
        let names: std::collections::BTreeSet<_> = manifest.nodes.iter().map(|n| &n.name).collect();
        if manifest.round_length != 53
            || manifest.block_time != 8
            || manifest.stop_height != 265
            || manifest.nodes.len() != expected
            || names.len() != expected
            || manifest.nodes.iter().filter(|n| n.forger).count() != 5
        {
            return Err(Error::Usage(format!(
                "run {} is a forging devnet of five validators and {} nodes, 53 slots of 8 s a round, stopping at 265; its run.json has {} nodes ({} validators), rounds of {}, {} s slots and the stop height {}",
                manifest.run,
                expected,
                manifest.nodes.len(),
                manifest.nodes.iter().filter(|n| n.forger).count(),
                manifest.round_length,
                manifest.block_time,
                manifest.stop_height
            )));
        }
    }
    let mut cases = Cases {
        run: manifest.run.clone(),
        results: Vec::new(),
    };
    let mut windows = run::windows(run_dir, manifest.window_seconds)?;
    if MIXED_RUNS.contains(&manifest.run.as_str()) {
        windows.extend(run::minority_window(&manifest, &run::steps(run_dir)?));
    }
    // The route corpus first: whether its judge is built in this tree decides how a failed
    // recording of it counts.
    let mut api_cases = Cases {
        run: manifest.run.clone(),
        results: Vec::new(),
    };
    let api_built = check_api(run_dir, &manifest, &mut api_cases);
    // The relay the run tested must be this tree's: a run of an older build judges nothing here.
    match run::relay_binding(&manifest, &options.relay_binary) {
        Some(Ok(text)) => cases.pass("run", text),
        Some(Err(why)) => cases.fail("run", "the relay the run tested is this tree's", why),
        None => {}
    }
    check_run(run_dir, &manifest, api_built, &mut cases)?;
    let chains = check_judge(run_dir, &manifest, options, &mut cases);
    check_nodes(run_dir, &manifest, &chains, &windows, &mut cases)?;
    cases.results.extend(api_cases.results);
    check_bans(run_dir, &manifest, &windows, &mut cases)?;
    check_invariants(run_dir, &manifest, &windows, &mut cases)?;
    forging::check(run_dir, &manifest, &windows, &mut cases)?;
    if MIXED_RUNS.contains(&manifest.run.as_str()) {
        let mixed = MixedRun {
            run_dir,
            manifest: &manifest,
            windows: &windows,
            baseline: baseline.as_deref(),
            corpus_root: &options.corpus_root,
            oracle: options.oracle,
        };
        mixed_checks(&mixed, &mut cases)?;
    }
    let report = Report {
        title: match (&manifest.relay_image, &manifest.relay_binary_sha256) {
            (Some(image), Some(sha)) if manifest.has_heartwood() => format!(
                "Live check of run {} ({}; relay {image}, binary sha256 {})",
                manifest.run,
                manifest.network,
                sha.get(..12).unwrap_or(sha)
            ),
            _ => format!("Live check of run {} ({})", manifest.run, manifest.network),
        },
        source: format!(
            "{}{}; every case in {}",
            run_dir.display(),
            if manifest.dry_run {
                " (a dry run: the nodes under test are reference relays)"
            } else {
                ""
            },
            run_dir.join("live-check.txt").display()
        ),
        files: 1,
        results: cases.results,
    };
    write_listing(run_dir, &report)?;
    Ok(report)
}

fn write_listing(run_dir: &Path, report: &Report) -> Result<()> {
    let mut text = String::new();
    for r in &report.results {
        let verdict = match &r.outcome {
            o if o.is_failure() => "FAIL",
            Outcome::Documented(_) => "doc ",
            Outcome::Skipped(_) => "skip",
            _ => "ok  ",
        };
        let _ = write!(text, "{verdict} {:<11} {}", r.op, r.label);
        match &r.outcome {
            Outcome::HarnessError(m) | Outcome::Skipped(m) => {
                let _ = write!(text, ": {m}");
            }
            _ => {}
        }
        text.push('\n');
    }
    let _ = writeln!(
        text,
        "{} cases: {} failed, {} documented, {} skipped",
        report.results.len(),
        report.failed(),
        report.documented(),
        report.skipped()
    );
    let path = run_dir.join("live-check.txt");
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))
}

/// The steps that record the route corpus: a failed one is skipped, not failed, in a tree whose
/// route corpus is still a stub (a dry run of the recipe against the stubs).
const API_STEPS: [&str; 1] = ["api-corpus"];

fn check_run(
    run_dir: &Path,
    manifest: &Manifest,
    api_built: bool,
    cases: &mut Cases,
) -> Result<()> {
    if manifest.status == "complete" {
        cases.pass(
            "run",
            format!("the run finished (network {})", manifest.network),
        );
    } else {
        cases.fail(
            "run",
            "the run finished",
            format!("its status is {:?}", manifest.status),
        );
    }
    if manifest.stop_height > 5 * manifest.round_length {
        cases.fail(
            "run",
            "the schedule stops within 5 rounds",
            format!("its stop height is {}", manifest.stop_height),
        );
    }
    if matches!(manifest.run.as_str(), "t" | "m") && manifest.api.is_none() {
        cases.fail(
            "run",
            "the API corpus",
            "the run does not name its two API nodes",
        );
    }
    let mut steps = run::steps(run_dir)?;
    for s in &mut steps {
        let reason = s
            .record
            .get("skipped")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                (!s.ok && !api_built && API_STEPS.contains(&s.name.as_str()))
                    .then(|| "the route corpus is not built in this tree".to_owned())
            });
        if let Some(reason) = reason {
            s.ok = true;
            cases.push(
                "run",
                format!("step {} ({})", s.name, s.node),
                0,
                Outcome::Skipped(reason),
            );
        }
    }
    if matches!(manifest.run.as_str(), "t" | "m" | "f") && manifest.dry_run {
        cases.fail(
            "run",
            "a live run",
            "a dry run is command coverage, not live evidence",
        );
    }
    let txgen = run_dir.join("txgen");
    // Runs t and m hold both txgen records: the funding's `fund/fund.json` and the scenarios'
    // `once/summary.json` (judged below with every txgen summary).
    if matches!(manifest.run.as_str(), "t" | "m") {
        match run::read_json(&txgen.join("fund/fund.json")) {
            Ok(f) if f.get("ok") == Some(&Value::Bool(true)) => cases.pass(
                "run",
                format!(
                    "txgen fund: {} of {} test wallets funded",
                    f.get("funded").unwrap_or(&Value::Null),
                    f.get("testWallets").unwrap_or(&Value::Null)
                ),
            ),
            Ok(f) => cases.fail("run", "txgen fund", format!("fund.json: {f}")),
            Err(e) => cases.fail("run", "txgen fund", e.to_string()),
        }
        if !txgen.join("once/summary.json").exists() {
            cases.fail("run", "txgen once", "no once/summary.json");
        }
    }
    if let Ok(entries) = std::fs::read_dir(&txgen) {
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("summary.json").exists())
            .collect();
        dirs.sort();
        for dir in dirs {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // The node txgen submitted to (its step's `submit`): a Heartwood relay names its own
            // decoder's reason in an `ERR_BAD_DATA` refusal.
            let heartwood = steps
                .iter()
                .find(|s| s.name == format!("txgen-{name}"))
                .and_then(|s| s.record.get("submit").and_then(Value::as_str))
                .and_then(|n| manifest.node(n))
                .is_some_and(|n| n.implementation == Implementation::Heartwood);
            let verdict = steps::txgen(&dir, heartwood)?;
            // The step's own `ok` is txgen's exit status, which holds the reference's texts; the
            // verdict judged here takes its place.
            if let Some(s) = steps.iter_mut().find(|s| s.name == format!("txgen-{name}")) {
                s.ok = verdict.failures.is_empty();
            }
            for d in &verdict.documented {
                cases.push(
                    "run",
                    format!("txgen {name}: {d}"),
                    0,
                    Outcome::Documented(Documented::DecodeReason.reason().to_owned()),
                );
            }
            if verdict.failures.is_empty() {
                cases.pass("run", format!("txgen {name}: {}", verdict.text));
            }
            for f in verdict.failures.iter().take(20) {
                cases.fail("run", format!("txgen {name} ({})", verdict.text), f.clone());
            }
        }
    }
    let problems = steps::problems(&manifest.run, &steps);
    if problems.is_empty() {
        cases.pass(
            "run",
            format!(
                "{} steps met their expectations; the schedule is complete",
                steps.len()
            ),
        );
    }
    for p in problems {
        cases.fail("run", "the scripted steps", p);
    }
    check_reconnect(manifest, &steps, cases);
    if let Some(pair) = &manifest.pair {
        let other = run_dir.parent().map(|p| p.join(pair));
        match other.filter(|d| d.join("run.json").exists()) {
            None if matches!(manifest.run.as_str(), "t" | "m") => cases.fail(
                "run",
                "the paired run",
                format!("no run {pair} beside this one"),
            ),
            None => cases.push(
                "run",
                format!("the answers and wallets of run {pair}"),
                0,
                Outcome::Skipped(format!("no run {pair} beside this one")),
            ),
            Some(other) => {
                if matches!(manifest.run.as_str(), "t" | "m") {
                    let paired = Manifest::load(&other)?;
                    if paired.status != "complete"
                        || paired.dry_run
                        || paired.seed != manifest.seed
                        || paired.round_length != manifest.round_length
                        || paired.stop_height != manifest.stop_height
                    {
                        cases.fail(
                            "run",
                            "the paired run",
                            "the pair is incomplete or has a different seed or schedule",
                        );
                    }
                }
                let submissions = |dir: &Path| -> Result<Vec<(String, Value)>> {
                    let modes: &[&str] = if matches!(manifest.run.as_str(), "t" | "m") {
                        &["fund", "once"]
                    } else {
                        &["once"]
                    };
                    let mut all = Vec::new();
                    for mode in modes {
                        all.extend(
                            steps::submissions(&dir.join("txgen").join(mode))?
                                .into_iter()
                                .map(|(label, v)| (format!("{mode}: {label}"), v)),
                        );
                    }
                    Ok(all)
                };
                let ours = submissions(run_dir)?;
                let theirs = submissions(&other)?;
                if matches!(manifest.run.as_str(), "t" | "m") {
                    let other_steps = run::steps(&other)?;
                    for name in ["vote-before-node2", "vote-after-node2"] {
                        let a = steps.iter().find(|s| s.name == name);
                        let b = other_steps.iter().find(|s| s.name == name);
                        if a.zip(b).is_none_or(|(a, b)| {
                            !a.ok
                                || !b.ok
                                || ["outcome", "code", "message", "http"]
                                    .iter()
                                    .any(|k| a.record[*k] != b.record[*k])
                                || a.record["outcome"].is_null()
                        }) {
                            cases.fail(
                                "run",
                                "scripted vote answers",
                                format!("{name}: missing or unequal verdicts"),
                            );
                        }
                    }
                }
                let d = steps::compare_submissions(&ours, &theirs);
                if d.differences.is_empty() && !ours.is_empty() {
                    cases.pass(
                        "run",
                        format!(
                            "each of {} submissions got run {pair}'s answer ({} with another reason text)",
                            ours.len(),
                            d.documented.len()
                        ),
                    );
                } else if ours.is_empty() {
                    cases.fail(
                        "run",
                        format!("the submissions against run {pair}"),
                        "no submission recorded",
                    );
                }
                for p in d.differences.iter().take(20) {
                    cases.fail(
                        "run",
                        format!("the submissions against run {pair}"),
                        p.clone(),
                    );
                }
                // Two chains of their own: a message that names a height or a time differs in its
                // numbers only (recorded, as for runs r and h).
                for p in &d.timing {
                    cases.record(
                        "run",
                        format!("an answer whose message differs from run {pair}'s in its numbers only: {p}"),
                    );
                }
                for p in &d.documented {
                    cases.push(
                        "run",
                        format!("the submissions against run {pair}: {p}"),
                        0,
                        Outcome::Documented(Documented::DecodeReason.reason().to_owned()),
                    );
                }
                let w = (
                    run::read_json(&run_dir.join("wallets.json")),
                    run::read_json(&other.join("wallets.json")),
                );
                match w {
                    (Ok(a), Ok(b)) => {
                        let d = if matches!(manifest.run.as_str(), "t" | "m") {
                            steps::compare_scenario_wallets(&a, &b)
                        } else {
                            steps::compare_wallets(&a, &b)
                        };
                        if d.is_empty() {
                            let n = a.get("count").and_then(Value::as_u64).unwrap_or(0);
                            cases.pass(
                                "run",
                                format!("the {n} scenario wallets equal run {pair}'s at the pause"),
                            );
                        }
                        for p in d.iter().take(20) {
                            cases.fail(
                                "run",
                                format!("the scenario wallets against run {pair}"),
                                p.clone(),
                            );
                        }
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        cases.fail("run", "the scenario wallets", e.to_string())
                    }
                }
            }
        }
    }
    Ok(())
}

/// How the minority of a partition rejoined after the heal. The recipe restarts node 3's relay
/// when there is no single tip 120 s after the heal (`reconnect-minority`): node 3 keeps its side
/// of the fork while it holds a peer, which a reference relay as node 4 is (a dry run). A Heartwood
/// relay without a listener is no peer of node 3, which then falls back to its seeds by itself; a
/// run with such a relay in the minority must converge without that restart. The relay of the
/// mixed runs listens, so it keeps node 3 on its side as a reference relay does: there the restart
/// of node 3 (a reference relay) and the reconnect of a reference node are the recipe's recorded
/// workarounds, and a Heartwood node the recipe had to reconnect (`reconnect-<node>`) fails.
fn check_reconnect(manifest: &Manifest, steps: &[run::Step], cases: &mut Cases) {
    if !steps.iter().any(|s| s.name == "converged") {
        return;
    }
    let restarted = steps.iter().any(|s| s.name == "reconnect-minority");
    if MIXED_RUNS.contains(&manifest.run.as_str()) {
        for s in steps {
            let Some(node) = s
                .name
                .strip_prefix("reconnect-")
                .and_then(|n| manifest.node(n))
            else {
                continue;
            };
            if node.implementation == Implementation::Heartwood && !manifest.dry_run {
                cases.fail(
                    "run",
                    format!(
                        "{} followed the heal without a scripted reconnect",
                        node.name
                    ),
                    format!(
                        "the recipe reconnected {}, a Heartwood relay ({})",
                        node.name, s.name
                    ),
                );
            } else {
                cases.record(
                    "run",
                    format!(
                        "{} was reconnected by the recipe after the heal (a reference relay)",
                        node.name
                    ),
                );
            }
        }
        cases.record(
            "run",
            if restarted {
                "the minority rejoined the majority after the scripted restart of node 3's relay (a relay in the minority, of either implementation, keeps node 3 on its side)"
            } else {
                "the minority rejoined the majority after the heal by itself"
            },
        );
        return;
    }
    let heartwood = !manifest.dry_run
        && manifest.minority.as_ref().is_some_and(|(members, _)| {
            members.iter().any(|m| {
                manifest
                    .node(m)
                    .is_some_and(|n| n.implementation == Implementation::Heartwood)
            })
        });
    match (restarted, heartwood) {
        (false, _) => cases.record(
            "run",
            "the minority rejoined the majority after the heal by itself",
        ),
        (true, false) => cases.record(
            "run",
            "the minority rejoined the majority after the scripted restart of node 3's relay (a reference relay in the minority keeps node 3 on its side)",
        ),
        (true, true) => cases.fail(
            "run",
            "the minority rejoined the majority after the heal by itself",
            "it rejoined only after the scripted restart of node 3's relay, 120 s after the heal: with a Heartwood relay in the minority, node 3 holds no peer and falls back to its seeds by itself",
        ),
    }
}

/// The judged chains by corpus name.
type Chains = BTreeMap<String, corpora::Judged>;

fn check_judge(
    run_dir: &Path,
    manifest: &Manifest,
    options: &LiveOptions<'_>,
    cases: &mut Cases,
) -> Chains {
    let mut out = Chains::new();
    for (name, archives) in &manifest.corpora {
        let label = format!("{name}: imported from {archives} and judged");
        match corpora::prepare(
            run_dir,
            name,
            archives,
            &manifest.seed,
            &options.corpus_root,
            options.oracle,
        ) {
            Err(e) => cases.fail("judge", label, e.to_string()),
            Ok(judged) => {
                if matches!(manifest.run.as_str(), "t" | "m" | "f") {
                    let results = run::read_lines(&judged.dir.join(crate::corpus::TS_PROCESSOR));
                    match results {
                        Ok(rows) => {
                            let accepted: std::collections::BTreeSet<_> = rows
                                .iter()
                                .filter(|r| r["step"] == "apply" && r["result"] == "Accepted")
                                .filter_map(|r| r["h"].as_u64())
                                .collect();
                            if (2..=u64::from(judged.tip)).all(|h| accepted.contains(&h))
                                && !rows
                                    .iter()
                                    .any(|r| r.get("result").is_some_and(|x| x != "Accepted"))
                            {
                                cases.pass("judge", "the reference processor accepted every block's schedule and economics");
                            } else {
                                cases.fail(
                                    "judge",
                                    "reference processor acceptance",
                                    "missing or refused block results",
                                );
                            }
                        }
                        Err(e) => {
                            cases.fail("judge", "reference processor acceptance", e.to_string())
                        }
                    }
                }
                let how = match (judged.imported, judged.judged) {
                    (true, true) => "imported and judged now",
                    (false, true) => "judged now",
                    (true, false) | (false, false) => "judged before",
                };
                if judged.anchors_ok {
                    let absent = if judged.absent.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "; {} do not apply to a corpus of relay block files",
                            judged.absent.join(" and ")
                        )
                    };
                    cases.pass(
                        "judge",
                        format!(
                            "{name}: tip {}, the judge's anchors hold ({how}, {}){absent}",
                            judged.tip,
                            judged.dir.display()
                        ),
                    );
                } else {
                    let failed: Vec<String> = judged
                        .anchors
                        .get("anchors")
                        .and_then(Value::as_object)
                        .map(|a| {
                            a.iter()
                                .filter(|(_, v)| v.get("ok") != Some(&Value::Bool(true)))
                                .map(|(k, _)| k.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    cases.fail(
                        "judge",
                        label,
                        format!(
                            "the judge's anchors do not hold ({}); see {}/anchors.json",
                            failed.join(", "),
                            judged.dir.display()
                        ),
                    );
                }
                // The node's missed rows at heights it refused or undid a block, which the judge
                // leaves out of its round-row comparison: the reference writes them before it
                // validates a block (Heartwood writes none for such a block).
                for row in judged
                    .anchors
                    .pointer("/anchors/A2/excluded")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let h = row.get("height").and_then(Value::as_u64).unwrap_or(0);
                    cases.push(
                        "judge",
                        format!(
                            "{name}: missed row of {} at {} (height {h}) left out: its node refused or undid a block there",
                            row.get("username").and_then(Value::as_str).unwrap_or("?"),
                            row.get("timestamp").map(Value::to_string).unwrap_or_default()
                        ),
                        u32::try_from(h).unwrap_or(0),
                        Outcome::Documented(Documented::MissedRowsOfRefusedBlock.reason().to_owned()),
                    );
                }
                out.insert(name.clone(), judged);
            }
        }
    }
    check_both_sides(manifest, &out, cases);
    out
}

/// After a partition, both sides forged: the minority's chain has blocks of its own after the
/// height where it leaves the final chain.
fn check_both_sides(manifest: &Manifest, chains: &Chains, cases: &mut Cases) {
    let Some((_, name)) = &manifest.minority else {
        return;
    };
    let (Some(last), Some(minority)) = (final_chain(manifest, chains), chains.get(name)) else {
        return;
    };
    let fork = (1..=minority.tip).find(|h| {
        let id = minority.chain.forward[h]
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("");
        !last.chain.holds(*h, id)
    });
    match fork {
        Some(fork) => cases.push(
            "judge",
            format!(
                "both sides forged: {name} leaves the final chain at height {fork} with {} blocks of its own (to {}); the final chain goes on to {}",
                minority.tip - fork + 1,
                minority.tip,
                last.tip
            ),
            fork,
            Outcome::Pass,
        ),
        None => cases.fail(
            "judge",
            "both sides forged",
            format!("{name} holds no block of its own: every block up to its tip {} is the final chain's", minority.tip),
        ),
    }
}

/// The final chain's corpus: the one that is not the minority's.
fn final_chain<'c>(manifest: &Manifest, chains: &'c Chains) -> Option<&'c corpora::Judged> {
    let minority = manifest.minority.as_ref().map(|(_, c)| c.as_str());
    chains
        .iter()
        .find(|(name, _)| Some(name.as_str()) != minority)
        .map(|(_, j)| j)
}

fn check_nodes(
    run_dir: &Path,
    manifest: &Manifest,
    chains: &Chains,
    windows: &[run::Window],
    cases: &mut Cases,
) -> Result<()> {
    let Some(last) = final_chain(manifest, chains) else {
        cases.fail(
            "nodes",
            "the nodes under test",
            "the final chain was not judged, so no node can be checked",
        );
        return Ok(());
    };
    let faults: Vec<(String, Vec<String>)> = windows
        .iter()
        .map(|w| (w.fault.clone(), w.nodes.clone()))
        .collect();
    let steps = run::steps(run_dir)?;
    // The peers whose violations fail a relay: the reference nodes, and in a devnet of Heartwood
    // nodes every node and forger address (no violation is recorded against a devnet node).
    let forgers = forging::validators(run_dir).unwrap_or_default();
    let mut references: Vec<&str> = manifest
        .nodes
        .iter()
        .filter(|n| {
            matches!(manifest.run.as_str(), "m" | "f")
                || n.implementation == Implementation::Reference
        })
        .map(|n| n.ip.as_str())
        .collect();
    if matches!(manifest.run.as_str(), "m" | "f") {
        references.extend(forgers.iter().map(|v| v.forger.as_str()));
    }
    for node in manifest.under_test() {
        let checks = match node.implementation {
            Implementation::Heartwood => {
                let mut judged = vec![last.chain.clone()];
                if let Some((members, corpus)) = &manifest.minority
                    && members.contains(&node.name)
                {
                    match chains.get(corpus) {
                        Some(m) => judged.push(m.chain.clone()),
                        None => cases.fail(
                            "nodes",
                            format!("{}: the minority's chain", node.name),
                            format!("{corpus} was not judged"),
                        ),
                    }
                }
                // The kill of a node that joins with an empty store, during its sync.
                let kill = steps
                    .iter()
                    .find(|s| s.name == "kill-during-sync" && s.node == node.name);
                let mut c = nodes::heartwood_digest(run_dir, node, &judged, &faults, kill);
                c.extend(nodes::heartwood_store(
                    run_dir,
                    node,
                    &last.dir,
                    last.tip,
                    manifest.round_length,
                ));
                c.extend(nodes::heartwood_logs(run_dir, node, &references));
                c
            }
            Implementation::Reference => nodes::reference(run_dir, manifest, node, &last.dir),
        };
        for (label, height, outcome) in checks {
            cases.push("nodes", label, height, outcome);
        }
    }
    for node in &manifest.nodes {
        let path = run_dir
            .join("rejections")
            .join(format!("{}.jsonl", node.name));
        if let Ok(lines) = run::read_lines(&path) {
            let count = |kind: &str| {
                lines
                    .iter()
                    .filter(|l| l.get("kind").and_then(Value::as_str) == Some(kind))
                    .count()
            };
            if !lines.is_empty() {
                cases.record(
                    "nodes",
                    format!(
                        "{}: {} blocks refused, {} undone",
                        node.name,
                        count("refused"),
                        count("undone")
                    ),
                );
            }
        }
    }
    Ok(())
}

/// The route corpus and the rate limit; whether the route corpus's judge is built in this tree.
fn check_api(run_dir: &Path, manifest: &Manifest, cases: &mut Cases) -> bool {
    let Some((left, right)) = &manifest.api else {
        return true;
    };
    let mut built = true;
    let args = ApiCompareArgs::Recorded {
        run: run_dir.to_path_buf(),
    };
    match store::guarded(|| api::compare(&args)) {
        Err(unbuilt) => {
            built = false;
            cases.push(
                "api",
                format!("the route corpus, {left} against {right}"),
                0,
                Outcome::Skipped(format!("its judge is not built yet ({})", unbuilt.0)),
            );
        }
        Ok(Err(e)) => cases.fail(
            "api",
            format!("the route corpus, {left} against {right}"),
            e.to_string(),
        ),
        Ok(Ok(report)) => {
            let (compared, failed) = (report.compared(), report.failed());
            for r in report.results {
                if r.outcome != Outcome::Pass {
                    cases.push(
                        "api",
                        format!("{}: {}", r.file, r.label),
                        u32::try_from(r.height).unwrap_or(0),
                        r.outcome,
                    );
                }
            }
            if failed == 0 {
                cases.pass(
                    "api",
                    format!(
                        "the route corpus, {left} against {right}: {compared} queries compared"
                    ),
                );
            }
        }
    }
    match steps::rate_limit(&run_dir.join("rate-limit.jsonl")) {
        Err(e) => cases.fail("api", "the rate limit", e.to_string()),
        Ok(Ok(text)) => cases.pass(
            "api",
            format!("the rate limit, {left} against {right}: {text}"),
        ),
        Ok(Err(problems)) => {
            for p in problems.iter().take(20) {
                cases.fail("api", "the rate limit", p.clone());
            }
        }
    }
    built
}

fn check_bans(
    run_dir: &Path,
    manifest: &Manifest,
    windows: &[run::Window],
    cases: &mut Cases,
) -> Result<()> {
    let path = run_dir.join("bans.jsonl");
    if !path.exists() {
        cases.fail(
            "bans",
            "the ban scan",
            "no bans.jsonl: the scan did not run",
        );
        return Ok(());
    }
    let records = run::read_lines(&path)?;
    let bans: Vec<bans::Ban> = records.iter().filter_map(bans::Ban::from_json).collect();
    if bans.len() != records.len() {
        cases.fail("bans", "the ban scan", "a malformed ban record");
    }
    // A node's addresses: its own, and its forger's when that is another (node 1 of run f).
    let forgers = forging::validators(run_dir).unwrap_or_default();
    let address_matches = |node: &run::Node, ip: &str| {
        ip == node.ip
            || forgers
                .iter()
                .any(|v| v.name == node.name && !v.forger.is_empty() && v.forger == ip)
    };
    for node in manifest.under_test() {
        let mut inside: BTreeMap<String, usize> = BTreeMap::new();
        let mut bad = 0usize;
        for ban in bans.iter().filter(|b| address_matches(node, &b.ip)) {
            match bans::classify(ban, &node.name, windows) {
                bans::Verdict::InWindow(fault) => {
                    *inside
                        .entry(format!("{} ({fault})", ban.reason))
                        .or_default() += 1
                }
                bans::Verdict::OutsideWindow => {
                    bad += 1;
                    cases.fail(
                        "bans",
                        format!("{}: no socket-close ban outside a fault window", node.name),
                        format!(
                            "{} banned it at {} outside every fault window: {}",
                            ban.node,
                            ban.at.map(run::format_time).unwrap_or_default(),
                            ban.reason
                        ),
                    );
                }
                bans::Verdict::Refusal => {
                    bad += 1;
                    cases.fail(
                        "bans",
                        format!("{}: no request-refusal or nes-termination ban", node.name),
                        format!("{} banned it: {}", ban.node, ban.reason),
                    );
                }
            }
        }
        if bad == 0 {
            cases.pass(
                "bans",
                format!(
                    "{}: no ban of {} but inside a fault window",
                    node.name, node.ip
                ),
            );
        }
        for (reason, n) in inside {
            cases.record(
                "bans",
                format!("{}: {n} bans inside fault windows: {reason}", node.name),
            );
        }
    }
    let others = bans
        .iter()
        .filter(|b| !manifest.under_test().any(|n| address_matches(n, &b.ip)))
        .count();
    cases.record(
        "bans",
        format!("{others} bans of other addresses (not gated)"),
    );
    Ok(())
}

fn check_invariants(
    run_dir: &Path,
    manifest: &Manifest,
    windows: &[run::Window],
    cases: &mut Cases,
) -> Result<()> {
    match run::read_json(&run_dir.join("final.json")) {
        Err(e) => cases.fail("invariants", "one tip at the pause", e.to_string()),
        Ok(f) => {
            let tips: BTreeMap<String, (u64, String)> = f
                .get("tips")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .map(|(n, t)| {
                            (
                                n.clone(),
                                (
                                    t.get("height").and_then(Value::as_u64).unwrap_or(0),
                                    t.get("id").and_then(Value::as_str).unwrap_or("").to_owned(),
                                ),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let distinct: std::collections::BTreeSet<&(u64, String)> = tips.values().collect();
            let missing: Vec<&String> = manifest
                .nodes
                .iter()
                .map(|n| &n.name)
                .filter(|n| !tips.contains_key(*n))
                .collect();
            match distinct.iter().next() {
                Some((h, id)) if distinct.len() == 1 && missing.is_empty() && !id.is_empty() => {
                    let h32 = u32::try_from(*h).unwrap_or(u32::MAX);
                    if h32 <= manifest.stop_height
                        && (!matches!(manifest.run.as_str(), "t" | "m" | "f")
                            || h32 == manifest.stop_height)
                    {
                        cases.push(
                            "invariants",
                            format!(
                                "one tip at the pause: height {h} on all {} nodes",
                                tips.len()
                            ),
                            h32,
                            Outcome::Pass,
                        );
                    } else {
                        cases.fail(
                            "invariants",
                            "the run stopped within 5 rounds",
                            format!("the tip is {h}, past {}", manifest.stop_height),
                        );
                    }
                }
                _ => cases.fail(
                    "invariants",
                    "one tip at the pause",
                    format!("tips {tips:?}, no tip of {missing:?}"),
                ),
            }
        }
    }
    if !matches!(manifest.run.as_str(), "m" | "f") {
        match run::read_json(&run_dir.join("check-chain.json")) {
            Err(e) => cases.fail(
                "invariants",
                "node 1's chain: the schedule and the supply",
                e.to_string(),
            ),
            Ok(c) => {
                let problems = c
                    .get("problems")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let ok = c
                    .get("ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(problems == 0);
                let scheduled = c
                    .pointer("/stats/scheduleChecked")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if ok && problems == 0 && scheduled > 0 {
                    cases.pass(
                    "invariants",
                    format!(
                        "node 1's chain: {scheduled} blocks forged by their scheduled delegate, the supply check holds"
                    ),
                );
                } else {
                    cases.fail(
                    "invariants",
                    "node 1's chain: the schedule and the supply",
                    format!(
                        "{problems} problems in check-chain.json, {scheduled} blocks checked against the schedule"
                    ),
                );
                }
            }
        }
    }
    let arrivals = steps::arrivals(&run_dir.join("propagation.jsonl"))?;
    let steps = run::steps(run_dir)?;
    for node in manifest.under_test() {
        // A node that joins later is timed from its first block.
        let from = arrivals
            .get(&node.name)
            .and_then(|a| a.values().min().copied())
            .unwrap_or(0);
        let from = steps
            .iter()
            .find(|s| s.name == format!("{}-synced", node.name))
            .and_then(|s| s.start)
            .map_or(from, |t| t.max(from));
        let lags = steps::lags(&arrivals, &node.name, &manifest.control, windows, from);
        // Node 4 of runs `r`, `h` and `m1` is gated: every block within one block time.
        let gated = (node.name == "node4" && manifest.run != "m2")
            || (matches!(manifest.run.as_str(), "t" | "m" | "f") && node.name != manifest.control);
        if lags.is_empty() {
            if gated {
                cases.fail(
                    "invariants",
                    format!("{}: every block within {LAG_LIMIT_SECONDS} s", node.name),
                    "no block arrival was recorded (propagation.jsonl)",
                );
            } else {
                cases.record(
                    "invariants",
                    format!("{}: no block lag measured", node.name),
                );
            }
            continue;
        }
        let mut sorted: Vec<f64> = lags.iter().map(|(_, l)| *l).collect();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let max = sorted[sorted.len() - 1];
        cases.record(
            "invariants",
            format!(
                "{}: block lag against {} over {} blocks: median {median:.1} s, max {max:.1} s",
                node.name,
                manifest.control,
                lags.len()
            ),
        );
        if gated {
            let late: Vec<String> = lags
                .iter()
                .filter(|(_, l)| *l > LAG_LIMIT_SECONDS)
                .map(|(h, l)| format!("{h} ({l:.1} s)"))
                .collect();
            if late.is_empty() {
                cases.pass(
                    "invariants",
                    format!(
                        "{}: every block within {LAG_LIMIT_SECONDS} s, outside its fault windows",
                        node.name
                    ),
                );
            } else {
                cases.fail(
                    "invariants",
                    format!("{}: every block within {LAG_LIMIT_SECONDS} s", node.name),
                    format!(
                        "{} late blocks: {}",
                        late.len(),
                        late.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
                    ),
                );
            }
        }
    }
    for s in &steps {
        match s.name.as_str() {
            "node5-synced" => {
                if let Some(seconds) = s.record.get("seconds") {
                    cases.record(
                        "invariants",
                        format!("node5 synced from an empty store in {seconds} s (after its kill)"),
                    );
                }
            }
            name if name.starts_with("batch-") => {
                if let Some(relays) = s
                    .record
                    .pointer("/detail/relays")
                    .and_then(Value::as_object)
                {
                    let seen: Vec<String> = relays
                        .iter()
                        .map(|(n, r)| {
                            format!(
                                "{n} {}",
                                r.get("seenAllMs").map(Value::to_string).unwrap_or_default()
                            )
                        })
                        .collect();
                    cases.record(
                        "invariants",
                        format!(
                            "{name}: the batch in every pool after (ms): {}",
                            seen.join(", ")
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::live::run::{Node, Step};

    fn manifest(node4: Implementation, dry_run: bool) -> Manifest {
        let node = |name: &str, implementation| Node {
            name: name.into(),
            ip: String::new(),
            implementation,
            forger: false,
            under_test: name == "node4",
        };
        Manifest {
            run: "m2".into(),
            network: "b5-m2".into(),
            dry_run,
            seed: "solar-devnet".into(),
            round_length: 53,
            block_time: 8,
            stop_height: 265,
            control: "node1".into(),
            nodes: vec![
                node("node3", Implementation::Reference),
                node("node4", node4),
            ],
            corpora: BTreeMap::new(),
            pair: None,
            minority: Some((vec!["node3".into(), "node4".into()], "minority".into())),
            api: None,
            window_seconds: 30,
            status: "complete".into(),
            relay_image: None,
            relay_binary_sha256: None,
            worktree: None,
        }
    }

    fn step(name: &str) -> Step {
        Step {
            name: name.into(),
            node: "node3".into(),
            ok: true,
            start: None,
            record: json!({"step": name}),
        }
    }

    #[test]
    fn a_heartwood_minority_rejoins_without_the_restart() {
        let judged = |node4, dry_run, steps: &[Step]| {
            let mut cases = Cases {
                run: "m2".into(),
                results: Vec::new(),
            };
            check_reconnect(&manifest(node4, dry_run), steps, &mut cases);
            cases.results
        };
        let restarted = [step("reconnect-minority"), step("converged")];
        let failed = judged(Implementation::Heartwood, false, &restarted);
        assert!(
            failed.len() == 1 && failed[0].outcome.is_failure(),
            "{failed:?}"
        );
        // A dry run's reference relay keeps node 3 on its side: the restart is recorded.
        let dry = judged(Implementation::Heartwood, true, &restarted);
        assert!(
            dry.len() == 1 && dry[0].label.starts_with("recorded:"),
            "{dry:?}"
        );
        let reference = judged(Implementation::Reference, false, &restarted);
        assert!(!reference[0].outcome.is_failure(), "{reference:?}");
        let by_itself = judged(Implementation::Heartwood, false, &[step("converged")]);
        assert!(
            by_itself.len() == 1 && by_itself[0].label.contains("by itself"),
            "{by_itself:?}"
        );
        // A run without a partition has nothing to record.
        assert!(judged(Implementation::Heartwood, false, &[]).is_empty());
    }

    #[test]
    fn a_mixed_minority_records_node_3_restart_and_fails_a_heartwood_reconnect() {
        let judged = |steps: &[Step]| {
            let mut m = manifest(Implementation::Heartwood, false);
            m.run = "p".into();
            m.nodes.push(Node {
                name: "node5".into(),
                ip: String::new(),
                implementation: Implementation::Reference,
                forger: false,
                under_test: false,
            });
            let mut cases = Cases {
                run: "p".into(),
                results: Vec::new(),
            };
            check_reconnect(&m, steps, &mut cases);
            cases.results
        };
        // A Heartwood relay in the minority listens, as a reference relay does: node 3's restart
        // (a reference relay) is the recipe's recorded workaround, as is node 5's reconnect.
        let restarted = judged(&[
            step("reconnect-node5"),
            step("reconnect-minority"),
            step("converged"),
        ]);
        assert!(
            !restarted.is_empty() && restarted.iter().all(|r| !r.outcome.is_failure()),
            "{restarted:?}"
        );
        assert!(
            restarted.iter().any(|r| r.label.contains("node 3")),
            "{restarted:?}"
        );
        // A Heartwood node the recipe had to reconnect fails the run.
        let touched = judged(&[step("reconnect-node4"), step("converged")]);
        assert!(
            touched
                .iter()
                .any(|r| r.outcome.is_failure() && r.label.contains("node4")),
            "{touched:?}"
        );
    }
}
