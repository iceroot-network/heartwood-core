//! The cases of the message formats: amounts over the whole `i64` and `u64` range, weighted to
//! `2^53` and to amounts whose double lies at a tie of the ninth fraction digit; token symbols;
//! counts; and transactions of the six kept types.

use std::sync::LazyLock;

use heartwood_crypto::identities::KeyPair;
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::transactions::types::ResignationType;
use heartwood_crypto::utils::{hex, js};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use crate::strategies::kernel::OpCase;
use crate::strategies::transactions::TxSpec;
use crate::strategies::{NETWORK, address_of, keys, split_now};

/// The rule height of the cases.
const HEIGHT: u64 = 2;

/// Offsets in a transaction with the standard header: the version byte and the network byte.
const VERSION_AT: usize = 1;
const NETWORK_AT: usize = 2;

/// The rules the cases' transactions are built under: the devnet's network byte, with no fee
/// floor; the format reads nothing else.
static RULES: LazyLock<Option<Milestones>> = LazyLock::new(|| {
    let network = Network::load(&format!(
        r#"{{"name":"devnet","messagePrefix":"m","bip32":{{"public":1,"private":2}},"pubKeyHash":{NETWORK},
        "nethash":"{}","wif":252,"slip44":1,"client":{{"token":"dROOT","symbol":"dRT","explorer":""}}}}"#,
        "00".repeat(32)
    ))
    .ok()?;
    let milestones = r#"[{"height":1,"activeDelegates":53,"blockTime":8,"epoch":"2026-01-01T00:00:00.000Z",
        "block":{"version":0},"burn":{"feeBasisPoints":9000,"txAmount":1},"transfer":{},"reward":0}]"#;
    Milestones::load(milestones, &network).ok()
});

/// Amounts over the whole `i64` and `u64` range, weighted to `2^53` (where an amount stops being
/// its own double), and their negatives.
fn amounts() -> impl Strategy<Value = i128> {
    let two_53 = 1i128 << 53;
    let magnitude = prop_oneof![
        2 => any::<i64>().prop_map(i128::from),
        2 => any::<u64>().prop_map(i128::from),
        3 => (-1_000_000i128..=1_000_000).prop_map(move |offset| two_53 + offset),
        2 => ((1u64 << 53)..=u64::MAX).prop_map(i128::from),
        2 => 0i128..=1_000_000_000_000,
        1 => (0u32..=20, -5i128..=5).prop_map(|(power, offset)| 10i128.pow(power) + offset),
    ];
    (magnitude, prop::bool::weighted(0.2)).prop_map(
        |(amount, negative)| {
            if negative { -amount } else { amount }
        },
    )
}

/// Numbers for `toLocaleString` with 0 to 8 fraction digits: ties of the rounding at the digit
/// count (a decimal ending in 5 one digit further), their neighbouring doubles, and any double.
fn fixed() -> impl Strategy<Value = (f64, u8)> {
    let tie = (0u8..=8, 0u64..1_000_000_000).prop_map(|(digits, mantissa)| {
        // `mantissa` followed by a 5 at the first dropped digit.
        let text = format!("{mantissa}5e-{}", u32::from(digits) + 1);
        (text.parse::<f64>().unwrap_or(0.0), digits)
    });
    let beside = (tie.clone(), any::<bool>()).prop_map(|((value, digits), up)| {
        let bits = value.to_bits();
        (
            f64::from_bits(if up {
                bits.saturating_add(1)
            } else {
                bits.saturating_sub(1)
            }),
            digits,
        )
    });
    let any_double =
        (any::<u64>(), 0u8..=8).prop_map(|(bits, digits)| (f64::from_bits(bits), digits));
    (
        prop_oneof![4 => tie, 2 => beside, 1 => any_double],
        any::<bool>(),
    )
        .prop_filter("a finite number", |((value, _), _)| value.is_finite())
        // A JSON copy of -0 is 0 on the oracle's side.
        .prop_map(|((value, digits), negative)| {
            (if negative { -value } else { value } + 0.0, digits)
        })
}

/// Token symbols.
fn symbols() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => Just("dRT".to_owned()),
        1 => prop::sample::select(vec!["SXP", "", "ROOT", "₿"]).prop_map(str::to_owned),
        1 => "[A-Za-z ]{0,6}",
    ]
}

/// Counts: small, around the digit groups, and up to `limit` in magnitude.
fn counts(limit: i64) -> impl Strategy<Value = i64> {
    prop_oneof![
        3 => -2i64..=2_000,
        2 => prop::sample::select(vec![999i64, 1_000, 999_999, 1_000_000, 2_000_000]),
        2 => -limit..=limit,
        1 => (-1_000i64..=1_000).prop_map(move |offset| limit.saturating_sub(1_000).saturating_add(offset)),
    ]
}

/// A transaction of a kept type from one of four senders, with any nonce the reference can give
/// an id (below `2^63`), and in a third of the cases another version or network byte.
fn transactions() -> impl Strategy<Value = String> {
    let spec = prop_oneof![
        3 => (1usize..=3, 1u64..=1_000_000_000).prop_map(|(n, amount)| {
            TxSpec::Transfer((0..n).map(|i| (address_of(keys(&format!("l1 text recipient {i}")).public_key()), amount)).collect())
        }),
        1 => Just(TxSpec::SecondSignature("l1 text second".into())),
        1 => "[a-z0-9_]{1,20}".prop_map(TxSpec::DelegateRegistration),
        1 => prop::sample::select(vec![None, Some(ResignationType::PermanentResign), Some(ResignationType::NotResigned)]).prop_map(TxSpec::Resignation),
        1 => (1u64..=100_000_000_000).prop_map(TxSpec::Burn),
        1 => (1usize..=4).prop_map(|n| TxSpec::Vote((1..=n).map(|i| format!("genesis_{i}")).zip(split_now(n)).collect())),
    ];
    let bytes = prop_oneof![
        4 => Just(None),
        1 => prop::sample::select(vec![(VERSION_AT, 0u8), (VERSION_AT, 2), (NETWORK_AT, 30), (NETWORK_AT, 63)]).prop_map(Some),
    ];
    let nonce = prop_oneof![3 => 0u64..=1_000, 1 => 0u64..(1 << 63)];
    (spec, 0usize..4, nonce, bytes).prop_filter_map(
        "the builder refused the transaction",
        |(spec, sender, nonce, change)| {
            let params = RULES.as_ref()?.at(2);
            let sender: KeyPair = keys(&format!("l1 text sender {sender}"));
            let transaction = spec.build(&sender, None, nonce, 100_000_000, params)?;
            let mut bytes = transaction.bytes().to_vec();
            if let Some((at, value)) = change {
                *bytes.get_mut(at)? = value;
            }
            Some(hex::encode(&bytes))
        },
    )
}

/// The cases of the `text.*` operations.
pub fn text_cases() -> BoxedStrategy<OpCase> {
    let words = prop_oneof![
        3 => prop::sample::select(vec!["second", "byte", "transaction", "more block", "vote", ""]).prop_map(str::to_owned),
        1 => "[a-z ]{0,12}",
    ];
    let safe = i64::try_from(js::MAX_SAFE_INTEGER).unwrap_or(i64::MAX);
    prop_oneof![
        4 => (amounts(), symbols()).prop_map(|(amount, symbol)| {
            ("text.formatSatoshi", HEIGHT, json!({"amount": amount.to_string(), "symbol": symbol}))
        }),
        3 => fixed().prop_map(|(value, digits)| {
            ("text.toLocaleFixed", HEIGHT, json!({"value": value, "digits": digits}))
        }),
        2 => (words, counts(safe), any::<bool>()).prop_map(|(word, count, inclusive)| {
            ("text.pluralise", HEIGHT, json!({"word": word, "count": count, "inclusive": inclusive}))
        }),
        2 => counts(i64::MAX).prop_map(|count| ("text.toLocaleString", HEIGHT, json!({"count": count}))),
        2 => transactions().prop_map(|hex| ("text.transaction", HEIGHT, json!({"hex": hex}))),
    ]
    .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    use super::*;

    #[test]
    fn the_rules_load() {
        assert!(RULES.is_some());
    }

    #[test]
    fn every_operation_is_drawn() {
        let mut runner = TestRunner::deterministic();
        let strategy = text_cases();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..400 {
            seen.insert(strategy.new_tree(&mut runner).unwrap().current().0);
        }
        assert_eq!(seen.len(), 5, "{seen:?}");
    }
}
