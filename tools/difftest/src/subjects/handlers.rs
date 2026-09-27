//! The transaction handlers: the dynamic fee floor, and one transaction applied to a wallet
//! fixture and reverted.

use std::borrow::Cow;

use heartwood_crypto::Transaction;
use heartwood_crypto::errors::ConfigError;
use heartwood_crypto::managers::Milestones;
use heartwood_crypto::utils::js;
use heartwood_state::digest::wallet_section;
use heartwood_state::errors::TransactionFailure;
use heartwood_state::{ApplyContext, block_state};
use heartwood_transactions::{Handlers, TransactionError, minimum_fee};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, decode_refused, failed, hex_bytes, integer, params, rejected};
use crate::fixture::Fixture;
use crate::subject::{Case, Documented, Verdict};

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

/// `state.applyTransaction`: a transaction applied to a wallet fixture as block
/// `lastBlockHeight + 1` applies it (the handler, then the vote balances of the wallets it
/// changed, under the rules of that height), then reverted. The output holds the `#W` section
/// after each step, or the refusal's class and the `#W` section after the refused attempt.
pub(super) fn apply_transaction(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = match milestones(env, case)? {
        Ok(milestones) => milestones,
        Err(refused) => return Ok(refused),
    };
    let last_block_height: u32 = integer(case.input, "lastBlockHeight")?;
    let height = last_block_height
        .checked_add(1)
        .ok_or_else(|| failed("input.lastBlockHeight is the last height there is"))?;
    let params = milestones.at(height);
    let fixture = Fixture::from_json(&case.input["fixture"]).map_err(|e| failed(e.to_string()))?;
    let mut wallets = fixture
        .to_wallets(params.network())
        .map_err(|e| failed(e.to_string()))?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    let transaction = match Transaction::decode(&bytes, params) {
        Ok(transaction) => transaction,
        Err(error) => return Ok(decode_refused(&error, &bytes)),
    };
    let ctx = ApplyContext {
        height,
        params,
        last_block_height,
    };
    // A refusal gives its class; a failed rollback leaves no state to compare, so it fails the
    // case.
    let error = |failure: TransactionFailure<TransactionError>| match failure {
        TransactionFailure::Refused(rejected) => {
            Ok(json!({ "class": rejected.class(), "message": rejected.to_string() }))
        }
        TransactionFailure::RollbackFailed(error) => Err(failed(error.to_string())),
    };
    let output = match block_state::apply_transaction(&ctx, &mut wallets, &transaction, &Handlers) {
        Ok(undo) => {
            let applied = wallet_section(&wallets);
            let reverted = match block_state::revert_transaction(
                &ctx,
                &mut wallets,
                &transaction,
                &undo,
                &Handlers,
            ) {
                Ok(()) => json!({ "W": wallet_section(&wallets) }),
                // Only the verdict of a failed revert is compared.
                Err(failure) => json!({ "error": error(failure)? }),
            };
            json!({ "applied": { "W": applied }, "reverted": reverted })
        }
        Err(failure) => {
            json!({ "error": error(failure)?, "after": { "W": wallet_section(&wallets) } })
        }
    };
    Ok(Verdict::Output(output))
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
