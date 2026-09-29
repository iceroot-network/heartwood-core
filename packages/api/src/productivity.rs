//! The delegates' productivity and the chain's totals over the store ([`StoreProductivity`]), as
//! the routes read them through [`Productivity`].
//!
//! The reference computes productivity at each queue drain into its wallets' attributes; here it
//! is computed from the store at the first request after the tip changes, and kept for that tip.
//! The chain's totals are summed once and then grown by the blocks each new tip adds.

use std::sync::Arc;

use heartwood_crypto::Block;
use heartwood_database::Readers;

use crate::context::{ChainTotals, Productivity, ProductivityTable};
use crate::errors::ApiError;

/// [`Productivity`] over the store's read-only connections.
pub struct StoreProductivity {
    history: Arc<Readers>,
    lookback_seconds: i64,
}

impl StoreProductivity {
    /// The productivity of the store behind `history`, over the reference's `missedBlocksLookback`
    /// window of `lookback_seconds`.
    pub fn new(history: Arc<Readers>, lookback_seconds: i64) -> StoreProductivity {
        StoreProductivity {
            history,
            lookback_seconds,
        }
    }
}

impl Productivity for StoreProductivity {
    fn table(&self, tip: &Block) -> Result<Arc<ProductivityTable>, ApiError> {
        let _ = (&self.history, self.lookback_seconds, tip);
        todo!("the productivity table per tip")
    }

    fn totals(&self, tip: &Block) -> Result<ChainTotals, ApiError> {
        let _ = (&self.history, tip);
        todo!("the chain's totals per tip")
    }
}
