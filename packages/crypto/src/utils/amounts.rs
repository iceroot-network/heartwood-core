//! Amounts in base units.
//!
//! Amounts are `u64` with 8 decimals, as in the reference. JSON carries them as decimal strings;
//! every sum is checked.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::AmountError;

/// Base units in one coin.
pub const SATOSHI: u64 = 100_000_000;

/// The amount that the decimal `text` of a JSON amount field holds.
pub fn parse_amount(text: &str) -> Result<u64, AmountError> {
    todo!()
}

/// Decimal text of `amount`, as the reference writes amounts into JSON.
pub fn format_amount(amount: u64) -> String {
    todo!()
}

/// The sum of `amounts`, or `None` if it does not fit 64 bits.
pub fn checked_sum(amounts: impl IntoIterator<Item = u64>) -> Option<u64> {
    todo!()
}
