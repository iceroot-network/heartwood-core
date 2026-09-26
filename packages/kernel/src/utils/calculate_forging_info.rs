//! The forger index of a slot: which delegate of the round's forging list may forge at a time.

use heartwood_crypto::Milestones;
use heartwood_crypto::crypto::slots;

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
///
/// The block time and the round length are those of height 1, so the height does not change the
/// result; it is taken because the reference's signature has it.
pub fn calculate_forging_info(timestamp: i64, height: u32, milestones: &Milestones) -> ForgingInfo {
    let _ = height;
    let params = milestones.at(1);
    let slot = slots::slot_info(timestamp, params.block_time());
    let n = i64::try_from(params.active_delegates())
        .unwrap_or(i64::MAX)
        .max(1);
    // `checked_rem` keeps the sign of the dividend, as JavaScript's `%` does; it fails only for a
    // divisor of 0 or −1, which `n` is not.
    let current_forger = slot.slot_number.checked_rem(n).unwrap_or(0);
    let next_forger = current_forger.saturating_add(1).checked_rem(n).unwrap_or(0);
    ForgingInfo {
        current_forger,
        next_forger,
        block_timestamp: slot.start_time,
        can_forge: slot.forging_status,
    }
}
