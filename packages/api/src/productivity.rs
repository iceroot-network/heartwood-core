//! The delegates' productivity and the chain's totals over the store ([`StoreProductivity`]), as
//! the routes read them through [`Productivity`].
//!
//! The reference computes productivity at each queue drain into its wallets' attributes; here it
//! is computed from the store at the first request after the tip changes, and kept for that tip.
//! The chain's totals are summed once and then grown by the blocks each new tip adds, and summed
//! again after a revert.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Block, BlockId};
use heartwood_database::{DatabaseError, Reader, hundredths};
use indexmap::IndexMap;

use crate::context::{
    ChainTotals, DelegateProduction, Hundredths, Productivity, ProductivityTable,
};
use crate::errors::ApiError;

/// Seconds in a day.
const DAY: i64 = 86_400;

/// The table at one tip.
struct Cached {
    id: BlockId,
    table: Arc<ProductivityTable>,
}

/// The totals at one tip.
struct Summed {
    id: BlockId,
    height: u32,
    fees: u128,
    burns: u128,
    rewards: u128,
    issued: u128,
}

/// What [`StoreProductivity`] keeps between calls.
#[derive(Default)]
struct Kept {
    /// The table of the last tip that computed, and the table the routes fall back on.
    table: Option<Cached>,
    /// The table of the last computation that succeeded, for a tip whose computation fails.
    last: Option<Arc<ProductivityTable>>,
    totals: Option<Summed>,
}

/// [`Productivity`] over the store's read-only connections.
pub struct StoreProductivity {
    history: Arc<heartwood_database::Readers>,
    lookback_seconds: i64,
    epoch_unix_ms: i64,
    kept: Mutex<Kept>,
}

impl StoreProductivity {
    /// The productivity of the store behind `history`, over the reference's `missedBlocksLookback`
    /// window of `lookback_seconds`, on a network whose epoch is the Unix epoch (see
    /// [`StoreProductivity::with_epoch`]).
    pub fn new(
        history: Arc<heartwood_database::Readers>,
        lookback_seconds: i64,
    ) -> StoreProductivity {
        StoreProductivity {
            history,
            lookback_seconds,
            epoch_unix_ms: 0,
            kept: Mutex::default(),
        }
    }

    /// The same, on the network whose first milestone's epoch is the text `epoch`: the window
    /// starts at the UTC midnight of the tip's date, which depends on it. Text that is not an
    /// epoch changes nothing.
    #[must_use]
    pub fn with_epoch(mut self, epoch: &str) -> StoreProductivity {
        if let Some(epoch_unix_ms) = slots::epoch_unix_ms(epoch) {
            self.epoch_unix_ms = epoch_unix_ms;
        }
        self
    }

    fn kept(&self) -> MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The first timestamp of the window that ends at a tip stamped `tip_timestamp`, in seconds since
/// the network epoch at `epoch_unix_ms`: the UTC midnight of the tip's date, less `lookback`
/// seconds, as the reference's `updateProductivity` computes it (a fractional start, from an epoch
/// with milliseconds, counts from the next whole second).
pub fn window_start(tip_timestamp: u32, epoch_unix_ms: i64, lookback: i64) -> i64 {
    let unix = (i64::from(tip_timestamp) * 1000 + epoch_unix_ms).div_euclid(1000);
    let midnight_ms = unix.div_euclid(DAY) * DAY * 1000;
    (midnight_ms - epoch_unix_ms).div_euclid(1000)
        + i64::from((midnight_ms - epoch_unix_ms).rem_euclid(1000) > 0)
        - lookback
}

/// The table the reference's wallets would hold: a delegate with neither a produced nor a missed
/// block in the window has none, and one with only misses has 0.00.
fn table_of(reader: &Reader<'_>, since: i64) -> Result<ProductivityTable, DatabaseError> {
    let mut by_username = IndexMap::new();
    for row in reader.productivity(since)? {
        let production = match hundredths(row.produced.unwrap_or(0), row.missed.unwrap_or(0)) {
            Some(value) => DelegateProduction {
                missed: Some(u32::try_from(row.missed.unwrap_or(0)).unwrap_or(u32::MAX)),
                productivity: Some(Hundredths(u32::try_from(value).unwrap_or(u32::MAX))),
            },
            None => DelegateProduction {
                missed: None,
                productivity: None,
            },
        };
        by_username.insert(row.username, production);
    }
    Ok(ProductivityTable { by_username })
}

impl Productivity for StoreProductivity {
    fn table(&self, tip: &Block) -> Result<Arc<ProductivityTable>, ApiError> {
        let mut kept = self.kept();
        if let Some(cached) = kept.table.as_ref().filter(|cached| cached.id == *tip.id()) {
            return Ok(Arc::clone(&cached.table));
        }
        let since = window_start(
            tip.header().timestamp,
            self.epoch_unix_ms,
            self.lookback_seconds,
        );
        match table_of(&self.history.get(), since) {
            Ok(table) => {
                let table = Arc::new(table);
                kept.last = Some(Arc::clone(&table));
                kept.table = Some(Cached {
                    id: *tip.id(),
                    table: Arc::clone(&table),
                });
                Ok(table)
            }
            Err(error) => {
                // The reference swallows the failure and keeps the attributes it set before;
                // the next request tries again.
                heartwood_kernel::warning!("The productivity table could not be computed: {error}");
                Ok(kept.last.clone().unwrap_or_default())
            }
        }
    }

    fn totals(&self, tip: &Block) -> Result<ChainTotals, ApiError> {
        let mut kept = self.kept();
        let height = tip.height();
        let reader = self.history.get();
        let summed = match kept.totals.take() {
            Some(summed) if summed.id == *tip.id() => Ok(summed),
            Some(summed) if summed.height < height && is_ancestor(&reader, &summed) => {
                grow(&reader, summed, tip)
            }
            _ => start(&reader, tip),
        };
        match summed {
            Ok(summed) => {
                let totals = totals_of(&summed);
                kept.totals = Some(summed);
                Ok(totals)
            }
            Err(error) => {
                heartwood_kernel::error!("The chain's totals could not be read: {error}");
                Err(ApiError::internal())
            }
        }
    }
}

/// Whether the stored block at the height of `summed` is the one it was summed at, so that the
/// chain has only grown since.
fn is_ancestor(reader: &Reader<'_>, summed: &Summed) -> bool {
    reader
        .blocks_by_height(&[summed.height])
        .is_ok_and(|rows| rows.first().is_some_and(|row| row.id == summed.id))
}

/// The sums up to `tip` from nothing.
fn start(reader: &Reader<'_>, tip: &Block) -> Result<Summed, DatabaseError> {
    let mut summed = Summed {
        id: *tip.id(),
        height: 0,
        fees: 0,
        burns: 0,
        rewards: 0,
        issued: 0,
    };
    summed.issued = reader.issued()?;
    add(reader, &mut summed, tip)?;
    Ok(summed)
}

/// `summed` grown by the blocks after its height, up to `tip`.
fn grow(reader: &Reader<'_>, mut summed: Summed, tip: &Block) -> Result<Summed, DatabaseError> {
    add(reader, &mut summed, tip)?;
    Ok(summed)
}

/// Add the blocks above the height of `summed` up to `tip` to `summed`.
fn add(reader: &Reader<'_>, summed: &mut Summed, tip: &Block) -> Result<(), DatabaseError> {
    let (above, to) = (summed.height, tip.height());
    let burned = reader.burned_totals(above, to)?;
    let rewards = reader.rewards(above, to)?;
    summed.fees = summed.fees.saturating_add(burned.fees);
    summed.burns = summed.burns.saturating_add(burned.transactions);
    summed.rewards = summed.rewards.saturating_add(rewards);
    summed.height = to;
    summed.id = *tip.id();
    Ok(())
}

/// The totals the sums give: the supply is what the genesis block issued and the blocks rewarded,
/// less what they burned.
fn totals_of(summed: &Summed) -> ChainTotals {
    let signed = |sum: u128| i128::try_from(sum).unwrap_or(i128::MAX);
    ChainTotals {
        burned_fees: summed.fees,
        burned_transactions: summed.burns,
        supply: signed(summed.issued)
            .saturating_add(signed(summed.rewards))
            .saturating_sub(signed(summed.fees))
            .saturating_sub(signed(summed.burns)),
    }
}
