//! The resource report of a forging run: each node's whole process tree sampled every 10 s (resident
//! and proportional memory, CPU time, written bytes) into the run's series, against the budgets (a
//! relay at 60 MB typical and 120 MB with a full pool, a forger at 15 MB, five nodes at 0.5 GB,
//! five nodes at 5 % of one core with empty blocks, 100 MB a day of disk); a budget missed is
//! reported with its cause, never re-baselined.

use std::path::Path;

use super::{Cases, ForgingRun};
use crate::errors::{Error, Result};
use crate::report::Report;

const SECTION: &str = "resources";

/// The resource rules within a run's check (not built yet: one case with no subject, so a strict
/// run fails).
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built(SECTION, "the resource series against the budgets");
    Ok(())
}

/// `difftest live resources RUN_DIR`: the run's resource report.
pub fn report(run_dir: &Path) -> Result<Report> {
    let _ = run_dir;
    Err(Error::NotYet("live resources is not built yet".into()))
}
