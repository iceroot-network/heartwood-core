//! The reference's formatting in messages: `text.formatSatoshi` (amounts with the token symbol),
//! `text.toLocaleFixed` (the number formatting it calls, on any number), `text.pluralise`
//! (counts), `text.toLocaleString` (integers) and `text.transaction` (a transaction's
//! `toString`).
//!
//! A count written in a default locale that is not English is `Documented::Locale`, which only
//! the oracle's answer shows (`notes.locale`).

use heartwood_crypto::Transaction;
use heartwood_crypto::utils::js::{self, to_locale_integer};
use heartwood_crypto::utils::{format_satoshi, hex};
use heartwood_kernel::utils::pluralise::pluralise;
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, params, text, unclassified};
use crate::subject::{Case, Documented, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("text.formatSatoshi", format_satoshi_text),
    ("text.toLocaleFixed", to_locale_fixed_text),
    ("text.pluralise", pluralise_text),
    ("text.toLocaleString", to_locale_string),
    ("text.transaction", transaction),
];

/// A count as the oracle's JSON holds it: an integer, which JavaScript reads as a double.
fn count(input: &Value) -> Outcome<i128> {
    let number = input["count"]
        .as_number()
        .ok_or_else(|| failed("input.count is not a number"))?;
    let exact = (number.as_i64().map(i128::from)).or_else(|| number.as_u64().map(i128::from));
    let double = number
        .as_f64()
        .filter(|d| d.fract() == 0.0 && d.abs() < 1e38);
    // A double beyond 2^64 is an integer, and within 1e38 it fits i128 exactly.
    exact
        .or_else(|| double.map(|d| d as i128))
        .ok_or_else(|| failed("input.count is not an integer"))
}

/// A count's text, shown different by the oracle when its default locale is not English.
fn in_english(output: Value) -> Verdict {
    Verdict::DiffersWhereShown {
        difference: Documented::Locale,
        verdict: Box::new(Verdict::Output(output)),
    }
}

/// `text.formatSatoshi`: `amount` (signed decimal text) with `symbol`, or the devnet's symbol.
fn format_satoshi_text(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let amount: i128 = text(case.input, "amount")?
        .parse()
        .map_err(|_| failed("input.amount is not a 128-bit integer"))?;
    let symbol = match case.input.get("symbol") {
        Some(_) => text(case.input, "symbol")?.to_owned(),
        None => env.devnet()?.network.symbol.clone(),
    };
    Ok(Verdict::Output(
        json!({ "text": format_satoshi(amount, &symbol) }),
    ))
}

/// `text.toLocaleFixed`: `value`, a JSON number read as a double, with at most `digits` fraction
/// digits.
fn to_locale_fixed_text(_: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let value = case.input["value"]
        .as_number()
        .map(js::number_value)
        .ok_or_else(|| failed("input.value is not a number"))?;
    let digits = super::integer::<u8>(case.input, "digits")?;
    Ok(Verdict::Output(
        json!({ "text": js::to_locale_fixed(value, digits) }),
    ))
}

/// `text.pluralise`.
fn pluralise_text(_: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let word = text(case.input, "word")?;
    let inclusive = case.input["inclusive"]
        .as_bool()
        .ok_or_else(|| failed("input.inclusive is not a boolean"))?;
    let written = pluralise(word, count(case.input)?, inclusive);
    Ok(in_english(json!({ "text": written })))
}

/// `text.toLocaleString`.
fn to_locale_string(_: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    Ok(in_english(
        json!({ "text": to_locale_integer(count(case.input)?) }),
    ))
}

/// `text.transaction`: the bytes read without checks, as the pool reads a verified transaction,
/// written with the devnet's address byte.
fn transaction(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let bytes =
        hex::decode(text(case.input, "hex")?).map_err(|e| failed(format!("input.hex: {e}")))?;
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    Ok(match Transaction::decode_trusted(&bytes, params) {
        Ok(transaction) => {
            let written = transaction.display(params.network()).to_string();
            Verdict::Output(json!({ "text": written }))
        }
        Err(error) => unclassified(error),
    })
}
