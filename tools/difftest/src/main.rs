//! The `difftest` binary. `difftest help` lists the commands.

use std::path::PathBuf;
use std::process::ExitCode;

use heartwood_difftest::cli::{self, Command, USAGE};
use heartwood_difftest::levels::{Level, l0};
use heartwood_difftest::oracle::GenOptions;
use heartwood_difftest::report::describe_meta;
use heartwood_difftest::vectors::VectorSet;
use heartwood_difftest::{Error, Result, subject};

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
            let report = l0::run(&set, &subject::heartwood());
            print!("{report}");
            Ok(report.is_success())
        }
        Command::Run { level, .. } => Err(Error::NotYet(format!(
            "{} is not built yet; it arrives with {}",
            level.title(),
            level.arrives_with()
        ))),
    }
}
