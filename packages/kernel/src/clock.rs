//! The time seam.
//!
//! Every reading of the wall clock outside the logger goes through [`Clock`], so that tests and
//! replays can set the time with `ManualClock` (feature `difftest`). Consensus code never reads
//! a clock: it takes the time as an argument.

#[cfg(feature = "difftest")]
use std::sync::atomic::{AtomicI64, Ordering};

/// A source of the current time.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[expect(
        clippy::disallowed_types,
        reason = "the system clock is the one place that reads the wall clock"
    )]
    fn now_ms(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
        }
    }
}

/// A clock that shows the time it was last set to (feature `difftest`, never in a release
/// build).
#[cfg(feature = "difftest")]
#[derive(Debug, Default)]
pub struct ManualClock {
    now_ms: AtomicI64,
}

#[cfg(feature = "difftest")]
impl ManualClock {
    /// A clock at `now_ms` milliseconds since the Unix epoch.
    pub fn new(now_ms: i64) -> ManualClock {
        ManualClock {
            now_ms: AtomicI64::new(now_ms),
        }
    }

    /// Set the time.
    pub fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }

    /// Move the time forward by `ms` milliseconds (backward when negative).
    pub fn advance(&self, ms: i64) {
        self.now_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

#[cfg(feature = "difftest")]
impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_is_after_2020() {
        assert!(SystemClock.now_ms() > 1_577_836_800_000);
    }

    #[cfg(feature = "difftest")]
    #[test]
    fn the_manual_clock_shows_what_it_was_set_to() {
        let clock = ManualClock::new(1_000);
        clock.advance(500);
        assert_eq!(clock.now_ms(), 1_500);
        clock.set(7);
        assert_eq!(clock.now_ms(), 7);
    }
}
