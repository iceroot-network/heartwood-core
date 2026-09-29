//! The AwaitBlock report of a live run (`difftest live await RUN_DIR`): per Heartwood relay and
//! route, the requests, the share that waited, the median, 99th percentile and longest wait and
//! how the waits ended, beside the machine's load; the stall's window; and the record's minimums.
//! A mixed run's live check runs the same checks.

use std::path::Path;

use super::{Cases, MixedRun};
use crate::errors::{Error, Result};
use crate::report::Report;

/// The AwaitBlock report of the run in `run_dir`.
pub fn report(run_dir: &Path) -> Result<Report> {
    let _ = run_dir;
    Err(Error::NotYet(
        "live await has no Rust implementation yet".into(),
    ))
}

/// The AwaitBlock checks of a mixed run's live check.
pub(super) fn gate(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("await", "the AwaitBlock record and its minimums");
    Ok(())
}
