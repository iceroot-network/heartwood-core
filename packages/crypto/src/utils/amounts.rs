//! Amounts in base units.
//!
//! Amounts are `u64` with 8 decimals, as in the reference. JSON carries them as decimal strings;
//! every sum is checked.

use crate::errors::AmountError;

/// Base units in one coin.
pub const SATOSHI: u64 = 100_000_000;

/// The amount that the decimal `text` of a JSON amount field holds: one or more ASCII digits,
/// nothing else.
pub fn parse_amount(text: &str) -> Result<u64, AmountError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AmountError::NotAnInteger(text.to_owned()));
    }
    text.parse()
        .map_err(|_| AmountError::OutOfRange(text.to_owned()))
}

/// Decimal text of `amount`, as the reference writes amounts into JSON.
pub fn format_amount(amount: u64) -> String {
    amount.to_string()
}

/// The sum of `amounts`, or `None` if it does not fit 64 bits.
pub fn checked_sum(amounts: impl IntoIterator<Item = u64>) -> Option<u64> {
    amounts
        .into_iter()
        .try_fold(0u64, |sum, amount| sum.checked_add(amount))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn parses_decimal_text_only() {
        assert_eq!(parse_amount("0"), Ok(0));
        assert_eq!(parse_amount("007"), Ok(7));
        assert_eq!(parse_amount("18446744073709551615"), Ok(u64::MAX));
        assert_eq!(
            parse_amount("18446744073709551616"),
            Err(AmountError::OutOfRange("18446744073709551616".into()))
        );
        for text in ["", "-1", "+1", " 1", "1 ", "1.0", "1e3", "0x10", "١"] {
            assert_eq!(
                parse_amount(text),
                Err(AmountError::NotAnInteger(text.into())),
                "{text:?}"
            );
        }
    }

    #[test]
    fn sums_are_checked() {
        assert_eq!(checked_sum([]), Some(0));
        assert_eq!(checked_sum([1, 2, 3]), Some(6));
        assert_eq!(checked_sum([u64::MAX, 0]), Some(u64::MAX));
        assert_eq!(checked_sum([u64::MAX, 1]), None);
        assert_eq!(format_amount(SATOSHI), "100000000");
    }

    proptest! {
        #[test]
        fn format_then_parse(amount in any::<u64>()) {
            prop_assert_eq!(parse_amount(&format_amount(amount)), Ok(amount));
        }
    }
}
