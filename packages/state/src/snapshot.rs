//! The published view of the state: what the API and the peer-to-peer client read between the
//! node's commits.
//!
//! The node publishes a [`StateView`] after each commit, never between two blocks of a chunk, so a
//! reader sees the state of a committed tip. A view is a copy: the wallets are cloned (a map of
//! shared records), the blocks are shared.

use std::sync::Arc;

use heartwood_crypto::Block;

use crate::machine::State;
use crate::round_state::RoundState;
use crate::stores::state::Totals;
use crate::wallets::WalletRepository;

/// The state at a committed tip, as the node publishes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateView {
    /// The last applied block.
    pub last_block: Arc<Block>,
    /// The genesis block.
    pub genesis: Arc<Block>,
    /// The wallets.
    pub wallets: WalletRepository,
    /// The round state: the round, its delegates and its forging list.
    pub round: RoundState,
    /// The supply totals.
    pub totals: Totals,
}

impl StateView {
    /// The height of the last block.
    pub fn height(&self) -> u32 {
        self.last_block.height()
    }
}

impl State {
    /// The view of this state, to publish.
    pub fn view(&self) -> StateView {
        StateView {
            last_block: Arc::new(self.last_block().clone()),
            genesis: Arc::new(self.genesis_block().clone()),
            wallets: self.wallets().clone(),
            round: self.round().clone(),
            totals: *self.totals(),
        }
    }

    /// The view of this state, sharing the genesis block of `previous` when it is the same block,
    /// so a node that publishes after every commit copies the genesis block once.
    pub fn view_after(&self, previous: &StateView) -> StateView {
        let genesis = if previous.genesis.id() == self.genesis_block().id() {
            Arc::clone(&previous.genesis)
        } else {
            Arc::new(self.genesis_block().clone())
        };
        StateView {
            last_block: Arc::new(self.last_block().clone()),
            genesis,
            wallets: self.wallets().clone(),
            round: self.round().clone(),
            totals: *self.totals(),
        }
    }
}
