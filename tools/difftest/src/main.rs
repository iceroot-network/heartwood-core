//! The `difftest` binary. `difftest help` lists the commands.

use std::path::PathBuf;
use std::process::ExitCode;

use heartwood_difftest::cli::{self, Command, GenesisArgs, USAGE};
use heartwood_difftest::genesis::{self, Built};
use heartwood_difftest::levels::{Level, l0};
use heartwood_difftest::oracle::GenOptions;
use heartwood_difftest::report::describe_meta;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;
use heartwood_difftest::{Error, Result, replay, subject};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = cli::parse(&args).and_then(execute);
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

/// Run a command; `Ok(false)` means it ran and found differences.
fn execute(command: Command) -> Result<bool> {
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
            Ok(report.is_success())
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
            Ok(replayed.report.is_success())
        }
        Command::Run { level, .. } => Err(Error::NotYet(format!(
            "{} is not built yet; it arrives with {}",
            level.title(),
            level.arrives_with()
        ))),
    }
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
