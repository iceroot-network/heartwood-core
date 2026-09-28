//! The restart rule, pm2's (`handleExit`): unlimited restarts with no back-off; a run under 1 s
//! counts as unstable only while the supervisor is younger than 5 s (`min_uptime` times
//! `max_restarts`), and the count is never reset, so the role is `errored` at the 5th unstable run
//! of that first window. A node that runs for a while and then crash-loops is restarted for good.

use std::time::Duration;

/// A run shorter than this is unstable.
pub const MIN_UPTIME: Duration = Duration::from_secs(1);
/// At this many unstable runs the supervisor gives up.
pub const MAX_UNSTABLE_RESTARTS: u32 = 5;
/// Only exits this soon after the supervisor started count as unstable.
pub const WINDOW: Duration = MIN_UPTIME.saturating_mul(MAX_UNSTABLE_RESTARTS);

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
    /// Unstable runs.
    pub unstable: u32,
}

impl Restarts {
    /// Count a node's exit after `uptime`, `age` after its supervisor started, `stop_requested`
    /// telling whether a stop was asked for, and decide what follows.
    pub fn after_exit(
        &mut self,
        uptime: Duration,
        age: Duration,
        stop_requested: bool,
    ) -> AfterExit {
        if stop_requested {
            return AfterExit::Stop;
        }
        if age < WINDOW && uptime < MIN_UPTIME {
            self.unstable = self.unstable.saturating_add(1);
        }
        if self.unstable >= MAX_UNSTABLE_RESTARTS {
            return AfterExit::GiveUp;
        }
        self.total = self.total.saturating_add(1);
        AfterExit::Restart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(999);
    const SOON: Duration = Duration::from_secs(1);
    const LATER: Duration = WINDOW;

    #[test]
    fn a_run_of_one_second_or_more_restarts_at_once_and_is_not_counted() {
        let mut restarts = Restarts::default();
        assert_eq!(restarts.after_exit(SHORT, SOON, false), AfterExit::Restart);
        assert_eq!(restarts.unstable, 1);
        let stable = restarts.after_exit(MIN_UPTIME, SOON, false);
        assert_eq!(stable, AfterExit::Restart);
        assert_eq!(
            restarts,
            Restarts {
                total: 2,
                unstable: 1
            }
        );
    }

    #[test]
    fn the_fifth_unstable_run_of_the_first_seconds_gives_up() {
        let mut restarts = Restarts::default();
        for _ in 1..MAX_UNSTABLE_RESTARTS {
            assert_eq!(restarts.after_exit(SHORT, SOON, false), AfterExit::Restart);
        }
        assert_eq!(restarts.after_exit(SHORT, SOON, false), AfterExit::GiveUp);
        assert_eq!(restarts.total, MAX_UNSTABLE_RESTARTS - 1);
    }

    #[test]
    fn a_stable_run_does_not_start_the_count_again() {
        let mut restarts = Restarts::default();
        for _ in 1..MAX_UNSTABLE_RESTARTS {
            restarts.after_exit(SHORT, SOON, false);
        }
        let stable = restarts.after_exit(Duration::from_secs(60), SOON, false);
        assert_eq!(stable, AfterExit::Restart);
        assert_eq!(restarts.unstable, MAX_UNSTABLE_RESTARTS - 1);
        assert_eq!(restarts.after_exit(SHORT, SOON, false), AfterExit::GiveUp);
    }

    #[test]
    fn a_node_that_crash_loops_after_the_first_seconds_is_restarted_for_good() {
        let mut restarts = Restarts::default();
        let hour = Duration::from_secs(3600);
        assert_eq!(restarts.after_exit(hour, hour, false), AfterExit::Restart);
        for _ in 0..100 {
            assert_eq!(restarts.after_exit(SHORT, LATER, false), AfterExit::Restart);
        }
        assert_eq!(restarts.unstable, 0);
        assert_eq!(restarts.total, 101);
    }

    #[test]
    fn an_exit_that_was_asked_for_stops_whatever_its_uptime() {
        let mut restarts = Restarts::default();
        assert_eq!(restarts.after_exit(SHORT, SOON, true), AfterExit::Stop);
        assert_eq!(restarts.after_exit(MIN_UPTIME, SOON, true), AfterExit::Stop);
        assert_eq!(restarts, Restarts::default());
    }
}
