//! The dynamic fee floor: the least fee a transaction may pay under the milestone in force.
//!
//! The reference computes the floor in its transaction handlers. It depends only on the
//! transaction's type, its size in bytes and the milestone, so it lives here as a pure function: a
//! client computes the exact floor before it signs, and `heartwood-transactions` checks every fee
//! against the same function.

use crate::managers::Params;
use crate::transactions::TransactionType;

/// The dynamic fee floor of a transaction of type `kind` that is `size` bytes long, under
/// `params`: 0 for burns and delegate resignations, and 0 when the milestone's dynamic fees are
/// absent or disabled; otherwise `(addonBytes[key] + round(size / 2)) × max(minFee, 1)`, where
/// `key` is the type key of `kind` ([`TransactionType::key`]) and the sum is a JavaScript number
/// addition.
///
/// `size` is the byte length of the whole transaction, signatures included, as it is sent (for a
/// received transaction, as it was decoded).
pub fn minimum_fee(kind: TransactionType, size: usize, params: &Params) -> i128 {
    let Some(fees) = params.dynamic_fees().filter(|fees| fees.enabled()) else {
        return 0;
    };
    if matches!(
        kind,
        TransactionType::Burn | TransactionType::DelegateResignation
    ) {
        return 0;
    }
    // Math.round(size / 2) of an integer size rounds the half up.
    let size = u64::try_from(size).unwrap_or(u64::MAX);
    let half = size.div_ceil(2);
    let bytes =
        as_javascript_number(i128::from(fees.addon(kind.key())).saturating_add(half.into()));
    // Both factors are below 2^54 in magnitude, so the product fits.
    bytes.saturating_mul(fees.min_fee().max(1).into())
}

/// The integer `value` as a JavaScript number holds it, when it is the exact result of adding two
/// JavaScript numbers: unchanged up to `2^53` in magnitude, above that rounded to the nearest
/// double, a tie going to the one with an even significand, as IEEE 754 addition rounds.
fn as_javascript_number(value: i128) -> i128 {
    const SIGNIFICAND_BITS: u32 = 53;
    let magnitude = value.unsigned_abs();
    let width = u128::BITS.saturating_sub(magnitude.leading_zeros());
    let Some(shift) = width
        .checked_sub(SIGNIFICAND_BITS)
        .filter(|shift| *shift > 0)
    else {
        return value;
    };
    let spacing = 1_u128 << shift;
    let below = magnitude & !(spacing.wrapping_sub(1));
    let remainder = magnitude.wrapping_sub(below);
    let half = spacing >> 1;
    let odd = below & spacing != 0;
    let rounded = if remainder > half || (remainder == half && odd) {
        below.saturating_add(spacing)
    } else {
        below
    };
    let rounded = i128::try_from(rounded).unwrap_or(i128::MAX);
    if value < 0 {
        rounded.saturating_neg()
    } else {
        rounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::{Milestones, Network};

    /// The milestones of one height with `fees` as their `dynamicFees` (JSON text).
    fn with_fees(fees: &str) -> Milestones {
        let network = Network {
            name: "devnet".into(),
            message_prefix: String::new(),
            address_character: None,
            bip32_public: 0,
            bip32_private: 0,
            pub_key_hash: 90,
            nethash: "00".repeat(32),
            wif: 0,
            slip44: 1,
            token: String::new(),
            symbol: String::new(),
            explorer: String::new(),
        };
        let text = format!(
            r#"[{{"height":1,"activeDelegates":1,"blockTime":8,"block":{{"version":0}},
            "epoch":"2026-01-01T00:00:00.000Z","burn":{{"feeBasisPoints":0,"txAmount":0}},
            "transfer":{{}},"dynamicFees":{fees}}}]"#
        );
        Milestones::load(&text, &network).unwrap()
    }

    const TABLE: &str = r#"{"enabled":true,"minFee":100,"addonBytes":{"transfer":10,
        "secondSignature":20,"delegateRegistration":1000,"delegateResignation":30,"burn":40,
        "vote":50}}"#;

    #[test]
    fn the_floor_of_each_type() {
        let milestones = with_fees(TABLE);
        let params = milestones.at(1);
        for (kind, addon) in [
            (TransactionType::Transfer, 10),
            (TransactionType::SecondSignature, 20),
            (TransactionType::DelegateRegistration, 1000),
            (TransactionType::Vote, 50),
        ] {
            // The half size is rounded up: 153 and 154 bytes both count 77.
            for size in [153, 154] {
                assert_eq!(
                    minimum_fee(kind, size, params),
                    (addon + 77) * 100,
                    "{kind:?}"
                );
            }
            assert_eq!(minimum_fee(kind, 0, params), addon * 100, "{kind:?}");
        }
        // Burns and resignations have no floor, whatever the table says.
        for kind in [TransactionType::Burn, TransactionType::DelegateResignation] {
            assert_eq!(minimum_fee(kind, 154, params), 0, "{kind:?}");
        }
    }

    #[test]
    fn no_floor_without_an_enabled_table() {
        for fees in [
            "null",
            "{}",
            r#"{"minFee":100,"addonBytes":{"transfer":10}}"#,
            r#"{"enabled":false,"minFee":100,"addonBytes":{"transfer":10}}"#,
        ] {
            let milestones = with_fees(fees);
            assert_eq!(
                minimum_fee(TransactionType::Transfer, 154, milestones.at(1)),
                0,
                "{fees}"
            );
        }
    }

    #[test]
    fn a_byte_price_below_1_is_1_and_a_missing_addon_is_0() {
        for min_fee in ["0", "-5", "1"] {
            let milestones = with_fees(&format!(
                r#"{{"enabled":true,"minFee":{min_fee},"addonBytes":{{"transfer":10}}}}"#
            ));
            assert_eq!(
                minimum_fee(TransactionType::Transfer, 154, milestones.at(1)),
                10 + 77,
                "{min_fee}"
            );
        }
        let milestones = with_fees(r#"{"enabled":true,"minFee":7,"addonBytes":{"vote":10}}"#);
        assert_eq!(
            minimum_fee(TransactionType::Transfer, 154, milestones.at(1)),
            77 * 7
        );
        let milestones =
            with_fees(r#"{"enabled":true,"minFee":7,"addonBytes":{"transfer":-1000}}"#);
        assert_eq!(
            minimum_fee(TransactionType::Transfer, 154, milestones.at(1)),
            (77 - 1000) * 7
        );
    }

    #[test]
    fn the_sum_is_a_javascript_number() {
        // Past 2^53 doubles are 2 apart: an odd sum is a tie, which goes to the neighbour with
        // the even significand. The product is exact.
        let big = 9_007_199_254_740_991_i128;
        let milestones = with_fees(
            r#"{"enabled":true,"minFee":9007199254740991,"addonBytes":{"transfer":9007199254740991}}"#,
        );
        let floor = |size| minimum_fee(TransactionType::Transfer, size, milestones.at(1));
        // 2^53 + 76 is a double.
        assert_eq!(floor(154), (big + 77) * big);
        // 2^53 + 75 rounds up to 2^53 + 76, and 2^53 + 73 down to 2^53 + 72.
        assert_eq!(floor(152), (big + 77) * big);
        assert_eq!(floor(148), (big + 73) * big);
    }

    #[test]
    fn sums_above_2_pow_53_round_as_javascript_numbers() {
        let two_53: i128 = 1 << 53;
        for (exact, number) in [
            (0, 0),
            (77, 77),
            (-23, -23),
            (two_53 - 1, two_53 - 1),
            (two_53, two_53),
            // Doubles are 2 apart here: a tie goes to the even significand.
            (two_53 + 1, two_53),
            (two_53 + 2, two_53 + 2),
            (two_53 + 3, two_53 + 4),
            (two_53 + 5, two_53 + 4),
            (-(two_53 + 3), -(two_53 + 4)),
            // 4 apart from 2^54.
            ((1 << 54) + 2, 1 << 54),
            ((1 << 54) + 3, (1 << 54) + 4),
            ((1 << 54) + 6, (1 << 54) + 8),
        ] {
            assert_eq!(as_javascript_number(exact), number, "{exact}");
        }
    }
}
