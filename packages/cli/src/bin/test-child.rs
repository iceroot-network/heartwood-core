//! The supervisor tests' child (feature `test-child`): it exits, runs, or ignores SIGINT, as its
//! arguments say, and writes a line to each output stream, so that the tests can check the
//! restart rules, the stop's signals and the captures. It can also stand for a node, holding the
//! role's lock and writing its pid file as `<role> run` does, and for `heartwood supervise`.
//!
//!   test-child exit CODE [AFTER_MS]      exit with CODE, after AFTER_MS milliseconds
//!   test-child run                       run until a signal ends it
//!   test-child ignore-sigint             run with SIGINT blocked, until SIGKILL
//!   test-child env NAME...               print `NAME=value` for each variable, then run
//!   test-child relay|forger run [...]    as `run`, holding `$CORE_PATH_DATA/run/<role>.lock` and
//!                                        writing `<role>.pid` beside it
//!   test-child supervise ...             `heartwood supervise`

use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use clap::Parser;
use heartwood_cli::supervisor::SuperviseArgs;
use heartwood_cli::supervisor::signals::take_lock;
use nix::sys::signal::{SigSet, Signal};

/// `test-child supervise ...`.
#[derive(Debug, Parser)]
#[command(name = "test-child")]
enum Supervise {
    /// The supervisor.
    Supervise(SuperviseArgs),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let pid = std::process::id();
    let sigint: SigSet = [Signal::SIGINT].into_iter().collect();
    if words == ["ignore-sigint"] && sigint.thread_block().is_err() {
        return ExitCode::FAILURE;
    }
    println!("test-child: {} {pid}", words.join(" "));
    eprintln!("test-child: error line {pid}");
    match words.as_slice() {
        ["supervise", ..] => match Supervise::try_parse() {
            Ok(Supervise::Supervise(args)) => args.run(),
            Err(e) => e.exit(),
        },
        ["exit", code, rest @ ..] => {
            let after = rest.first().and_then(|ms| ms.parse().ok()).unwrap_or(0);
            thread::sleep(Duration::from_millis(after));
            ExitCode::from(code.parse::<u8>().unwrap_or(1))
        }
        ["run" | "ignore-sigint"] => run_forever(),
        ["env", names @ ..] => {
            for name in names {
                let value = std::env::var(name).unwrap_or_default();
                println!("{name}={value}");
            }
            run_forever()
        }
        [role @ ("relay" | "forger"), "run", ..] => {
            let run =
                PathBuf::from(std::env::var_os("CORE_PATH_DATA").unwrap_or_default()).join("run");
            let lock = match take_lock(&run.join(format!("{role}.lock"))) {
                Ok(Some(lock)) => lock,
                _ => return ExitCode::FAILURE,
            };
            if std::fs::write(run.join(format!("{role}.pid")), pid.to_string()).is_err() {
                return ExitCode::FAILURE;
            }
            let _held = lock;
            run_forever()
        }
        _ => ExitCode::from(2),
    }
}

fn run_forever() -> ! {
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
