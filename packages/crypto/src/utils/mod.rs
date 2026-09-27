//! Helpers shared by the codecs and the rules.

pub mod amounts;
pub mod base58;
pub mod burn;
pub mod bytes;
pub mod donations;
pub mod hex;
pub mod js;
pub mod reward_calculator;
pub mod sort_votes;

/// The reference's `formatSatoshi`: `amount` in base units as the token's amount,
/// `(amount / 10^8).toLocaleString("en", {minimumFractionDigits: 0, maximumFractionDigits: 8})`,
/// then a space and `symbol`. The amount is first the double that JavaScript's `Number` gives
/// for its decimal text; for example `format_satoshi(150_000_000, "dRT")` is `"1.5 dRT"`.
pub fn format_satoshi(amount: i128, symbol: &str) -> String {
    let _ = (amount, symbol);
    todo!("formatSatoshi")
}
