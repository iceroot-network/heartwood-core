//! The node's events and its listeners.
//!
//! After each commit the node thread reports what the commit did ([`NodeEvent`], by the
//! reference's event names) to its [`NodeListener`], which runs the reference's internal
//! listeners as ordered calls: `pingAll` on each applied round while synced, the peers of an
//! invalid version disconnected on a milestone change, and the events on the kernel's bus. The
//! pool's clean-up per applied block runs on the node thread itself ([`crate::pool_task`]).
//!
//! A [`CommitObserver`] sees every applied and reverted block on the node thread, right after the
//! state changed and before the next block: the harness's digest writer.

use heartwood_crypto::Block;
use heartwood_kernel::Event;
use heartwood_state::State;

/// What a commit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEvent {
    /// `block.applied`: the block at `height` was applied; `round_started` is the round it
    /// started, when it ended one (`round.applied`).
    BlockApplied {
        /// The block's height.
        height: u32,
        /// The round the block started.
        round_started: Option<u64>,
    },
    /// `block.reverted`: the block at `height` was reverted.
    BlockReverted {
        /// The block's height.
        height: u32,
    },
    /// `crypto.milestone.changed`: a new milestone is in force from `height`.
    MilestoneChanged {
        /// The height.
        height: u32,
    },
}

impl NodeEvent {
    /// The reference's event.
    pub fn event(&self) -> Event {
        match self {
            NodeEvent::BlockApplied { .. } => Event::BlockApplied,
            NodeEvent::BlockReverted { .. } => Event::BlockReverted,
            NodeEvent::MilestoneChanged { .. } => Event::MilestoneChanged,
        }
    }
}

/// Who hears of each commit's events, after the commit, in order.
pub trait NodeListener: Send + Sync {
    /// The events of one commit.
    fn committed(&self, events: &[NodeEvent]);
}

/// Events heard by no one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoListener;

impl NodeListener for NoListener {
    fn committed(&self, _events: &[NodeEvent]) {}
}

/// Who sees every state change on the node thread.
pub trait CommitObserver: Send {
    /// `block` was applied; `state` is the state after it.
    fn applied(&mut self, state: &State, block: &Block);

    /// The last block was reverted; `state` is the state after the revert.
    fn reverted(&mut self, state: &State);

    /// The node started from the store; `state` is the restored state.
    fn restarted(&mut self, state: &State);
}

/// State changes seen by no one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoObserver;

impl CommitObserver for NoObserver {
    fn applied(&mut self, _state: &State, _block: &Block) {}

    fn reverted(&mut self, _state: &State) {}

    fn restarted(&mut self, _state: &State) {}
}
