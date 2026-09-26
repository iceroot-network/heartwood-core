//! The forger index of a slot: which delegate of the round's forging list may forge at a time.

use heartwood_crypto::Milestones;

/// The forging slot of a time, as the reference's `calculateForgingInfo` gives it.
///
/// The indexes are the slot number modulo `activeDelegates` with JavaScript's `%`, so they are
/// negative for a time before the network epoch, where no delegate is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgingInfo {
    /// The index of the delegate that forges in the slot of the time.
    pub current_forger: i64,
    /// The index of the delegate that forges in the next slot.
    pub next_forger: i64,
    /// The first second of the slot.
    pub block_timestamp: i64,
    /// Whether the time is in the first half of its slot, where forging is allowed.
    pub can_forge: bool,
}

/// The forging slot of `timestamp` (seconds since the network epoch) for a block at `height`.
pub fn calculate_forging_info(timestamp: i64, height: u32, milestones: &Milestones) -> ForgingInfo {
    let _ = (timestamp, height, milestones);
    todo!()
}
