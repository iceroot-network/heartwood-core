//! The response shapes of the served routes, as the reference's resources write them, keys in the
//! reference's order.

use heartwood_crypto::{BlockId, Milestones, Transaction};
use heartwood_database::BlockRow;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_pool::PoolView;
use heartwood_state::{StateView, Wallet};
use serde_json::Value;

/// Where a confirmed transaction is, as its resource shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionBlock {
    /// The block's id.
    pub id: BlockId,
    /// The block's height.
    pub height: u32,
    /// The block's timestamp; `None` leaves `timestamp` out (a block's transactions route).
    pub timestamp: Option<u32>,
    /// The confirmations shown.
    pub confirmations: u32,
}

/// A block, from its stored row, with the tip's height for its confirmations.
pub fn block(row: &BlockRow, tip: u32, milestones: &Milestones) -> Value {
    let _ = (row, tip, milestones);
    todo!()
}

/// A transaction, confirmed in `block` or in the pool (`None`).
pub fn transaction(
    transaction: &Transaction,
    block: Option<&TransactionBlock>,
    milestones: &Milestones,
) -> Value {
    let _ = (transaction, block, milestones);
    todo!()
}

/// A wallet, as the reference's basic wallet shows it; the delegate's version comes from the
/// online record of `pool`.
pub fn wallet(wallet: &Wallet, state: &StateView, pool: &PoolView) -> Value {
    let _ = (wallet, state, pool);
    todo!()
}

/// A peer.
pub fn peer(peer: &PeerInfo) -> Value {
    let _ = peer;
    todo!()
}

/// An epoch timestamp as `{epoch, unix, human}`.
pub fn timestamp(epoch_stamp: u32, milestones: &Milestones) -> Value {
    let _ = (epoch_stamp, milestones);
    todo!()
}
