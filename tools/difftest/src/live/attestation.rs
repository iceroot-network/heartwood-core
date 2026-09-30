//! The attestation's timing (recorded, not gated): from each Heartwood relay's attestation record
//! (an offer, its signatures' attachment, each status served unsigned after the wait; written by a
//! harness build), the wait from an offer to its signatures and the unsigned statuses served, per
//! relay and round, beside the network-state proxy.

use super::{Cases, ForgingRun};
use crate::errors::Result;

const SECTION: &str = "attestation";

/// The attestation record's report (not built yet: one case with no subject, so a strict run
/// fails).
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built(
        SECTION,
        "the attestation record's waits and unsigned statuses",
    );
    Ok(())
}
