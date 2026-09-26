//! The block reward by delegate rank.

use crate::errors::RewardError;
use crate::managers::Params;

/// The reward of the delegate at `rank` under the merged milestone `params`.
///
/// With the dynamic reward enabled, the rank table decides and a missing rank is an error;
/// otherwise the milestone's `reward` applies (0 when absent).
pub fn calculate_reward(params: &Params, rank: u32) -> Result<u64, RewardError> {
    match params.dynamic_reward() {
        Some(table) if table.enabled() => {
            table.rank(rank).ok_or(RewardError::NoRankReward { rank })
        }
        _ => Ok(params.reward()),
    }
}
