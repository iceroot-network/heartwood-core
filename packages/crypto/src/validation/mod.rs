//! The transaction rules that the reference writes as JSON schemas, written by hand.
//!
//! Each rule has the reference's observable semantics: the same values accepted and refused, with
//! the same changes to the data. Like the reference's schema validator, the rules remove vote
//! names that are not delegate names instead of refusing them, and they do so before they check
//! the vote shares; a transaction whose vote asset names only such keys is a valid vote to remove
//! all votes.
//!
//! Vote rules: every share is 1 to 10,000 basis points, and the shares add up to exactly 10,000
//! unless there are none. From JSON, a share must be a multiple of 0.01 as the reference's
//! `multipleOf` keyword decides it before it becomes `round(value × 100)` basis points.

use serde_json::{Map, Value};

use crate::errors::SchemaError;
use crate::managers::Params;
use crate::transactions::TransactionData;
use crate::transactions::types::{Asset, Votes};
use crate::transactions::{TransactionType, codec};
use crate::utils::js;

/// The largest value of the reference's 64-bit number fields (nonce, fee and amounts): `2^63 − 1`.
pub const MAX_AMOUNT: u64 = i64::MAX as u64;

/// The total of a vote's shares in basis points: 10,000 is 100 %.
pub const VOTE_TOTAL_BASIS_POINTS: u32 = 10_000;

/// Which variant of the reference's transaction schema applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The strict schema, which every received transaction passes: a signature is required (and,
    /// in the JSON form, an `id` key).
    Strict,
    /// The schema without those requirements, as the reference's builders use it.
    NonStrict,
    /// The strict schema for a transaction of the configured genesis block: a zero fee passes the
    /// fee minimum and no fee maximum applies, as the reference allows for the transaction ids its
    /// genesis block lists.
    Genesis,
}

impl Mode {
    /// Whether the strict schema applies: [`Mode::Strict`] and [`Mode::Genesis`].
    pub const fn is_strict(self) -> bool {
        match self {
            Mode::Strict | Mode::Genesis => true,
            Mode::NonStrict => false,
        }
    }

    /// Whether the fee limits of a genesis transaction apply: [`Mode::Genesis`] only.
    pub const fn is_genesis(self) -> bool {
        match self {
            Mode::Genesis => true,
            Mode::Strict | Mode::NonStrict => false,
        }
    }
}

/// Apply the rules of the transaction's type to `data` under the merged milestone `params`.
///
/// Vote names that are not delegate names are removed from `data` first, as the reference's
/// schema removes them; `data` is changed even when a later rule refuses it.
pub fn validate_transaction(
    data: &mut TransactionData,
    params: &Params,
    mode: Mode,
) -> Result<(), SchemaError> {
    if let Asset::Vote { votes } = &mut data.asset {
        remove_invalid_names(votes);
    }
    validate_common(data, params, mode)?;
    validate_asset(data, params)
}

/// The rules that every type shares.
fn validate_common(data: &TransactionData, params: &Params, mode: Mode) -> Result<(), SchemaError> {
    if data.kind != data.asset.kind() {
        return Err(SchemaError::invalid("type", "does not match the asset"));
    }
    if !matches!(data.effective_version(), 2 | 3) {
        return Err(SchemaError::invalid("version", "must be 2 or 3"));
    }
    if data.network != params.network() {
        return Err(SchemaError::invalid(
            "network",
            "must be the network byte of the milestone",
        ));
    }
    if data.nonce > MAX_AMOUNT {
        return Err(SchemaError::invalid("nonce", "must be at most 2^63 - 1"));
    }
    let minimum = fee_minimum(data.kind);
    if data.fee < minimum && !(data.fee == 0 && mode.is_genesis()) {
        return Err(SchemaError::invalid("fee", "is below the type's minimum"));
    }
    if data.fee > MAX_AMOUNT && !mode.is_genesis() {
        return Err(SchemaError::invalid("fee", "must be at most 2^63 - 1"));
    }
    if data.sender_public_key.as_bytes().len() != codec::PUBLIC_KEY_SIZE {
        return Err(SchemaError::invalid("senderPublicKey", "must be 33 bytes"));
    }
    if !is_address_text(&data.sender_address(params).to_base58()) {
        return Err(SchemaError::invalid(
            "senderId",
            "must be 34 Base58 characters",
        ));
    }
    if mode.is_strict() && data.signature.is_none() {
        return Err(SchemaError::missing("signature"));
    }
    Ok(())
}

/// The rules of the type's asset.
fn validate_asset(data: &TransactionData, params: &Params) -> Result<(), SchemaError> {
    match &data.asset {
        Asset::Transfer { transfers } => {
            let count = transfers.len();
            if count == 0 {
                return Err(SchemaError::invalid(
                    "asset.transfers",
                    "must have at least one item",
                ));
            }
            let (minimum, maximum) = transfer_limits(params);
            if count < minimum || count > maximum {
                return Err(SchemaError::invalid(
                    "type",
                    "the number of transfer items is outside the milestone's limits",
                ));
            }
            for (index, item) in transfers.iter().enumerate() {
                if item.amount == 0 || item.amount > MAX_AMOUNT {
                    return Err(SchemaError::invalid(
                        format!("asset.transfers[{index}].amount"),
                        "must be 1 to 2^63 - 1",
                    ));
                }
                if !is_address_text(&item.recipient.to_base58()) {
                    return Err(SchemaError::invalid(
                        format!("asset.transfers[{index}].recipientId"),
                        "must be 34 Base58 characters",
                    ));
                }
            }
            Ok(())
        }
        Asset::SecondSignature { .. } => {
            if data.second_signature.is_some() {
                return Err(SchemaError::invalid(
                    "secondSignature",
                    "a second-signature registration has no second signature",
                ));
            }
            Ok(())
        }
        Asset::DelegateRegistration { username } => validate_username(username),
        Asset::DelegateResignation { .. } => Ok(()),
        Asset::Burn { amount } => {
            if *amount > MAX_AMOUNT {
                return Err(SchemaError::invalid("amount", "must be at most 2^63 - 1"));
            }
            Ok(())
        }
        Asset::Vote { votes } => validate_votes(votes),
    }
}

/// The smallest fee of `kind`: 1 base unit, or 0 for a resignation and a burn.
fn fee_minimum(kind: TransactionType) -> u64 {
    match kind {
        TransactionType::Transfer
        | TransactionType::SecondSignature
        | TransactionType::DelegateRegistration
        | TransactionType::Vote => 1,
        TransactionType::DelegateResignation | TransactionType::Burn => 0,
    }
}

/// The fewest and most items a transfer may have: the milestone's `transfer.minimum` and
/// `transfer.maximum`, where 0 means the reference's defaults, 1 and 256.
pub(crate) fn transfer_limits(params: &Params) -> (usize, usize) {
    let minimum = match params.transfer_minimum() {
        0 => 1,
        value => usize::try_from(value).unwrap_or(usize::MAX),
    };
    let maximum = match params.transfer_maximum() {
        0 => 256,
        value => usize::try_from(value).unwrap_or(usize::MAX),
    };
    (minimum, maximum)
}

/// Whether the Base58Check `text` of an address has the 34 characters the reference's schema
/// requires. A 21-byte payload has 34 characters for most network bytes, including 90, but not
/// for all.
fn is_address_text(text: &str) -> bool {
    text.len() == 34
}

/// Whether `name` is a delegate name: 1 to 20 characters of `a-z`, `0-9` and `!@$&_.`, not
/// starting with `_`, and not only digits.
pub fn is_delegate_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 20
        && !name.starts_with('_')
        && name.bytes().all(is_name_byte)
        && name.bytes().any(|byte| !byte.is_ascii_digit())
}

/// Whether `byte` may appear in a delegate name.
fn is_name_byte(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'!' | b'@' | b'$' | b'&' | b'_' | b'.')
}

/// Whether `username` is a valid delegate name (see [`is_delegate_name`]).
pub fn validate_username(username: &str) -> Result<(), SchemaError> {
    if is_delegate_name(username) {
        Ok(())
    } else {
        Err(SchemaError::invalid(
            "asset.delegate.username",
            "must be 1 to 20 of a-z, 0-9 and !@$&_., not starting with _ nor only digits",
        ))
    }
}

/// Remove the names that are not delegate names, as the reference's vote schema does.
pub fn remove_invalid_names(votes: &mut Votes) {
    votes.retain(|name, _| is_delegate_name(name));
}

/// Whether `votes` is a valid vote asset: names that are delegate names, shares of 1 to 10,000
/// basis points that add up to exactly 10,000, or no votes at all.
pub fn validate_votes(votes: &Votes) -> Result<(), SchemaError> {
    let mut total: u32 = 0;
    for (name, &basis_points) in votes {
        if !is_delegate_name(name) {
            return Err(SchemaError::invalid(
                format!("asset.votes['{name}']"),
                "is not a delegate name",
            ));
        }
        if basis_points == 0 {
            return Err(SchemaError::invalid(
                format!("asset.votes['{name}']"),
                "must be at least 0.01",
            ));
        }
        if u32::from(basis_points) > VOTE_TOTAL_BASIS_POINTS {
            return Err(SchemaError::invalid(
                format!("asset.votes['{name}']"),
                "must be at most 100",
            ));
        }
        total += u32::from(basis_points);
    }
    if votes.is_empty() || total == VOTE_TOTAL_BASIS_POINTS {
        Ok(())
    } else {
        Err(SchemaError::VoteSum {
            basis_points: total,
        })
    }
}

/// The basis points of the vote percentage `percent` read from JSON: refused unless it is a
/// multiple of 0.01 in the sense of the reference's `multipleOf` keyword (see
/// [`is_multiple_of_cent`]) from 0.01 to 100, then `round(percent × 100)`.
pub fn vote_basis_points(percent: f64) -> Result<u16, SchemaError> {
    if !is_multiple_of_cent(percent) {
        return Err(SchemaError::invalid(
            "asset.votes",
            "must be a multiple of 0.01",
        ));
    }
    if percent < 0.01 {
        return Err(SchemaError::invalid("asset.votes", "must be at least 0.01"));
    }
    if percent > 100.0 {
        return Err(SchemaError::invalid("asset.votes", "must be at most 100"));
    }
    // 0.01 <= percent <= 100, so the rounded value is an integer from 1 to 10,000.
    Ok((percent * 100.0).round() as u16)
}

/// Whether `value` is a multiple of 0.01 as the reference's `multipleOf` keyword decides it.
///
/// The keyword scales both numbers by `10^k`, where `k` is the larger of 2 (the decimals of 0.01)
/// and the number of characters after the point in `value`'s `Number.prototype.toString` text,
/// rounds both with `Math.round`, and requires the remainder of the division to be 0. The
/// arithmetic is double arithmetic, so a value a few units in the last place away from a cent,
/// such as `0.19999999999999998` or `39.49999999999999`, can pass: its scaled value rounds to a
/// multiple of the scaled 0.01.
pub fn is_multiple_of_cent(value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    let text = js::number_to_string(value);
    let decimals = text
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let exponent = i32::try_from(decimals.max(2)).unwrap_or(i32::MAX);
    let multiplier = 10f64.powi(exponent);
    js_round(value * multiplier) % js_round(0.01 * multiplier) == 0.0
}

/// JavaScript's `Math.round`: the nearest integer, ties toward positive infinity.
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// The votes of a JSON vote object, as the reference's vote schema leaves them: names that are
/// not delegate names removed, every other share a number that [`vote_basis_points`] accepts, and
/// the shares adding up to 10,000 basis points unless there are none.
pub fn votes_from_json(votes: &Map<String, Value>) -> Result<Votes, SchemaError> {
    let mut out = Votes::new();
    for (name, value) in votes {
        if !is_delegate_name(name) {
            continue;
        }
        let percent = value.as_f64().ok_or_else(|| {
            SchemaError::invalid(format!("asset.votes['{name}']"), "must be a number")
        })?;
        let basis_points = vote_basis_points(percent).map_err(|error| match error {
            SchemaError::Invalid { reason, .. } => {
                SchemaError::invalid(format!("asset.votes['{name}']"), reason)
            }
            other => other,
        })?;
        out.insert(name.clone(), basis_points);
    }
    validate_votes(&out)?;
    Ok(out)
}

/// The integer that the reference's `bignumber` schema keyword makes of a JSON value, or `None`
/// when the keyword refuses the value.
///
/// A string is read as JavaScript's `BigInt` reads text: surrounding white space is ignored, an
/// empty text is 0, and a decimal integer may have a sign while `0x`, `0o` and `0b` prefixes
/// select another base. A number must be an integer. `true` is 1. `false`, `null`, the empty
/// string, arrays and objects are refused. Values outside the `i128` range are clamped, which
/// keeps them outside every range the rules allow.
pub(crate) fn bignumber(value: &Value) -> Option<i128> {
    match value {
        Value::Null | Value::Bool(false) => None,
        Value::String(text) if text.is_empty() => None,
        _ => bigint(value),
    }
}

/// JavaScript's `BigInt(value)` for a JSON value, clamped to the `i128` range: text as described
/// for [`bignumber`] (the empty text is 0), an integer number, or a boolean. `null`, arrays and
/// objects are refused.
pub(crate) fn bigint(value: &Value) -> Option<i128> {
    match value {
        Value::String(text) => bigint_from_text(text),
        Value::Number(number) => {
            let double = number.as_f64()?;
            if !double.is_finite() || double.fract() != 0.0 {
                return None;
            }
            // Saturating conversion; every value the rules accept is well inside the range.
            Some(double as i128)
        }
        Value::Bool(flag) => Some(i128::from(*flag)),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// JavaScript's `BigInt(text)`, clamped to the `i128` range.
fn bigint_from_text(text: &str) -> Option<i128> {
    let trimmed = text.trim_matches(is_js_white_space);
    if trimmed.is_empty() {
        return Some(0);
    }
    let (radix, digits, negative) = match trimmed.get(..2) {
        Some("0x" | "0X") => (16, trimmed.get(2..)?, false),
        Some("0o" | "0O") => (8, trimmed.get(2..)?, false),
        Some("0b" | "0B") => (2, trimmed.get(2..)?, false),
        _ => match trimmed.strip_prefix('-') {
            Some(rest) => (10, rest, true),
            None => (10, trimmed.strip_prefix('+').unwrap_or(trimmed), false),
        },
    };
    if digits.is_empty() {
        return None;
    }
    let mut magnitude: i128 = 0;
    for character in digits.chars() {
        let digit = character.to_digit(radix)?;
        magnitude = magnitude
            .saturating_mul(i128::from(radix))
            .saturating_add(i128::from(digit));
    }
    Some(if negative { -magnitude } else { magnitude })
}

/// JavaScript's white space and line terminators, which `BigInt` ignores around a number.
fn is_js_white_space(character: char) -> bool {
    matches!(
        character,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    #[test]
    fn delegate_names() {
        for name in [
            "a",
            "delegate_1",
            "a.b",
            "a!b",
            "1a",
            "a1",
            "abcdefghijklmnopqrst",
            "._",
        ] {
            assert!(is_delegate_name(name), "{name:?}");
        }
        for name in [
            "",
            "_a",
            "A",
            "123",
            "abcdefghijklmnopqrstu",
            "a b",
            "é",
            "a-b",
            "\u{fffd}",
        ] {
            assert!(!is_delegate_name(name), "{name:?}");
        }
    }

    #[test]
    fn vote_shares_from_json() {
        assert_eq!(vote_basis_points(100.0), Ok(10_000));
        assert_eq!(vote_basis_points(0.01), Ok(1));
        assert_eq!(vote_basis_points(18.1), Ok(1810));
        assert_eq!(vote_basis_points(33.33), Ok(3333));
        for refused in [
            0.0,
            -0.0,
            0.005,
            100.01,
            99.995,
            1.001,
            f64::NAN,
            f64::INFINITY,
            -1.0,
        ] {
            assert!(vote_basis_points(refused).is_err(), "{refused}");
        }
        // 0.1 + 0.2 is not a multiple of 0.01 for the reference's rule.
        assert!(vote_basis_points(0.1 + 0.2).is_err());
        // Shares a few units in the last place from a cent that the reference's rule accepts:
        // their scaled value rounds to a multiple of the scaled 0.01.
        assert_eq!(vote_basis_points(0.19999999999999998), Ok(20));
        assert_eq!(vote_basis_points(0.9400000000000001), Ok(94));
        assert_eq!(vote_basis_points(10.040000000000001), Ok(1004));
        assert_eq!(vote_basis_points(39.49999999999999), Ok(3950));
        assert!(vote_basis_points(50.00000000000001).is_err());
        assert!(vote_basis_points(99.99999999999999).is_err());
    }

    #[test]
    fn multiples_of_a_cent_follow_the_reference_rule() {
        // Every exact cent, and the doubles on either side of 0.01 and 100.
        for cent in 1..=10_000u32 {
            assert!(is_multiple_of_cent(f64::from(cent) / 100.0), "{cent}");
        }
        assert!(!is_multiple_of_cent(0.009_999_999_999_999_998));
        assert!(!is_multiple_of_cent(f64::NAN));
        assert!(!is_multiple_of_cent(f64::INFINITY));
        // The scaled value uses the decimals of the shortest text: 1e-7 has none, so it scales by
        // 100 and rounds to 0, a multiple.
        assert!(is_multiple_of_cent(1e-7));
        assert!(vote_basis_points(1e-7).is_err());
        assert_eq!(js_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
    }

    #[test]
    fn vote_objects() {
        let object = |value: Value| value.as_object().cloned().unwrap();
        let votes = votes_from_json(&object(json!({"A": 50, "b": 100}))).unwrap();
        assert_eq!(votes.keys().collect::<Vec<_>>(), ["b"]);
        assert_eq!(
            votes_from_json(&object(json!({"A": 100}))).unwrap(),
            Votes::new()
        );
        assert!(votes_from_json(&object(json!({"a": 50, "b": 49.99}))).is_err());
        assert!(votes_from_json(&object(json!({"a": "100"}))).is_err());
        assert!(votes_from_json(&object(json!({"a": null}))).is_err());
        assert_eq!(
            votes_from_json(&object(json!({"a": 18.1, "b": 81.9}))).unwrap(),
            [("a".to_owned(), 1810), ("b".to_owned(), 8190)]
                .into_iter()
                .collect::<Votes>()
        );
    }

    #[test]
    fn javascript_bigint_text() {
        assert_eq!(bignumber(&json!("12")), Some(12));
        assert_eq!(bignumber(&json!(" 12\n")), Some(12));
        assert_eq!(bignumber(&json!("-5")), Some(-5));
        assert_eq!(bignumber(&json!("+5")), Some(5));
        assert_eq!(bignumber(&json!("0x10")), Some(16));
        assert_eq!(bignumber(&json!("0b101")), Some(5));
        assert_eq!(bignumber(&json!("   ")), Some(0));
        assert_eq!(bignumber(&json!(12)), Some(12));
        assert_eq!(bignumber(&json!(12.0)), Some(12));
        assert_eq!(bignumber(&json!(true)), Some(1));
        for refused in [
            json!(""),
            json!("1.5"),
            json!("1e3"),
            json!("-0x10"),
            json!("0x"),
            json!("1_000"),
            json!(1.5),
            json!(false),
            json!(null),
            json!([]),
            json!({}),
        ] {
            assert_eq!(bignumber(&refused), None, "{refused}");
        }
        assert_eq!(
            bignumber(&json!("99999999999999999999999999999999999999999999")),
            Some(i128::MAX)
        );
    }

    proptest! {
        #[test]
        fn every_cent_is_its_basis_points(basis_points in 1u16..=10_000) {
            let percent = f64::from(basis_points) / 100.0;
            prop_assert_eq!(vote_basis_points(percent), Ok(basis_points));
            // The text the reference would print reads back to the same share.
            let text = format!("{percent}");
            prop_assert_eq!(vote_basis_points(text.parse().unwrap()), Ok(basis_points));
        }

        #[test]
        fn an_accepted_share_near_a_cent_is_that_cent(cent in 1u32..=10_000, step in -8i64..=8) {
            let exact = f64::from(cent) / 100.0;
            let share = f64::from_bits(exact.to_bits().checked_add_signed(step).unwrap());
            if let Ok(basis_points) = vote_basis_points(share) {
                prop_assert_eq!(u32::from(basis_points), cent);
            }
        }

        #[test]
        fn two_way_splits_are_valid(a in 1u16..10_000) {
            let votes: Votes = [("a".to_owned(), a), ("b".to_owned(), 10_000 - a)]
                .into_iter()
                .collect();
            prop_assert_eq!(validate_votes(&votes), Ok(()));
        }
    }
}
