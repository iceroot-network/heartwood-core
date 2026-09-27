//! The round of a height.
//!
//! With `n = activeDelegates` (from the milestone at height 1): round `r` holds the heights
//! `(r − 1) × n + 1` to `r × n`, and a new round starts at every height `h` with `h = 1` or
//! `(h − 1) mod n = 0`.
//!
//! Height 0 is not a block height. It gives round 0, round height 0 and next round 1, and it
//! starts no round; the reference has no defined answer there.

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

/// The round length: `activeDelegates` at height 1, which the loader keeps at 1 or more.
fn round_length(milestones: &Milestones) -> u64 {
    milestones.at(1).active_delegates().max(1)
}

/// The round of `height` (1 or more).
pub fn calculate_round(height: u32, milestones: &Milestones) -> RoundInfo {
    let n = round_length(milestones);
    let height = u64::from(height);
    let Some(index) = height.checked_sub(1).and_then(|h| h.checked_div(n)) else {
        return RoundInfo {
            round: 0,
            round_height: 0,
            next_round: 1,
            max_delegates: n,
        };
    };
    // index ≤ height − 1 < 2^32 and index × n ≤ height − 1, so nothing here can overflow.
    let round = index.saturating_add(1);
    let ends_round = height.checked_rem(n) == Some(0);
    RoundInfo {
        round,
        round_height: index.saturating_mul(n).saturating_add(1),
        next_round: round.saturating_add(u64::from(ends_round)),
        max_delegates: n,
    }
}

/// Whether a new round starts at `height`.
pub fn is_new_round(height: u32, milestones: &Milestones) -> bool {
    u64::from(height)
        .checked_sub(1)
        .and_then(|h| h.checked_rem(round_length(milestones)))
        == Some(0)
}
