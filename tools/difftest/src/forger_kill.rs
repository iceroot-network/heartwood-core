//! The forger's crash test (`difftest forger crash`): the `forger-child` binary against a mock
//! relay on a test network with a 3 s block time, killed with SIGKILL at random moments, most of
//! them between the signing record's write (which the child reports on its output) and the relay's
//! answer to its `postBlock`. The mock relay makes the child's delegate the forger of every slot
//! and holds each `postBlock` answer for a random time.
//!
//! After every restart no slot may have two different blocks at the mock relay, and a restart
//! inside a recorded slot must send the recorded bytes again.

use std::path::PathBuf;

use crate::errors::Result;
use crate::report::Report;

/// What a crash test runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillOptions {
    /// The number of kills.
    pub kills: u32,
    /// The seed of the kill moments and the relay's delays.
    pub seed: u64,
    /// The `forger-child` binary.
    pub child: PathBuf,
}

/// Run the crash test of `options`.
pub fn run(options: &KillOptions) -> Result<Report> {
    let _ = options;
    todo!("the forger's crash test")
}
