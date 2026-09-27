//! The crash test's child (feature `difftest`): a forger over a configuration directory, run
//! against the harness's mock relay, which the harness kills with SIGKILL at random moments. It
//! reports each write of its signing record on its standard output, so that the harness can kill
//! it between the write and the relay's answer.
//!
//!   forger-child --config DIR --data DIR --relay IP:PORT

use std::process::ExitCode;

fn main() -> ExitCode {
    todo!("the crash test's child")
}
