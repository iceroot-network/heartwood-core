//! Fork recovery: how many blocks a recovery removes, and the recovery itself.
//!
//! `forkBlock(block, n)` sets the forked block, clears and stops the queue, sets the rollback count
//! when given, and dispatches `FORK`. The recovery (the machine's `startForkRecovery`), unless one
//! runs already: stop the queue, remove the set number of blocks, or a random 4 to 102 when none is
//! set ([`RollbackDepth`], reproduced), clear the queue, set the last downloaded block to the
//! stored tip, reset the count, resume the queue, refresh the peers with a forced cleanse, and
//! dispatch `SUCCESS`. A live fork is judged by its converged result, never by the depth.
//!
//! The recovery is the machine's `startForkRecovery` action, run by the blockchain service. A
//! failed removal stops the node, and the machine exits (`FAILURE`).

use std::sync::Arc;

use heartwood_kernel::info;

use crate::blockchain::{Blockchain, Queue};
use crate::network::Network;
use crate::state_machine::MachineEvent;

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
/// How many random counts there are.
const SPAN: u64 = 99;

impl RollbackDepth {
    /// The next count: from 4 to 102, each equally likely, for the random depths.
    pub fn draw(&mut self) -> u32 {
        let random = match self {
            // A system without a random source leaves the lowest count.
            RollbackDepth::Random => getrandom::u64().unwrap_or(0),
            #[cfg(feature = "difftest")]
            RollbackDepth::Seeded(state) => split_mix(state),
            #[cfg(feature = "difftest")]
            RollbackDepth::Fixed(count) => return *count,
        };
        // The remainder is below 99.
        MIN_RANDOM_DEPTH.saturating_add(u32::try_from(random % SPAN).unwrap_or(0))
    }
}

impl<N: Network, Q: Queue> Blockchain<N, Q> {
    /// `startForkRecovery` (see the module documentation).
    pub(crate) async fn start_fork_recovery(self: Arc<Self>) {
        if self.flags().forking {
            return;
        }
        let set = self.chain().blocks_to_rollback;
        let count = if set > 0 { set } else { self.draw_depth() };
        self.flags().forking = true;
        info!("Starting fork recovery :fork_and_knife:");
        self.queue.stop().await;
        let removed = self.remove_blocks(count).await;
        self.queue.clear();
        self.rolled_back(count);
        if removed.is_err() {
            return self.dispatch(MachineEvent::Failure);
        }
        self.queue.resume();
        self.parts.network.refresh_peers_after_fork().await;
        self.dispatch(MachineEvent::Success);
    }
}

/// The next value of the SplitMix64 generator whose state is `state`.
#[cfg(feature = "difftest")]
fn split_mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_span_covers_the_counts() {
        assert_eq!(SPAN, u64::from(MAX_RANDOM_DEPTH - MIN_RANDOM_DEPTH + 1));
        let mut depth = RollbackDepth::Random;
        assert!((0..200).all(|_| (MIN_RANDOM_DEPTH..=MAX_RANDOM_DEPTH).contains(&depth.draw())));
    }
}
