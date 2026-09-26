//! The fee burn.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::managers::Params;

/// The part of `fee` that is burned under the merged milestone `params`:
/// `floor(fee × burn.feeBasisPoints / 10000)`.
pub fn burned_fee(fee: u64, params: &Params) -> u64 {
    todo!()
}
