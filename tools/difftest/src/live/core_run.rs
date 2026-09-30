//! The core run's checks (`core`): one process per node holding both roles' locks; forging and
//! attestations as a split node (all 53 delegates counted); `relay start` refused while `core`
//! runs; `devnet stop` graceful within 30 s.

use super::{Cases, ForgingRun};
use crate::errors::Result;

const SECTION: &str = "core";

/// The core run's checks (not built yet: one case with no subject, so a strict run fails).
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built(SECTION, "the core run's checks");
    Ok(())
}
