//! Run `s`'s status attestations: the reference observer admits both scripted votes for delegates
//! of Heartwood relays, every forged block's slot had a quorum in the relays' network-state
//! records, and txgen's results equal the baseline's.

use super::{Cases, MixedRun};
use crate::errors::Result;

/// The attestations' checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("attest", "the reference observer's attestations");
    Ok(())
}
