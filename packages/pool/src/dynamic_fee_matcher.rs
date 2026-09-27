//! The fee rules to enter the pool and to be broadcast.
//!
//! When the milestone of the tip plus one has dynamic fees enabled, a transaction pays at least
//! the handler's floor with the milestone's fee table. Otherwise the pool's own
//! [`crate::PoolFees`] apply: when enabled, the floor with `minFeePool` (to enter) or
//! `minFeeBroadcast` (to be broadcast) and the pool's `addonBytes`; when disabled, the fee must be
//! the milestone's static fee exactly (`ERR_LOW_FEE` below it, `ERR_HIGH_FEE` above it).

use heartwood_crypto::Transaction;

use crate::errors::Refusal;
use crate::options::Options;
use crate::service::PoolContext;

/// Refuse `transaction` with `ERR_LOW_FEE` or `ERR_HIGH_FEE` when its fee does not let it enter
/// the pool.
#[doc(alias = "throwIfCannotEnterPool")]
pub fn check_enter_pool(
    ctx: &PoolContext<'_>,
    options: &Options,
    transaction: &Transaction,
) -> Result<(), Refusal> {
    let _ = (ctx, options, transaction);
    todo!("the fee rule to enter the pool")
}

/// Refuse `transaction` when its fee does not let it be broadcast.
#[doc(alias = "throwIfCannotBroadcast")]
pub fn check_broadcast(
    ctx: &PoolContext<'_>,
    options: &Options,
    transaction: &Transaction,
) -> Result<(), Refusal> {
    let _ = (ctx, options, transaction);
    todo!("the fee rule to be broadcast")
}
