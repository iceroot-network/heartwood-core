//! Donations taken from the block reward.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::identities::Address;
use crate::managers::Params;

/// The donation paid to each recipient of the merged milestone `params` out of `reward`, in the
/// order of the milestone file: `floor(reward × basisPoints / 10000)` each. The remainder stays
/// with the forger.
pub fn calculate_donations(reward: u64, params: &Params) -> Vec<(Address, u64)> {
    todo!()
}
