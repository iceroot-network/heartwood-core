//! Fork recovery: how many blocks a recovery removes, and the recovery itself.
//!
//! `forkBlock(block, n)` sets the forked block, clears and stops the queue, sets the rollback count
//! when given, and dispatches `FORK`. The recovery (the machine's `startForkRecovery`), unless one
//! runs already: stop the queue, remove the set number of blocks, or a random 4 to 102 when none is
//! set ([`RollbackDepth`], reproduced), clear the queue, set the last downloaded block to the
//! stored tip, reset the count, resume the queue, refresh the peers with a forced cleanse, and
//! dispatch `SUCCESS`. A live fork is judged by its converged result, never by the depth.

/// How many blocks a recovery without a set count removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackDepth {
    /// A random count from 4 to 102, as the reference draws it.
    Random,
    /// A count from 4 to 102 drawn from a generator seeded with this value (tests only).
    #[cfg(feature = "difftest")]
    Seeded(u64),
    /// Always this count (tests only).
    #[cfg(feature = "difftest")]
    Fixed(u32),
}

/// The lowest random count.
pub const MIN_RANDOM_DEPTH: u32 = 4;
/// The highest random count.
pub const MAX_RANDOM_DEPTH: u32 = 102;

impl RollbackDepth {
    /// The next count.
    pub fn draw(&mut self) -> u32 {
        todo!()
    }
}
