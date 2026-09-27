//! The fee rules to enter the pool and to be broadcast.
//!
//! When the milestone of the tip plus one has dynamic fees enabled, a transaction pays at least
//! the handler's floor with the milestone's fee table. Otherwise the pool's own
//! [`crate::PoolFees`] apply: when enabled, the floor with `minFeePool` (to enter) or
//! `minFeeBroadcast` (to be broadcast) and the pool's `addonBytes`; when disabled, the fee must be
//! the milestone's static fee exactly (`ERR_LOW_FEE` below it, `ERR_HIGH_FEE` above it).

use std::cmp::Ordering;

use heartwood_crypto::Transaction;
use heartwood_crypto::transactions::builders::static_fee;
use heartwood_transactions::{minimum_fee, minimum_fee_with};

use crate::errors::{PoolCode, Refusal};
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
    check(ctx, options, transaction, options.dynamic_fees.min_fee_pool)
}

/// Refuse `transaction` when its fee does not let it be broadcast.
#[doc(alias = "throwIfCannotBroadcast")]
pub fn check_broadcast(
    ctx: &PoolContext<'_>,
    options: &Options,
    transaction: &Transaction,
) -> Result<(), Refusal> {
    check(
        ctx,
        options,
        transaction,
        options.dynamic_fees.min_fee_broadcast,
    )
}

/// The fee rule, with `pool_min_fee` as the price of a byte when the pool's own floor applies.
fn check(
    ctx: &PoolContext<'_>,
    options: &Options,
    transaction: &Transaction,
    pool_min_fee: u64,
) -> Result<(), Refusal> {
    let params = ctx.params();
    let pool_fees = &options.dynamic_fees;
    let floor = if params.dynamic_fees().is_some_and(|fees| fees.enabled()) {
        Some(minimum_fee(transaction, params))
    } else if pool_fees.enabled {
        let addon = pool_fees
            .addon_bytes
            .get(transaction.key())
            .copied()
            .unwrap_or(0);
        Some(minimum_fee_with(
            transaction,
            i64::try_from(pool_min_fee).unwrap_or(i64::MAX),
            i64::try_from(addon).unwrap_or(i64::MAX),
        ))
    } else {
        None
    };
    let text = || transaction.display(ctx.network.pub_key_hash);
    let too_low = || {
        Refusal::new(
            PoolCode::LowFee,
            format!("{} fee is too low to enter the pool", text()),
        )
    };
    match floor {
        Some(floor) if i128::from(transaction.fee()) >= floor => Ok(()),
        Some(_) => Err(too_low()),
        None => {
            let fee = static_fee(transaction.kind(), params)
                .map_err(|error| Refusal::new(PoolCode::Other, error.to_string()))?;
            match transaction.fee().cmp(&fee) {
                Ordering::Equal => Ok(()),
                Ordering::Less => Err(too_low()),
                Ordering::Greater => Err(Refusal::new(
                    PoolCode::HighFee,
                    format!("{} fee is too high to enter the pool", text()),
                )),
            }
        }
    }
}
