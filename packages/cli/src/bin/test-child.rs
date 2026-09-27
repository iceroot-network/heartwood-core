//! The supervisor tests' child (feature `test-child`): it exits, runs, or ignores SIGINT, as its
//! arguments say, and writes a line to each output stream, so that the tests can check the
//! restart rules, the stop's signals and the captures.
//!
//!   test-child exit CODE [AFTER_MS] | run | ignore-sigint

use std::process::ExitCode;

fn main() -> ExitCode {
    todo!("the supervisor tests' child")
}
