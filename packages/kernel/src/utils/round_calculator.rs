//! The round of a height.
//!
//! With `n = activeDelegates` (from the milestone at height 1): round `r` holds the heights
//! `(r − 1) × n + 1` to `r × n`, and a new round starts at every height `h` with `h = 1` or
//! `(h − 1) mod n = 0`.

use heartwood_crypto::Milestones;

/// The round of a height, as the reference's `calculateRound` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundInfo {
    /// The round number, from 1.
    pub round: u64,
    /// The first height of the round.
    pub round_height: u64,
    /// The round of the next height: `round + 1` when the height is the last of its round, else
    /// `round`.
    pub next_round: u64,
    /// The number of delegates of a round, `activeDelegates`.
    pub max_delegates: u64,
}

/// The round of `height` (1 or more).
pub fn calculate_round(height: u32, milestones: &Milestones) -> RoundInfo {
    let _ = (height, milestones);
    todo!()
}

/// Whether a new round starts at `height`.
pub fn is_new_round(height: u32, milestones: &Milestones) -> bool {
    let _ = (height, milestones);
    todo!()
}
