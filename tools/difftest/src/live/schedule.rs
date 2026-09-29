//! The schedule of a mixed run: every step's recorded start height in its planned round, the steps
//! in their planned order, and the pause at the run's stop height.

use super::{Cases, MixedRun};
use crate::errors::Result;

/// The schedule's checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("schedule", "every step in its planned round and order");
    Ok(())
}
