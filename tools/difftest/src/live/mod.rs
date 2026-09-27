//! `difftest live check`: the comparisons of a live run's directory (the devnet tooling writes it):
//! node 1's archive as a corpus judged by the oracle, each Heartwood node's digest lines against
//! the judge's (a torn last line dropped), the store anchors, the step reports, the recorded API
//! answers, the ban scan with its fault windows, and the invariants.

use std::path::Path;

use crate::errors::Result;
use crate::report::Report;

/// Check the live run in `run_dir`.
pub fn check(run_dir: &Path) -> Result<Report> {
    let _ = run_dir;
    todo!()
}
