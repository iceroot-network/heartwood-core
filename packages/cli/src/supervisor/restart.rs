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
        let _ = (uptime, stop_requested);
        todo!("the restart rule")
    }
}
