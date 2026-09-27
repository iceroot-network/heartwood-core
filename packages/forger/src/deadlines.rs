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
//! The routes a forger never calls get 20 s. A call past its deadline fails as the reference's
//! `RelayCommunicationError` ("Request timed out") and is counted as the loop counts that error.
//! When a computed wait is zero or negative, the loop waits [`POLL_FLOOR`] instead of polling
//! again at once.

use std::time::Duration;

use heartwood_p2p_wire::RouteId;

use crate::seams::Deadline;

/// The shortest wait of the slot loop: a computed delay of zero or less (a forger's clock ahead of
/// its relay's) waits this long, where the reference polls again at once.
pub const POLL_FLOOR: Duration = Duration::from_millis(200);

/// The timeout of a call on `route`, `to_next_slot_end` being the time from now to the end of the
/// slot after the current one, by the forger's clock.
pub fn timeout(route: RouteId, to_next_slot_end: Duration) -> Duration {
    let seconds = Duration::from_secs;
    match route {
        RouteId::GetNetworkState | RouteId::GetUnconfirmedTransactions | RouteId::PostBlock => {
            to_next_slot_end.max(seconds(4))
        }
        RouteId::GetStatus | RouteId::GetSlotNumber => to_next_slot_end.max(seconds(2)),
        RouteId::GetStatusToSign => seconds(15),
        RouteId::PostStatusSignatures => seconds(2),
        _ => seconds(20),
    }
}

/// The deadline of a call on `route` made now.
pub fn deadline(route: RouteId, to_next_slot_end: Duration) -> Deadline {
    Deadline::after(timeout(route, to_next_slot_end))
}

/// The time from `now_ms` to the end of the slot after the current one: the slots of `block_time`
/// seconds from `epoch_ms` (both Unix milliseconds), before the epoch as after it. Never
/// negative.
pub fn to_next_slot_end(now_ms: i64, epoch_ms: i64, block_time: u64) -> Duration {
    let span = i128::from(block_time.max(1)).saturating_mul(1_000);
    let elapsed = i128::from(now_ms).saturating_sub(i128::from(epoch_ms));
    let slot = elapsed.checked_div_euclid(span).unwrap_or_default();
    let end = slot.saturating_add(2).saturating_mul(span);
    Duration::from_millis(u64::try_from(end.saturating_sub(elapsed)).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_has_its_deadline() {
        let long = Duration::from_millis(9_500);
        let short = Duration::from_millis(900);
        for (route, near, far) in [
            (RouteId::GetCurrentRound, 20_000, 20_000),
            (RouteId::SyncBlockchain, 20_000, 20_000),
            (RouteId::EmitEvent, 20_000, 20_000),
            (RouteId::GetNetworkState, 4_000, 9_500),
            (RouteId::GetUnconfirmedTransactions, 4_000, 9_500),
            (RouteId::PostBlock, 4_000, 9_500),
            (RouteId::GetStatus, 2_000, 9_500),
            (RouteId::GetSlotNumber, 2_000, 9_500),
            (RouteId::GetStatusToSign, 15_000, 15_000),
            (RouteId::PostStatusSignatures, 2_000, 2_000),
            (RouteId::GetBlocks, 20_000, 20_000),
        ] {
            assert_eq!(
                timeout(route, short),
                Duration::from_millis(near),
                "{route:?}"
            );
            assert_eq!(
                timeout(route, long),
                Duration::from_millis(far),
                "{route:?}"
            );
        }
    }

    #[test]
    fn the_next_slot_ends_one_whole_slot_after_the_current_one() {
        let epoch = 1_000_000;
        let at = |ms: i64| to_next_slot_end(epoch + ms, epoch, 8);
        assert_eq!(at(0), Duration::from_millis(16_000));
        assert_eq!(at(1), Duration::from_millis(15_999));
        assert_eq!(at(7_999), Duration::from_millis(8_001));
        assert_eq!(at(8_000), Duration::from_millis(16_000));
        assert_eq!(
            at(-1),
            Duration::from_millis(8_001),
            "slot -1 before the epoch"
        );
        assert_eq!(at(-8_000), Duration::from_millis(16_000));
        assert_eq!(to_next_slot_end(0, 0, 0), Duration::from_millis(2_000));
        let extreme = to_next_slot_end(i64::MAX, i64::MIN, u64::MAX);
        assert!(extreme > Duration::ZERO);
    }
}
