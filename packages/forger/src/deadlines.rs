//! The deadline of each forger call. The reference sets none (a relay whose handler never answers
//! holds its forger); Heartwood's never fire where the reference could still forge.
//!
//! | Route | Deadline |
//! |---|---|
//! | `getCurrentRound`, `syncBlockchain`, `emitEvent` | 20 s |
//! | `getNetworkState`, `getUnconfirmedTransactions`, `postBlock` | the longer of 4 s and the time to the end of the next slot |
//! | `getStatus`, `getSlotNumber` | the longer of 2 s and the time to the end of the next slot |
//! | `getStatusToSign` | 15 s |
//! | `postStatusSignatures` | 2 s |
//!
//! A call past its deadline fails as the reference's `RelayCommunicationError` and is counted as
//! the loop counts that error. When a computed wait is zero or negative, the loop waits
//! [`POLL_FLOOR`] instead of polling again at once.

use std::time::Duration;

use heartwood_p2p_wire::RouteId;

use crate::seams::Deadline;

/// The shortest wait of the slot loop: a computed delay of zero or less (a forger's clock ahead of
/// its relay's) waits this long, where the reference polls again at once.
pub const POLL_FLOOR: Duration = Duration::from_millis(200);

/// The timeout of a call on `route`, `to_next_slot_end` being the time from now to the end of the
/// slot after the current one, by the forger's clock.
pub fn timeout(route: RouteId, to_next_slot_end: Duration) -> Duration {
    let _ = (route, to_next_slot_end);
    todo!("the call deadlines")
}

/// The deadline of a call on `route` made now.
pub fn deadline(route: RouteId, to_next_slot_end: Duration) -> Deadline {
    Deadline::after(timeout(route, to_next_slot_end))
}

/// The time from `now_ms` to the end of the slot after the current one: the slots of `block_time`
/// seconds from `epoch_ms` (both Unix milliseconds). Zero before the epoch's first slot has an
/// end, never negative.
pub fn to_next_slot_end(now_ms: i64, epoch_ms: i64, block_time: u64) -> Duration {
    let _ = (now_ms, epoch_ms, block_time);
    todo!("the time to the next slot's end")
}
