//! The churn run's checks (`churn`): every scripted event with its evidence; missed slots only
//! inside fault windows; the supervised crashes restarted, no process errored; the two-node outage
//! forging, the three-node outage halting and resuming; the split-host forger's delegates forging
//! and a vote for one refused `ERR_OFFLINE`; the relay-only node serving every route; the fresh
//! reference relay reaching the tip from one Heartwood relay alone, its judged digests equal to the
//! Heartwood lines.

use super::{Cases, ForgingRun};
use crate::errors::Result;

const SECTION: &str = "churn";

/// The churn run's checks (not built yet: one case with no subject, so a strict run fails).
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built(SECTION, "the churn run's checks");
    Ok(())
}
