//! The transaction handlers: the dynamic fee floor, and one transaction applied to a wallet
//! fixture and reverted.

use std::borrow::Cow;

use heartwood_crypto::Transaction;
use heartwood_crypto::errors::ConfigError;
use heartwood_crypto::managers::Milestones;
use heartwood_crypto::utils::js;
use heartwood_transactions::minimum_fee;
use serde_json::{Map, Value, json};

use super::{Env, Outcome, decode_refused, failed, hex_bytes, params, rejected};
use crate::subject::{Case, Documented, SubjectError, Verdict};

/// The key of the revoke delay in a milestone.
const REVOKE_DELAY: &str = "blocksToRevokeDelegateResignation";

/// `fee.minimum`: the dynamic fee floor of a transaction decoded (strictly) at the case's height,
/// with its type key and byte length.
pub(super) fn fee_minimum(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = match milestones(env, case)? {
        Ok(milestones) => milestones,
        Err(refused) => return Ok(refused),
    };
    let params = params(&milestones, case)?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    Ok(match Transaction::decode(&bytes, params) {
        Ok(tx) => Verdict::Output(json!({
            "key": tx.key(),
            "size": tx.bytes().len(),
            "minimumFee": minimum_fee(&tx, params).to_string(),
        })),
        Err(error) => decode_refused(&error, &bytes),
    })
}

/// `state.applyTransaction`: a transaction applied to a fixture, then reverted.
pub(super) fn apply_transaction(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// The milestones a case runs under (its own file, or the devnet's), or the verdict on a file
/// that Heartwood refuses for a fee or revoke-delay value the reference loads.
fn milestones<'e>(
    env: &'e Env<'_>,
    case: &Case<'_>,
) -> Outcome<Result<Cow<'e, Milestones>, Verdict>> {
    let devnet = env.devnet()?;
    let text = match case.input.get("milestones") {
        None | Some(Value::Null) => return Ok(Ok(Cow::Borrowed(&devnet.milestones))),
        Some(value) => value
            .as_str()
            .ok_or_else(|| failed("input.milestones is not text"))?,
    };
    match Milestones::load(text, &devnet.network) {
        Ok(milestones) => Ok(Ok(Cow::Owned(milestones))),
        Err(ConfigError::Invalid { key, .. } | ConfigError::Missing { key, .. })
            if key.starts_with("dynamicFees") || key == REVOKE_DELAY =>
        {
            Ok(Err(Verdict::RefusedByDesign {
                difference: Documented::FeeKeyValueType,
                holds: has_fee_key_form(text),
            }))
        }
        Err(error) => Err(failed(format!(
            "the case's milestones do not load: {error}"
        ))),
    }
}

/// Whether the milestone file `text` states a fee table or revoke delay that Heartwood refuses:
/// `dynamicFees` neither an object nor `null`; in the table, `enabled` not a boolean, `minFee` or
/// an `addonBytes` value not a safe integer, `addonBytes` not an object, or `enabled` true without
/// `minFee` or `addonBytes`; or a revoke delay that is not a safe integer. It reads each milestone
/// as written: a table left without `minFee` by the merge was switched on by a milestone that
/// lacks it.
fn has_fee_key_form(text: &str) -> bool {
    let Ok(Value::Array(milestones)) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    let unsafe_number = |value: &Value| js::safe_integer(value).is_none();
    let bad_table = |table: &Map<String, Value>| {
        let enabled = table.get("enabled");
        let addons = match table.get("addonBytes") {
            None => false,
            Some(Value::Object(addons)) => addons.values().any(unsafe_number),
            Some(_) => true,
        };
        enabled.is_some_and(|value| !value.is_boolean())
            || table.get("minFee").is_some_and(unsafe_number)
            || addons
            || (enabled == Some(&Value::Bool(true))
                && !(table.contains_key("minFee") && table.contains_key("addonBytes")))
    };
    milestones
        .iter()
        .filter_map(Value::as_object)
        .any(|milestone| {
            let table = match milestone.get("dynamicFees") {
                None | Some(Value::Null) => false,
                Some(Value::Object(table)) => bad_table(table),
                Some(_) => true,
            };
            table || milestone.get(REVOKE_DELAY).is_some_and(unsafe_number)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fee_key_form() {
        let with = |members: &str| format!(r#"[{{"height":1{members}}}]"#);
        for refused in [
            r#","dynamicFees":true"#,
            r#","dynamicFees":{"enabled":1}"#,
            r#","dynamicFees":{"minFee":"6173"}"#,
            r#","dynamicFees":{"minFee":1.5}"#,
            r#","dynamicFees":{"addonBytes":null}"#,
            r#","dynamicFees":{"addonBytes":{"transfer":"85"}}"#,
            r#","dynamicFees":{"enabled":true,"addonBytes":{}}"#,
            r#","dynamicFees":{"enabled":true,"minFee":1}"#,
            r#","blocksToRevokeDelegateResignation":"106""#,
            r#","blocksToRevokeDelegateResignation":null"#,
        ] {
            assert!(has_fee_key_form(&with(refused)), "{refused}");
        }
        for accepted in [
            "",
            r#","dynamicFees":null"#,
            r#","dynamicFees":{}"#,
            r#","dynamicFees":{"enabled":false}"#,
            r#","dynamicFees":{"enabled":true,"minFee":-5,"addonBytes":{"transfer":85.0}}"#,
            r#","blocksToRevokeDelegateResignation":-3"#,
        ] {
            assert!(!has_fee_key_form(&with(accepted)), "{accepted}");
        }
        assert!(!has_fee_key_form("not json"));
    }
}
