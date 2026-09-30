//! The mixed forging runs' checks (`mixed-short`, `mixed`, `mixed-split`): every forger produced
//! in every round outside its fault windows; the scripted votes admitted on both sides, none for a
//! restarted relay's own delegates before the next round's start; a partition's sides and their
//! convergence after the heal; no reference relay's ban line naming a Heartwood address outside a
//! fault window, and no Heartwood violation against a reference node; the attestation evidence
//! (after round 1 every reference relay lists a version for every Heartwood-hosted delegate, and
//! every Heartwood relay's network-state record counts all 53 delegates before a partition).

use super::{Cases, ForgingRun};
use crate::errors::Result;

const SECTION: &str = "mixed-forging";

/// The mixed forging runs' checks (not built yet: one case with no subject, so a strict run
/// fails).
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built(SECTION, "the mixed forging network's checks");
    Ok(())
}
