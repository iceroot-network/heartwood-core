//! The recovery commands in a mixed run (node 6 of runs `w` and `w0`): the pooled transfer kept
//! across a restart and gone after `pool clear`, the replay start after `state clear` and the vote
//! it admits, and the rollback's store summaries against the oracle's prediction for the run's own
//! chain, before and after the start that follows it, then the re-sync.

use super::{Cases, MixedRun};
use crate::errors::Result;

/// The recovery commands' checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("recovery", "the recovery commands on node 6");
    Ok(())
}
