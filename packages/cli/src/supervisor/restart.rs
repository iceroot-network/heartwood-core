//! The restart rule, pm2's: unlimited restarts, a run under 1 s counted as unstable, `errored`
//! after 5 unstable runs in a row, no back-off.

use std::time::Duration;

/// A run shorter than this is unstable.
pub const MIN_UPTIME: Duration = Duration::from_secs(1);
/// After this many unstable runs in a row the supervisor gives up.
pub const MAX_UNSTABLE_RESTARTS: u32 = 5;

/// What the supervisor does after its node exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterExit {
    /// Start the node again.
    Restart,
    /// Stop: the exit was asked for.
    Stop,
    /// Give up: mark the role `errored` and exit.
    GiveUp,
}

/// The restart counters of one supervisor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Restarts {
    /// Restarts since the supervisor started.
    pub total: u32,
    /// Unstable runs in a row.
    pub unstable: u32,
}

impl Restarts {
    /// Count a node's exit after `uptime`, `stop_requested` telling whether a stop was asked for,
    /// and decide what follows.
    pub fn after_exit(&mut self, uptime: Duration, stop_requested: bool) -> AfterExit {
        if stop_requested {
            return AfterExit::Stop;
        }
        if uptime < MIN_UPTIME {
            self.unstable = self.unstable.saturating_add(1);
            if self.unstable >= MAX_UNSTABLE_RESTARTS {
                return AfterExit::GiveUp;
            }
        } else {
            self.unstable = 0;
        }
        self.total = self.total.saturating_add(1);
        AfterExit::Restart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(999);

    #[test]
    fn a_run_of_one_second_or_more_restarts_at_once_and_resets_the_unstable_count() {
        let mut restarts = Restarts::default();
        assert_eq!(restarts.after_exit(SHORT, false), AfterExit::Restart);
        assert_eq!(restarts.unstable, 1);
        assert_eq!(restarts.after_exit(MIN_UPTIME, false), AfterExit::Restart);
        assert_eq!(
            restarts,
            Restarts {
                total: 2,
                unstable: 0
            }
        );
    }

    #[test]
    fn the_fifth_unstable_run_in_a_row_gives_up() {
        let mut restarts = Restarts::default();
        for _ in 1..MAX_UNSTABLE_RESTARTS {
            assert_eq!(restarts.after_exit(SHORT, false), AfterExit::Restart);
        }
        assert_eq!(restarts.after_exit(SHORT, false), AfterExit::GiveUp);
        assert_eq!(restarts.total, MAX_UNSTABLE_RESTARTS - 1);
    }

    #[test]
    fn a_stable_run_between_unstable_ones_starts_the_count_again() {
        let mut restarts = Restarts::default();
        for _ in 0..40 {
            for _ in 1..MAX_UNSTABLE_RESTARTS {
                assert_eq!(restarts.after_exit(SHORT, false), AfterExit::Restart);
            }
            assert_eq!(
                restarts.after_exit(Duration::from_secs(60), false),
                AfterExit::Restart
            );
        }
        assert_eq!(restarts.total, 40 * MAX_UNSTABLE_RESTARTS);
    }

    #[test]
    fn an_exit_that_was_asked_for_stops_whatever_its_uptime() {
        let mut restarts = Restarts::default();
        assert_eq!(restarts.after_exit(SHORT, true), AfterExit::Stop);
        assert_eq!(restarts.after_exit(MIN_UPTIME, true), AfterExit::Stop);
        assert_eq!(restarts, Restarts::default());
    }
}
