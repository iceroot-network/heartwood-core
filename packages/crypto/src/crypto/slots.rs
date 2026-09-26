//! Slot arithmetic with one block time from height 1.
//!
//! Times are whole seconds since the network epoch, as the reference's `Slots.getTime` gives them;
//! they can be negative before the epoch, and every division rounds toward negative infinity, as
//! `Math.floor` does.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

/// The slot that a timestamp falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotInfo {
    /// First second of the slot.
    pub start_time: i64,
    /// Last second of the slot.
    pub end_time: i64,
    /// The block time, in seconds.
    pub block_time: u64,
    /// The slot number.
    pub slot_number: i64,
    /// Whether the timestamp is in the first half of the slot, where forging is allowed.
    pub forging_status: bool,
}

/// Milliseconds since the Unix epoch of the milestone `epoch` text (an ISO 8601 date and time),
/// or `None` if the text is not one.
pub fn epoch_unix_ms(epoch: &str) -> Option<i64> {
    todo!()
}

/// Seconds since the network epoch at `now_unix_ms`: `floor((now − epoch) / 1000)`.
pub fn epoch_time(now_unix_ms: i64, epoch_unix_ms: i64) -> i64 {
    todo!()
}

/// The slot of `timestamp` with `block_time`-second slots.
pub fn slot_info(timestamp: i64, block_time: u64) -> SlotInfo {
    todo!()
}

/// The slot number of `timestamp`.
pub fn slot_number(timestamp: i64, block_time: u64) -> i64 {
    todo!()
}

/// The first second of `slot`.
pub fn slot_time(slot: i64, block_time: u64) -> i64 {
    todo!()
}

/// Whether forging is allowed at `timestamp`: it is in the first half of its slot.
pub fn is_forging_allowed(timestamp: i64, block_time: u64) -> bool {
    todo!()
}
