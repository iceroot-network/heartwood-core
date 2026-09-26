//! The fee burn.

use crate::managers::Params;
use crate::utils::donations::share;

/// The part of `fee` that is burned under the merged milestone `params`:
/// `floor(fee × burn.feeBasisPoints / 10000)`.
pub fn burned_fee(fee: u64, params: &Params) -> u64 {
    share(fee, params.fee_basis_points())
}
