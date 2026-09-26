//! The block reward by delegate rank.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::RewardError;
use crate::managers::Params;

/// The reward of the delegate at `rank` under the merged milestone `params`.
///
/// With the dynamic reward enabled, the rank table decides and a missing rank is an error;
/// otherwise the milestone's `reward` applies (0 when absent).
pub fn calculate_reward(params: &Params, rank: u32) -> Result<u64, RewardError> {
    todo!()
}
