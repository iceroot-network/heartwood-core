//! `difftest live check`: the comparisons of a live run's directory (the devnet tooling writes it):
//! node 1's archive as a corpus judged by the oracle, each Heartwood node's digest lines against
//! the judge's (a torn last line dropped), the store anchors, the step reports, the recorded API
//! answers, the ban scan with its fault windows, and the invariants.
//!
//! The run directory's layout is [`run`]'s. The checks, in the report's order:
//!
//! 1. `run`: the run finished, and every scripted step of its schedule ran and met its expectation
//!    ([`steps::required`]); txgen's own checks passed; for run `h`, each submission got the answer it
//!    got in run `r` and the scenario wallets at the pause equal run `r`'s (the run beside it, `../r`);
//! 2. `judge`: every chain of the run (node 1's archive, and the minority's archive from before a
//!    heal) imported under the corpus root (`target/corpus` of the current directory, as the
//!    Makefile's `CORPUS`) and judged (the oracle of `DIFFTEST_IMAGE` or `SOLAR_DIR` runs only when
//!    a corpus has no judge's lines yet); the judge's anchors hold;
//! 3. `nodes`: each node under test: a Heartwood relay's digest lines, store and logs
//!    ([`nodes`]); a reference relay's archive against node 1's;
//! 4. `api`: the route corpus's answers, judged by `api compare` ([`crate::api`]), and the rate
//!    limit's answers from an address that no node whitelists;
//! 5. `bans`: every ban of a node under test's address ([`bans`]);
//! 6. `invariants`: one tip at the pause, at or below the run's stop height; node 1's chain checked
//!    by the devnet tooling (every generator the scheduled one, the supply); every block reached
//!    the nodes under test within one block time (node 4 of runs `r`, `h` and `m1`), outside their fault
//!    windows;
//! 7. recorded, not gated (outcome `Pass`, label `recorded: ...`): block lags, the joining node's
//!    sync time, the transaction propagation, socket-close bans inside fault windows, each node's
//!    refused and undone blocks.
//!
//! Besides the report, the check writes every case with its outcome to `live-check.txt` in the run
//! directory.

pub mod bans;
mod corpora;
pub mod digest;
mod nodes;
pub mod run;
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
use run::{Implementation, Manifest};

/// Where the run's corpora go, relative to the current directory: the Makefile's `CORPUS`.
pub const CORPUS_ROOT: &str = "target/corpus";
/// The time a block may take to reach a node under test: one block time.
pub const LAG_LIMIT_SECONDS: f64 = 8.0;

/// What a live check reads besides the run directory.
pub struct LiveOptions<'a> {
    /// Where the run's corpora are imported and judged.
    pub corpus_root: PathBuf,
    /// The oracle, asked for only when a corpus has not been judged yet.
    pub oracle: &'a dyn Fn() -> Result<Oracle>,
}

/// Check the live run in `run_dir`, with the corpora under [`CORPUS_ROOT`] and the oracle of the
/// environment (`DIFFTEST_IMAGE` or `SOLAR_DIR`).
pub fn check(run_dir: &Path) -> Result<Report> {
    let oracle = || OracleArgs::default().resolve(|name: &str| std::env::var(name).ok());
    check_with(
        run_dir,
        &LiveOptions {
            corpus_root: PathBuf::from(CORPUS_ROOT),
            oracle: &oracle,
        },
    )
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
    let manifest = Manifest::load(run_dir)?;
    let mut cases = Cases {
        run: manifest.run.clone(),
        results: Vec::new(),
    };
    let windows = run::windows(run_dir, manifest.window_seconds)?;
    // The route corpus first: whether its judge is built in this tree decides how a failed
    // recording of it counts.
    let mut api_cases = Cases {
        run: manifest.run.clone(),
        results: Vec::new(),
    };
    let api_built = check_api(run_dir, &manifest, &mut api_cases);
    check_run(run_dir, &manifest, api_built, &mut cases)?;
    let chains = check_judge(run_dir, &manifest, options, &mut cases);
    check_nodes(run_dir, &manifest, &chains, &mut cases);
    cases.results.extend(api_cases.results);
    check_bans(run_dir, &manifest, &windows, &mut cases)?;
    check_invariants(run_dir, &manifest, &windows, &mut cases)?;
    let report = Report {
        title: format!("Live check of run {} ({})", manifest.run, manifest.network),
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
    let txgen = run_dir.join("txgen");
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
            match steps::txgen(&dir)? {
                Ok(text) => cases.pass("run", format!("txgen {name}: {text}")),
                Err(text) => cases.fail("run", format!("txgen {name}"), text),
            }
        }
    }
    if let Some(pair) = &manifest.pair {
        let other = run_dir.parent().map(|p| p.join(pair));
        match other.filter(|d| d.join("run.json").exists()) {
            None => cases.push(
                "run",
                format!("the answers and wallets of run {pair}"),
                0,
                Outcome::Skipped(format!("no run {pair} beside this one")),
            ),
            Some(other) => {
                let ours = steps::submissions(&run_dir.join("txgen/once"))?;
                let theirs = steps::submissions(&other.join("txgen/once"))?;
                let d = steps::compare_submissions(&ours, &theirs);
                if d.is_empty() && !ours.is_empty() {
                    cases.pass(
                        "run",
                        format!("each of {} submissions got run {pair}'s answer", ours.len()),
                    );
                } else if ours.is_empty() {
                    cases.fail(
                        "run",
                        format!("the submissions against run {pair}"),
                        "no submission recorded",
                    );
                }
                for p in d.iter().take(20) {
                    cases.fail(
                        "run",
                        format!("the submissions against run {pair}"),
                        p.clone(),
                    );
                }
                let w = (
                    run::read_json(&run_dir.join("wallets.json")),
                    run::read_json(&other.join("wallets.json")),
                );
                match w {
                    (Ok(a), Ok(b)) => {
                        let d = steps::compare_wallets(&a, &b);
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
                let how = match (judged.imported, judged.judged) {
                    (true, true) => "imported and judged now",
                    (false, true) => "judged now",
                    (true, false) | (false, false) => "judged before",
                };
                if judged.anchors_ok {
                    cases.pass(
                        "judge",
                        format!(
                            "{name}: tip {}, the judge's anchors hold ({how}, {})",
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
                out.insert(name.clone(), judged);
            }
        }
    }
    out
}

/// The final chain's corpus: the one that is not the minority's.
fn final_chain<'c>(manifest: &Manifest, chains: &'c Chains) -> Option<&'c corpora::Judged> {
    let minority = manifest.minority.as_ref().map(|(_, c)| c.as_str());
    chains
        .iter()
        .find(|(name, _)| Some(name.as_str()) != minority)
        .map(|(_, j)| j)
}

fn check_nodes(run_dir: &Path, manifest: &Manifest, chains: &Chains, cases: &mut Cases) {
    let Some(last) = final_chain(manifest, chains) else {
        cases.fail(
            "nodes",
            "the nodes under test",
            "the final chain was not judged, so no node can be checked",
        );
        return;
    };
    let references: Vec<&str> = manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Reference)
        .map(|n| n.ip.as_str())
        .collect();
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
                let mut c = nodes::heartwood_digest(run_dir, node, &judged);
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
    match steps::rate_limit(&run_dir.join("api/rate-limit/answers.jsonl")) {
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
    let bans: Vec<bans::Ban> = run::read_lines(&path)?
        .iter()
        .filter_map(bans::Ban::from_json)
        .collect();
    for node in manifest.under_test() {
        let mut inside: BTreeMap<String, usize> = BTreeMap::new();
        let mut bad = 0usize;
        for ban in bans.iter().filter(|b| b.ip == node.ip) {
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
                            ban.at.map(|t| t.to_string()).unwrap_or_default(),
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
        .filter(|b| !manifest.under_test().any(|n| n.ip == b.ip))
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
                    if h32 <= manifest.stop_height {
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
        if lags.is_empty() {
            cases.record(
                "invariants",
                format!("{}: no block lag measured", node.name),
            );
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
        let gated = node.name == "node4" && manifest.run != "m2";
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
