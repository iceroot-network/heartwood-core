//! The fresh sync of a mixed run (run `w`'s node 5, run `s`'s node 4): a reference relay with an
//! empty store synced from one relay alone while the chain advanced; its block ids, the judge's
//! lines over its archive, and its round and missed-slot rows equal the serving chain's.

use super::{Cases, MixedRun};
use crate::errors::Result;

/// The fresh sync's checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("fresh", "the fresh sync from one relay alone");
    Ok(())
}
