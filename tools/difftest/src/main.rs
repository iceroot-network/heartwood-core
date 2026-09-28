//! The `difftest` binary. `difftest help` lists the commands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use heartwood_difftest::cli::{
    self, Command, ForgeArgs, GenesisArgs, JudgeArgs, L1Args, L2Args, L2ProcessorArgs, USAGE,
};
use heartwood_difftest::corpus::{self, Corpus};
use heartwood_difftest::genesis::{self, Built};
use heartwood_difftest::levels::l1::{self, L1Options, Property};
use heartwood_difftest::levels::l2::{self, L2Options};
use heartwood_difftest::levels::l2_forge::{self, ForgeOptions};
use heartwood_difftest::levels::l2_processor::{self, L2ProcessorOptions};
use heartwood_difftest::levels::{Level, l0};
use heartwood_difftest::oracle::GenOptions;
use heartwood_difftest::report::{CaseResult, Outcome, Report, describe_meta};
use heartwood_difftest::restart::RestartPlan;
use heartwood_difftest::store;
use heartwood_difftest::strategies::transactions::TxContext;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;
use heartwood_difftest::{
    Error, Result, api, corpus_from_blocks, forger_kill, live, network_generate, replay,
    runtime_kill, subject,
};

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let strict = cli::take_strict(&mut args);
    let outcome = cli::parse(&args).and_then(|command| execute(command, strict));
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(Error::Usage(message)) => {
            eprintln!("difftest: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::NotYet(message)) => {
            eprintln!("difftest: {message}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("difftest: {error}");
            ExitCode::from(1)
        }
    }
}

/// The verdict of a comparing command's `report`: strict, a case that was not compared fails it
/// too, and the report says so.
fn verdict(report: &Report, strict: bool) -> bool {
    if let Some(note) = report.strict_note(strict) {
        println!("{note}");
    }
    report.passes(strict)
}

/// Run a command; `Ok(false)` means it ran and found differences (or, `strict`, left cases
/// uncompared).
fn execute(command: Command, strict: bool) -> Result<bool> {
    let env = |name: &str| std::env::var(name).ok();
    match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(true)
        }
        Command::Levels => {
            for level in Level::ALL {
                let state = if level.is_available() {
                    "runs"
                } else {
                    "not yet"
                };
                println!(
                    "{level}  {:<45} {:<8} arrives with {}",
                    level.title(),
                    state,
                    level.arrives_with()
                );
            }
            Ok(true)
        }
        Command::OracleGen {
            oracle,
            out,
            backend_check,
            devnet_generator,
        } => {
            let oracle = oracle.resolve(env)?;
            let devnet_generator = devnet_generator.or_else(|| {
                env("DIFFTEST_DEVNET_GENERATOR")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            });
            let options = GenOptions {
                backend_check,
                devnet_generator,
            };
            oracle.gen_vectors(&out, &options)?;
            let set = VectorSet::load(&out)?;
            println!(
                "difftest: {} records in {} files at {}, manifest verified",
                set.record_count(),
                set.files.len(),
                out.display()
            );
            Ok(true)
        }
        Command::OraclePing { oracle } => {
            let oracle = oracle.resolve(env)?;
            let mut server = oracle.server()?;
            let pong = server.ping()?;
            println!("difftest: oracle ({:?}) answered: {pong}", oracle.mode);
            Ok(true)
        }
        Command::VectorsVerify { dir } => {
            let set = VectorSet::load(&dir)?;
            println!(
                "difftest: {}: manifest verified, {} records in {} files",
                dir.display(),
                set.record_count(),
                set.files.len()
            );
            if let Some(meta) = set.meta() {
                println!("  oracle: {}", describe_meta(meta));
            }
            for file in &set.files {
                println!(
                    "  {:<28} {:>5} records  {}",
                    file.file_name(),
                    file.records.len(),
                    file.meta.class
                );
            }
            Ok(true)
        }
        Command::VectorsDiff { a, b } => {
            let (sa, sb) = (VectorSet::load(&a)?, VectorSet::load(&b)?);
            let d = l0::diff_runs(&sa, &sb);
            println!("difftest: {} vs {}", a.display(), b.display());
            for m in &d.meta {
                println!("  meta differs (informational): {m}");
            }
            for f in &d.missing_files {
                println!("  MISSING {f}");
            }
            for (record, differences) in &d.records {
                println!("  DIFF {record}");
                for difference in differences.iter().take(10) {
                    println!("      {}", difference.to_string().replace('\n', "\n      "));
                }
            }
            if d.is_identical() {
                println!("  {} records identical", d.compared);
            } else {
                println!(
                    "  {} of {} records differ, {} files missing",
                    d.records.len(),
                    d.compared,
                    d.missing_files.len()
                );
            }
            Ok(d.is_identical())
        }
        Command::Run {
            level: Level::L0,
            dir: Some(dir),
        } => {
            let set = VectorSet::load(&dir)?;
            let devnet = Devnet::from_vectors(&set)?;
            if devnet.is_none() {
                eprintln!(
                    "difftest: {} has no genesis class, so the devnet rules are unknown and the \
                     cases that need them are skipped",
                    dir.display()
                );
            }
            let report = l0::run(&set, &subject::heartwood(devnet));
            print!("{report}");
            Ok(verdict(&report, strict))
        }
        Command::Genesis(args) => write_genesis(&args),
        Command::OracleReplay(args) => {
            let set = VectorSet::load(&args.vectors)?;
            let devnet = Devnet::from_vectors(&set)?
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "{} has no genesis class, so the devnet rules are unknown",
                        args.vectors.display()
                    ))
                })?
                .at_current_time()?;
            let milestones = match &args.milestones {
                Some(path) => Some(std::fs::read_to_string(path).map_err(|e| Error::Io {
                    path: path.clone(),
                    source: e,
                })?),
                None => None,
            };
            let oracle = args.oracle.resolve(env)?;
            let mut server = oracle.server()?;
            let replayed = replay::run(
                &mut server,
                &subject::heartwood(Some(devnet)),
                &replay::Replay {
                    op: &args.op,
                    corpus: &args.corpus,
                    height: args.height,
                    milestones: milestones.as_deref(),
                },
            )?;
            print!("{}", replayed.report);
            println!(
                "  the reference accepted {} of the {} inputs",
                replayed.accepted,
                replayed.report.results.len()
            );
            Ok(verdict(&replayed.report, strict))
        }
        Command::RunL1(args) => run_l1(&args, env, strict),
        Command::RunL2(args) => run_l2(&args, env, strict),
        Command::CorpusImport {
            archives,
            out,
            seed,
        } => {
            corpus::import(&archives, &out, &seed)?;
            let loaded = Corpus::load(&out)?;
            println!(
                "difftest: corpus {} written: blocks 2 to {}, {} revert steps, manifest verified",
                out.display(),
                loaded.tip(),
                loaded.revert_plan.steps.len()
            );
            Ok(true)
        }
        Command::CorpusFromBlocks {
            blocks,
            crypto,
            out,
            seed,
        } => {
            corpus_from_blocks::from_blocks(&blocks, &crypto, &seed, &out)?;
            let loaded = Corpus::load(&out)?;
            println!(
                "difftest: corpus {} written: blocks 2 to {}, manifest verified",
                out.display(),
                loaded.tip()
            );
            Ok(true)
        }
        Command::CorpusVerify { dir } => {
            let loaded = Corpus::load(&dir)?;
            println!(
                "difftest: {}: manifest verified, blocks 2 to {}, {} revert steps",
                dir.display(),
                loaded.tip(),
                loaded.revert_plan.steps.len()
            );
            Ok(true)
        }
        Command::CorpusJudge(args) => judge(&args, env),
        Command::ForgeReplay(args) => forge_replay(&args, env),
        Command::ForgerCrash { kills, seed, child } => {
            let options = forger_kill::KillOptions { kills, seed, child };
            let report = built(store::guarded(|| forger_kill::run(&options)))?;
            print!("{report}");
            Ok(report.is_success())
        }
        Command::NetworkGenerate { vectors } => {
            let set = VectorSet::load(&vectors)?;
            let report = built(store::guarded(|| network_generate::check(&set)))?;
            print!("{report}");
            Ok(report.is_success())
        }
        Command::StoreReplay { dir, db, to } => {
            let loaded = Corpus::load(&dir)?;
            let tip = store::replay(&loaded, &db, to)?;
            println!(
                "difftest: the store under {} is at height {tip}",
                db.display()
            );
            Ok(true)
        }
        Command::StoreCheck { dir, db } => {
            let loaded = Corpus::load(&dir)?;
            let checked = store::check(&loaded, &db)?;
            let mut ok = true;
            for (mode, outcome) in &checked.modes {
                match outcome {
                    Ok((tip, line)) => println!("{} {tip} {line}", mode.name()),
                    Err(message) => {
                        ok = false;
                        println!("{} refused: {message}", mode.name());
                    }
                }
            }
            Ok(ok)
        }
        Command::StoreCrash {
            dir,
            kills,
            seed,
            max_delay_ms,
            runtime,
        } => {
            let loaded = Corpus::load(&dir)?;
            let exe = std::env::current_exe()
                .map_err(|e| Error::Usage(format!("cannot find this program: {e}")))?;
            let max_delay = std::time::Duration::from_millis(max_delay_ms);
            let report = if runtime {
                runtime_kill::crash(&loaded, &exe, kills, seed, max_delay)?
            } else {
                store::crash(&loaded, &exe, kills, seed, max_delay)?
            };
            print!("{report}");
            Ok(verdict(&report, strict))
        }
        Command::RunL2Processor(args) => run_l2_processor(&args, env, strict),
        Command::LiveCheck { dir } => {
            let report = live::check(&dir)?;
            print!("{report}");
            Ok(verdict(&report, strict))
        }
        Command::ApiCompare(args) => {
            let mut report = api::compare(&args)?;
            // Recorded answers of a Heartwood relay count only for the tree whose relay answered.
            if let api::ApiCompareArgs::Recorded { run } = &args
                && let Ok(manifest) = live::run::Manifest::load(run)
                && let Some(binding) =
                    live::run::relay_binding(&manifest, Path::new(live::run::RELAY_BINARY))
            {
                report.results.insert(
                    0,
                    CaseResult {
                        file: run.join("run.json").display().to_string(),
                        line: 0,
                        op: "relay".into(),
                        label: "the relay that answered is this tree's".into(),
                        height: 0,
                        outcome: match binding {
                            Ok(_) => Outcome::Pass,
                            Err(why) => Outcome::HarnessError(why),
                        },
                    },
                );
            }
            print!("{report}");
            Ok(verdict(&report, strict))
        }
        Command::Run { level, .. } => Err(Error::NotYet(format!(
            "{} is not built yet; it arrives with {}",
            level.title(),
            level.arrives_with()
        ))),
    }
}

/// The committed golden vectors, which hold the devnet configuration.
fn default_vectors() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors")
}

fn run_l1(args: &L1Args, env: impl Fn(&str) -> Option<String>, strict: bool) -> Result<bool> {
    let vectors = args.vectors.clone().unwrap_or_else(default_vectors);
    let set = VectorSet::load(&vectors)?;
    let devnet = Devnet::from_vectors(&set)?.ok_or_else(|| {
        Error::Usage(format!(
            "{} has no genesis class, so the devnet rules are unknown",
            vectors.display()
        ))
    })?;
    let context = TxContext::new(devnet.network.clone(), l1::devnet_milestones(&set)?)?;
    let registry = subject::heartwood(Some(devnet));
    let oracle = args.oracle.resolve(env)?;
    let mut server = oracle.server()?;
    let properties = if args.properties.is_empty() {
        Property::ALL.to_vec()
    } else {
        args.properties.clone()
    };
    let run = l1::run(
        &mut server,
        &registry,
        &Arc::new(context),
        &L1Options {
            properties,
            cases: args.cases,
            seed: args.seed,
            regressions: args.regressions.clone(),
            min_reach: args.min_reach,
        },
    )?;
    print!("{}", run.report);
    if !run.classes.is_empty() {
        println!("  outcomes (the reference's):");
        for (class, n) in &run.classes {
            println!("    {n:>7}  {class}");
        }
    }
    Ok(verdict(&run.report, strict))
}

/// `difftest corpus judge`: write the restart plan when asked, run the judge, record its output.
fn judge(args: &JudgeArgs, env: impl Fn(&str) -> Option<String>) -> Result<bool> {
    let dir = &args.dir;
    corpus::verify(dir)?;
    if let Some(kind) = args.restarts {
        let loaded = Corpus::load(dir)?;
        let tip = args.to.unwrap_or_else(|| loaded.tip());
        let tip_id = loaded
            .block_id(tip)
            .ok_or_else(|| Error::Usage(format!("the chain has no block {tip}")))?;
        let mut live: Vec<u32> = loaded
            .live_restarts
            .iter()
            .map(|l| l.height)
            .filter(|h| *h <= tip)
            .collect();
        live.sort_unstable();
        live.dedup();
        let plan = RestartPlan::generate(
            kind,
            RestartPlan::seed_of(&tip_id),
            tip,
            loaded.revert_plan.round_length,
            &live,
        );
        corpus::write_restart_plan(dir, &plan)?;
        println!(
            "difftest: restart plan {kind} for tip {tip}: {} restarts",
            plan.heights.len()
        );
    } else if let Some(file) = &args.restart_plan {
        let text = std::fs::read_to_string(file).map_err(|e| Error::Io {
            path: file.clone(),
            source: e,
        })?;
        let value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
            path: file.clone(),
            message: e.to_string(),
        })?;
        corpus::write_restart_plan(dir, &RestartPlan::from_json(&value, file)?)?;
    }
    let oracle = args.oracle.resolve(env)?;
    let work = std::env::temp_dir().join(format!("heartwood-judge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let mut extra = Vec::new();
    if !args.dump.is_empty() {
        let list: Vec<String> = args.dump.iter().map(u32::to_string).collect();
        extra.extend(["--dump".to_string(), list.join(",")]);
    }
    if let Some(to) = args.to {
        extra.extend(["--to".to_string(), to.to_string()]);
    }
    if args.processor {
        extra.push("--processor".to_string());
    }
    let ok = oracle.judge(dir, &work, &extra)?;
    let recorded = corpus::record_judge(dir, &work)?;
    println!(
        "difftest: the judge's lines and anchors are in {} (anchors {})",
        dir.display(),
        if recorded && ok {
            "hold"
        } else {
            "FAILED: see anchors.json"
        }
    );
    if args.dump.is_empty() {
        let _ = std::fs::remove_dir_all(&work);
    } else {
        println!("  dumps: {}", work.display());
    }
    Ok(ok && recorded)
}

fn run_l2(args: &L2Args, env: impl Fn(&str) -> Option<String>, strict: bool) -> Result<bool> {
    let corpus = Corpus::load(&args.dir)?;
    let mut server = match &args.oracle {
        Some(oracle) => Some(oracle.resolve(env)?.server()?),
        None => None,
    };
    let report = l2::run(
        &corpus,
        &L2Options {
            synthetic: args.synthetic,
            restart: args.restart,
            store: !args.memory,
            to: args.to,
        },
        server.as_mut(),
    )?;
    print!("{report}");
    if args.synthetic {
        println!("  synthetic steps:");
        for r in report
            .results
            .iter()
            .filter(|r| r.file.ends_with("/synthetic"))
        {
            let verdict = match &r.outcome {
                o if o.is_failure() => "FAIL",
                heartwood_difftest::report::Outcome::Skipped(_) => "skip",
                heartwood_difftest::report::Outcome::Documented(_) => "doc",
                _ => "ok",
            };
            println!("    {verdict:<4} {}", r.label);
        }
    }
    Ok(verdict(&report, strict))
}

fn run_l2_processor(
    args: &L2ProcessorArgs,
    env: impl Fn(&str) -> Option<String>,
    strict: bool,
) -> Result<bool> {
    let corpus = Corpus::load(&args.dir)?;
    let mut server = match &args.oracle {
        Some(oracle) => Some(oracle.resolve(env)?.server()?),
        None => None,
    };
    let report = l2_processor::run(
        &corpus,
        &L2ProcessorOptions {
            chunks: args.chunks,
            to: args.to,
            synthetic: args.synthetic,
        },
        server.as_mut(),
    )?;
    print!("{report}");
    Ok(verdict(&report, strict))
}

/// The result of a part that may still be a stub: its `todo!()` is "not built yet".
fn built<T>(outcome: std::result::Result<Result<T>, store::Unbuilt>) -> Result<T> {
    outcome.map_err(|store::Unbuilt(message)| Error::NotYet(format!("not built yet: {message}")))?
}

/// The forge replay of a corpus, in the direction the arguments give.
fn forge_replay(args: &ForgeArgs, env: impl Fn(&str) -> Option<String>) -> Result<bool> {
    let corpus = Corpus::load(&args.dir)?;
    let mut server = match &args.oracle {
        Some(oracle) => Some(oracle.resolve(env)?.server()?),
        None => None,
    };
    let options = ForgeOptions {
        direction: args.direction,
        to: args.to,
    };
    let report = built(store::guarded(|| {
        l2_forge::run(&corpus, &options, server.as_mut())
    }))?;
    print!("{report}");
    Ok(report.is_success())
}

/// Write a chain definition, and compare it with the devnet generator's output when asked.
fn write_genesis(args: &GenesisArgs) -> Result<bool> {
    let built = Built::new(&args.profile, &args.seed, &args.epoch)?;
    built.write(&args.out, args.force)?;
    let summary = &built.definition.summary;
    println!(
        "difftest: chain definition of profile {} written to {}",
        built.profile.name,
        args.out.display()
    );
    println!("  epoch       {}", built.input.epoch);
    println!(
        "  nethash     {}",
        heartwood_crypto::utils::hex::encode(&summary.nethash)
    );
    println!(
        "  genesis id  {}  ({} transactions)",
        summary.genesis_id, summary.transactions
    );
    println!("  issued      {}", summary.issued);
    let Some(dir) = &args.compare else {
        return Ok(true);
    };
    let differences = genesis::compare(&built, dir)?;
    if differences.is_empty() {
        println!(
            "  compared with {}: the crypto files, the keys and the summary are identical",
            dir.display()
        );
    } else {
        println!(
            "  compared with {}: {} differences",
            dir.display(),
            differences.len()
        );
        for difference in &differences {
            println!("    {difference}");
        }
    }
    Ok(differences.is_empty())
}
