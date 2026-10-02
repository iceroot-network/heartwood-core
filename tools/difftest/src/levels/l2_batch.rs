//! L2 over a lockstep batch (`difftest run l2-batch`): every chain of the batch files in a corpus
//! directory judged by the reference and replayed in Heartwood, one report for them all.
//!
//! For each chain: the judge (`corpus judge` with the chain's round restart plan and the
//! processor's lines), `l2` with the restarts (the replay through the store, and both of
//! Heartwood's restart paths at every restart point, each compared with both of the reference's),
//! `l2-processor` in both job cuts, and the revert symmetry check on the reference's lines as well
//! as Heartwood's. Three checks of the batch itself join them: the recipe's restart points are in
//! the restart plan with both reference lines, its reverts are in the revert plan, and a delegate
//! the chain registers that forges within one 400-block download of its registration is named
//! (the judge's one-block jobs cannot show the download). A chain the batch could not make fails
//! the report.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::l2::{self, L2Options};
use super::l2_processor::{self, L2ProcessorOptions};
use crate::cli::{L2BatchArgs, OracleArgs};
use crate::corpus::{self, Corpus, TS_DIGESTS};
use crate::digests;
use crate::errors::{Error, Result};
use crate::lockstep::batch::{self, Batch, BatchChain, DOWNLOAD_BLOCKS};
use crate::lockstep::{Recipe, Step};
use crate::oracle::Oracle;
use crate::report::{CaseResult, Outcome, Report};
use crate::restart::{self, ReferenceMode, RestartOutcome};
use crate::store::ScratchDir;

/// The batch files `args` names in its corpus directory, in name order.
pub fn batch_files(args: &L2BatchArgs) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if args.batches.is_empty() {
        let entries = std::fs::read_dir(&args.dir).map_err(|e| Error::io(&args.dir, e))?;
        for entry in entries {
            let path = entry.map_err(|e| Error::io(&args.dir, e))?.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with("batch-") && name.ends_with(".json") && path.is_file() {
                files.push(path);
            }
        }
        files.sort();
    } else {
        for seed in &args.batches {
            let path = batch::batch_file(&args.dir, seed);
            if !path.is_file() {
                return Err(Error::Usage(format!(
                    "no batch {seed} in {}: {} does not exist",
                    args.dir.display(),
                    path.display()
                )));
            }
            files.push(path);
        }
    }
    if files.is_empty() {
        return Err(Error::Usage(format!(
            "{} holds no batch file: make one with `make lockstep-batch`",
            args.dir.display()
        )));
    }
    Ok(files)
}

fn case(
    file: &str,
    line: usize,
    op: &str,
    label: &str,
    height: u32,
    outcome: Outcome,
) -> CaseResult {
    CaseResult {
        file: file.to_owned(),
        line,
        op: op.to_owned(),
        label: label.to_owned(),
        height: u64::from(height),
        outcome,
    }
}

fn failed(file: &str, op: &str, label: &str, message: impl Into<String>) -> CaseResult {
    case(file, 0, op, label, 0, Outcome::HarnessError(message.into()))
}

/// The restart steps of `recipe`.
fn restart_heights(recipe: &Recipe) -> Vec<u32> {
    recipe
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Restart { height } => Some(*height),
            _ => None,
        })
        .collect()
}

/// The restart plan holds each of the recipe's restart points, and the judge wrote both of the
/// reference's lines at each, which started.
fn restart_points(chain: &BatchChain, corpus: &Corpus, dir: &Path) -> Vec<CaseResult> {
    let file = format!("{}/restart-plan.json", chain.drawn.name);
    let label = "the recipe's restart points hold both of the reference's lines";
    let Some(plan) = &corpus.restart_plan else {
        return vec![failed(
            &file,
            "restart",
            label,
            "the corpus has no restart plan",
        )];
    };
    let records = match restart::read_restarts(&dir.join(restart::TS_RESTARTS)) {
        Ok(records) => records,
        Err(e) => return vec![failed(&file, "restart", label, e.to_string())],
    };
    let mut out = Vec::new();
    for (i, height) in restart_heights(&chain.drawn.recipe).into_iter().enumerate() {
        let mut problems = Vec::new();
        if !plan.restarts_at(height) {
            problems.push("the plan does not restart there".to_owned());
        }
        for mode in ReferenceMode::ALL {
            match records
                .iter()
                .find(|r| r.mode == mode && r.height == height)
            {
                Some(r) => {
                    if let RestartOutcome::Refused(reason) = &r.outcome {
                        problems.push(format!(
                            "the reference's {} start was refused: {reason}",
                            mode.name()
                        ));
                    }
                }
                None => problems.push(format!("no {} line of the reference", mode.name())),
            }
        }
        let outcome = if problems.is_empty() {
            Outcome::Pass
        } else {
            Outcome::HarnessError(problems.join("; "))
        };
        out.push(case(&file, i + 1, "restart", label, height, outcome));
    }
    out
}

/// The recipe's reverts are steps of the corpus's revert plan, and the reference's lines revert
/// as they apply.
fn reverts(chain: &BatchChain, corpus: &Corpus) -> Vec<CaseResult> {
    let file = format!("{}/revert-plan.json", chain.drawn.name);
    let mut out = Vec::new();
    let steps: Vec<(u32, u32)> = chain
        .drawn
        .recipe
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Revert {
                after_height,
                blocks,
            } => Some((*after_height, *blocks)),
            _ => None,
        })
        .collect();
    for (i, (after, blocks)) in steps.into_iter().enumerate() {
        let held = corpus
            .revert_plan
            .steps
            .iter()
            .any(|s| s.after_height == after && s.blocks == blocks);
        let outcome = if held {
            Outcome::Pass
        } else {
            Outcome::HarnessError("the revert plan does not hold the recipe's revert".into())
        };
        out.push(case(
            &file,
            i + 1,
            "revert",
            &format!("the recipe's revert of {blocks} blocks is in the plan"),
            after,
            outcome,
        ));
    }
    let lines = digests::read(&corpus.dir.join(TS_DIGESTS));
    let outcome = match lines.map(|l| digests::revert_asymmetries(&l)) {
        Ok(problems) if problems.is_empty() => Outcome::Pass,
        Ok(problems) => Outcome::HarnessError(problems.join("; ")),
        Err(e) => Outcome::HarnessError(e.to_string()),
    };
    out.push(case(
        &format!("{}/{TS_DIGESTS}", chain.drawn.name),
        0,
        "revert",
        "the reference's revert lines equal the forward lines of their heights",
        0,
        outcome,
    ));
    out
}

/// The delegates the chain registers that forge within one download of their registration.
fn registered_forgers(chain: &BatchChain, dir: &Path) -> Vec<CaseResult> {
    let file = format!("{}/reference/meta.jsonl", chain.drawn.name);
    match batch::registered_forgers(chain, dir) {
        Ok(found) => found
            .into_iter()
            .map(|f| {
                case(
                    &file,
                    0,
                    "download",
                    &format!(
                        "{} registered at {} forges {} blocks within one {DOWNLOAD_BLOCKS}-block \
                         download, the first at {}",
                        f.username, f.registered_at, f.blocks, f.first_block
                    ),
                    f.first_block,
                    Outcome::Documented(
                        "a delegate registered in the chain forges within one download of its \
                         registration, which the judge's one-block jobs do not show"
                            .to_owned(),
                    ),
                )
            })
            .collect(),
        Err(e) => vec![failed(
            &file,
            "download",
            "the registered delegates' blocks",
            e.to_string(),
        )],
    }
}

/// Judge and replay the chain `chain`, made in `dir`.
fn judge_chain(chain: &BatchChain, dir: &Path, oracle: &Oracle) -> Result<Vec<CaseResult>> {
    let name = &chain.drawn.name;
    let chain_dir = dir.join(name);
    let mut results = Vec::new();
    if let Some(stop) = &chain.stopped {
        results.push(failed(
            name,
            "lockstep",
            "the chain was made",
            format!("the producer stopped: {stop}"),
        ));
        return Ok(results);
    }
    if chain.tip.is_none() {
        results.push(failed(
            name,
            "lockstep",
            "the chain was made",
            "the batch file records no tip",
        ));
        return Ok(results);
    }
    corpus::verify(&chain_dir)?;
    // The judge: the reference's lines, its restarts at the chain's plan, the processor's results.
    let work = ScratchDir::new("l2-batch-judge")?;
    // The judge's exit status holds the chain to every anchor, which includes the two a corpus of
    // block files has no data for; the recorded verdict reads the report as the other gate
    // targets do.
    oracle.judge(&chain_dir, work.path(), &["--processor".to_owned()])?;
    let anchors_hold = corpus::record_judge(&chain_dir, work.path())?;
    let loaded = Corpus::load(&chain_dir)?;
    results.push(case(
        &format!("{name}/{}", corpus::ANCHORS),
        0,
        "judge",
        "the judge's anchors hold",
        0,
        if anchors_hold {
            Outcome::Pass
        } else {
            Outcome::HarnessError("an anchor of the judge failed: see anchors.json".into())
        },
    ));
    // `l2` and `l2-restart`: one replay through the store, then both restart paths.
    results.extend(
        l2::run(
            &loaded,
            &L2Options {
                synthetic: false,
                restart: true,
                store: true,
                to: None,
            },
            None,
        )?
        .results,
    );
    // `l2-processor`, in the reference's job cut and with one block per job.
    results.extend(
        l2_processor::run(
            &loaded,
            &L2ProcessorOptions {
                chunks: None,
                to: None,
                synthetic: false,
            },
            None,
        )?
        .results,
    );
    results.extend(restart_points(chain, &loaded, &chain_dir));
    results.extend(reverts(chain, &loaded));
    results.extend(registered_forgers(chain, dir));
    Ok(results)
}

/// Judge and replay every chain of the batch file `batch`, whose chains are in `dir`.
pub fn run_batch(batch: &Path, dir: &Path, oracle: &OracleArgs) -> Result<Report> {
    let made: Batch = batch::read_batch(batch)?;
    let oracle = oracle.resolve(|name| std::env::var(name).ok())?;
    let source = format!(
        "{} chains of batch {} in {}",
        made.chains.len(),
        made.seed,
        dir.display()
    );
    let jobs = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .clamp(1, 8)
        .min(made.chains.len().max(1));
    let next = AtomicUsize::new(0);
    let done: Mutex<BTreeMap<usize, Vec<CaseResult>>> = Mutex::new(BTreeMap::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(chain) = made.chains.get(at) else {
                        break;
                    };
                    let results = judge_chain(chain, dir, &oracle).unwrap_or_else(|e| {
                        vec![failed(
                            &chain.drawn.name,
                            "l2-batch",
                            "the chain was judged",
                            e.to_string(),
                        )]
                    });
                    let bad = results.iter().filter(|r| r.outcome.is_failure()).count();
                    eprintln!(
                        "l2-batch: {}: {} cases, {bad} failed",
                        chain.drawn.name,
                        results.len()
                    );
                    if let Ok(mut done) = done.lock() {
                        done.insert(at, results);
                    }
                }
            });
        }
    });
    let results: Vec<CaseResult> = done
        .into_inner()
        .map_err(|_| Error::Usage("an l2-batch worker stopped".into()))?
        .into_values()
        .flatten()
        .collect();
    Ok(Report {
        title: format!("L2 over lockstep batch {}", made.seed),
        source,
        files: made.chains.len(),
        results,
    })
}

/// `difftest run l2-batch`: every chain of the batches `args` names, in one report.
pub fn run(args: &L2BatchArgs) -> Result<Report> {
    let mut merged: Option<Report> = None;
    for file in batch_files(args)? {
        let report = run_batch(&file, &args.dir, &args.oracle)?;
        merged = Some(match merged {
            None => report,
            Some(mut all) => {
                all.title = "L2 over lockstep batches".to_owned();
                all.source = format!("{}; {}", all.source, report.source);
                all.files += report.files;
                all.results.extend(report.results);
                all
            }
        });
    }
    merged.ok_or_else(|| Error::Usage("no batch to judge".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(dir: &Path, batches: &[&str]) -> L2BatchArgs {
        L2BatchArgs {
            oracle: OracleArgs::default(),
            dir: dir.to_path_buf(),
            batches: batches.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn the_batch_files_are_the_named_ones_or_every_one_in_name_order() {
        let scratch = ScratchDir::new("l2-batch-unit").unwrap();
        let dir = scratch.path();
        assert!(
            matches!(batch_files(&args(dir, &[])), Err(Error::Usage(m)) if m.contains("no batch file"))
        );
        for seed in ["b", "a", "c"] {
            std::fs::write(batch::batch_file(dir, seed), "{}").unwrap();
        }
        std::fs::write(dir.join("notes.json"), "{}").unwrap();
        std::fs::create_dir(dir.join("batch-x.json.d")).unwrap();
        let names = |files: Vec<PathBuf>| -> Vec<String> {
            files
                .iter()
                .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            names(batch_files(&args(dir, &[])).unwrap()),
            ["batch-a.json", "batch-b.json", "batch-c.json"]
        );
        assert_eq!(
            names(batch_files(&args(dir, &["c", "a"])).unwrap()),
            ["batch-c.json", "batch-a.json"]
        );
        assert!(matches!(
            batch_files(&args(dir, &["z"])),
            Err(Error::Usage(m)) if m.contains("no batch z")
        ));
    }

    #[test]
    fn a_chain_the_batch_could_not_make_fails_the_report_without_the_oracle() {
        let scratch = ScratchDir::new("l2-batch-stopped").unwrap();
        let dir = scratch.path();
        let drawn = batch::draw("stop", 1, 1, false).unwrap();
        let chain = BatchChain {
            drawn,
            tip: None,
            transactions: 0,
            missed: 0,
            stopped: Some("the pools do not agree".into()),
        };
        let file = batch::batch_file(dir, "stop");
        let made = Batch {
            format: batch::BATCH_FORMAT.to_owned(),
            seed: "stop".into(),
            profile: "devnet".into(),
            chains: vec![chain],
        };
        std::fs::write(&file, serde_json::to_string(&made).unwrap()).unwrap();
        let oracle = OracleArgs {
            solar_dir: Some(dir.join("no-reference")),
            ..OracleArgs::default()
        };
        let report = run_batch(&file, dir, &oracle).unwrap();
        assert_eq!(report.failed(), 1);
        assert!(!report.passes(true));
        let text = report.to_string();
        assert!(text.contains("batch-stop-1"), "{text}");
    }
}
