//! Helpers shared by the codecs and the rules.

pub mod amounts;
pub mod base58;
pub mod burn;
pub mod bytes;
pub mod donations;
pub mod fee_floor;
pub mod hex;
pub mod js;
pub mod reward_calculator;
pub mod sort_votes;

/// The reference's `formatSatoshi`: `amount` in base units as the token's amount,
/// `(amount / 10^8).toLocaleString("en", {minimumFractionDigits: 0, maximumFractionDigits: 8})`,
/// then a space and `symbol`. The amount is first the double that JavaScript's `Number` gives
/// for its decimal text, divided as a double; for example `format_satoshi(150_000_000, "dRT")` is
/// `"1.5 dRT"` and `format_satoshi(-50, "dRT")` is `"-0.0000005 dRT"`.
pub fn format_satoshi(amount: i128, symbol: &str) -> String {
    // The conversion rounds to the nearest double, a tie to even, as `Number` reads decimal text.
    format!("{} {symbol}", js::to_locale_fixed(amount as f64 / 1e8, 8))
}
