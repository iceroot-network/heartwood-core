//! Transactions: the builders, both decoders, the JSON reader, the handler table and the vote
//! rules.

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::errors::{BuildError, DecodeError};
use heartwood_crypto::transactions::codec::HEADER_SIZE;
use heartwood_crypto::transactions::serialiser::{SerialiseOptions, serialise};
use heartwood_crypto::transactions::types::{Asset, Memo, ResignationType, TransferItem, Votes};
use heartwood_crypto::transactions::{TransactionBuilder, TransactionType};
use heartwood_crypto::utils::hex;
use heartwood_crypto::utils::sort_votes::sort_votes;
use heartwood_crypto::validation::{Mode, votes_from_json};
use heartwood_crypto::{Address, Aux, KeyPair, Params, Transaction, TransactionData};
use serde_json::{Map, Value, json};

use super::{
    Env, Outcome, build_class, decode_class, failed, hex_bytes, integer, params, refused_by_design,
    rejected, text,
};
use crate::subject::{Case, SubjectError, Verdict};

/// The builders of the kept types, by the reference's builder name.
const BUILDERS: [&str; 6] = [
    "transfer",
    "secondSignature",
    "delegateRegistration",
    "delegateResignation",
    "burn",
    "vote",
];

/// `{id, hex, json, isVerified}` of a transaction, as the oracle writes it.
fn output(tx: &Transaction, params: &Params) -> Value {
    json!({
        "id": tx.id().to_hex(),
        "hex": hex::encode(tx.bytes()),
        "json": tx.to_json(params),
        "isVerified": tx.is_verified(),
    })
}

/// A refused decode: a documented difference, or the reference's error class.
fn refused(error: &DecodeError) -> Verdict {
    match refused_by_design(error) {
        Some(reason) => Verdict::RefusedByDesign {
            class: format!("{error:?}"),
            reason: reason.into(),
        },
        None => rejected(decode_class(error)),
    }
}

/// `tx.fromBytes`: the strict decode (or the non-strict one when `input.strict` is false) under
/// the rules at the case's height.
pub(super) fn from_bytes(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    let mode = if case.input.get("strict") == Some(&json!(false)) {
        Mode::NON_STRICT
    } else {
        Mode::STRICT
    };
    Ok(match Transaction::decode_with(&bytes, params, mode) {
        Ok(tx) => Verdict::Output(output(&tx, params)),
        Err(error) => refused(&error),
    })
}

/// `tx.fromJson`: the reference's strict JSON form back to bytes.
pub(super) fn from_json(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    Ok(match Transaction::from_json(&case.input["json"], params) {
        Ok(tx) => Verdict::Output(output(&tx, params)),
        Err(BuildError::Decode(error)) => refused(&error),
        Err(error) => rejected(build_class(&error)),
    })
}

/// `tx.build`: the reference's builder calls, translated to [`TransactionBuilder`], signed with the
/// oracle's aux; the output adds the strict decode verdict of the built bytes.
pub(super) fn build(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let devnet = env.devnet()?;
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let aux = Aux::Fixed(devnet.aux);
    let input = case.input;
    let builder = text(input, "builder")?;
    if !BUILDERS.contains(&builder) {
        return Err(SubjectError::Unsupported(format!(
            "no builder for the removed type of the reference's {builder} builder"
        )));
    }
    let calls = input["calls"]
        .as_array()
        .ok_or_else(|| failed("input.calls is not a list"))?;
    let maximum = match params.transfer_maximum() {
        0 => 256,
        maximum => usize::try_from(maximum).unwrap_or(usize::MAX),
    };
    let mut transfers: Vec<TransferItem> = Vec::new();
    let (mut nonce, mut fee, mut memo, mut asset) = (None, None, None, None);
    for call in calls {
        let method = call[0]
            .as_str()
            .ok_or_else(|| failed("a builder call has no method name"))?;
        let argument = &call[1];
        let argument_text = || {
            argument
                .as_str()
                .ok_or_else(|| failed(format!("the argument of {method} is not text")))
        };
        let argument_amount = |value: &Value| -> Outcome<u64> {
            value
                .as_str()
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| failed(format!("the argument of {method} is not an amount")))
        };
        let recipient = |text: &str| {
            Address::from_base58(text).map_err(|e| failed(format!("recipient {text}: {e}")))
        };
        match method {
            "nonce" => nonce = Some(argument_amount(argument)?),
            "fee" => fee = Some(argument_amount(argument)?),
            // The reference's builder ignores an empty memo.
            "memo" if argument_text()?.is_empty() => {}
            "memo" => match Memo::new(argument_text()?) {
                Ok(value) => memo = Some(value),
                Err(error) => return Ok(rejected(build_class(&BuildError::Memo(error)))),
            },
            // `recipientId` and `amount` set the one item of a single transfer.
            "recipientId" => {
                let amount = transfers.first().map_or(0, |item| item.amount);
                transfers = vec![TransferItem {
                    amount,
                    recipient: recipient(argument_text()?)?,
                }];
            }
            "amount" if builder == "burn" => {
                asset = Some(Asset::Burn {
                    amount: argument_amount(argument)?,
                });
            }
            "amount" => {
                let item = transfers
                    .first()
                    .ok_or_else(|| failed("amount before recipientId"))?;
                transfers = vec![TransferItem {
                    amount: argument_amount(argument)?,
                    recipient: item.recipient,
                }];
            }
            "addTransfer" => {
                // The reference's builder refuses the item over the maximum when it is added.
                if transfers.len() >= maximum {
                    return Ok(rejected("MaximumTransferCountExceededError"));
                }
                transfers.push(TransferItem {
                    amount: argument_amount(&call[2])?,
                    recipient: recipient(argument_text()?)?,
                });
            }
            "signatureAsset" => {
                let keys = KeyPair::from_passphrase(argument_text()?)
                    .map_err(|e| failed(format!("signatureAsset: {e}")))?;
                asset = Some(Asset::SecondSignature {
                    public_key: keys.public_key().to_compressed(),
                });
            }
            "usernameAsset" => {
                asset = Some(Asset::DelegateRegistration {
                    username: argument_text()?.to_owned(),
                });
            }
            "resignationTypeAsset" => {
                let number = argument
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| failed("resignationTypeAsset is not a byte"))?;
                let resignation_type = ResignationType::from_number(number)
                    .ok_or_else(|| failed(format!("resignation type {number}")))?;
                asset = Some(Asset::DelegateResignation {
                    resignation_type: Some(resignation_type),
                });
            }
            "votesAsset" => {
                asset = Some(Asset::Vote {
                    votes: builder_votes(argument)?,
                });
            }
            "senderId" => {
                return Err(SubjectError::Unsupported(
                    "the builders write the standard header only; the extended header is \
                     refused at decode"
                        .into(),
                ));
            }
            other => {
                return Err(SubjectError::Unsupported(format!(
                    "the builder call {other} is not translated"
                )));
            }
        }
    }
    let mut builder = match (builder, asset) {
        ("transfer", _) => TransactionBuilder::transfer(transfers),
        (_, Some(Asset::SecondSignature { public_key })) => {
            TransactionBuilder::second_signature(public_key)
        }
        (_, Some(Asset::DelegateRegistration { username })) => {
            TransactionBuilder::delegate_registration(&username)
        }
        (_, Some(Asset::DelegateResignation { resignation_type })) => {
            TransactionBuilder::delegate_resignation(resignation_type)
        }
        (_, Some(Asset::Burn { amount })) => TransactionBuilder::burn(amount),
        (_, Some(Asset::Vote { votes })) => TransactionBuilder::vote(votes),
        (other, _) => return Err(failed(format!("the {other} builder has no asset"))),
    };
    if let Some(nonce) = nonce {
        builder = builder.nonce(nonce);
    }
    if let Some(fee) = fee {
        builder = builder.fee(fee);
    }
    if let Some(memo) = memo {
        builder = builder.memo(memo);
    }
    let keys =
        KeyPair::from_passphrase(text(input, "sign")?).map_err(|e| failed(format!("sign: {e}")))?;
    let built = match input.get("secondSign").and_then(Value::as_str) {
        Some(second) => {
            let second =
                KeyPair::from_passphrase(second).map_err(|e| failed(format!("secondSign: {e}")))?;
            builder.build_second_signed(&keys, aux, &second, aux, params)
        }
        None => builder.build(&keys, aux, params),
    };
    Ok(match built {
        Ok(tx) => {
            let mut out = output(&tx, params);
            out["strict"] = match Transaction::decode(tx.bytes(), params) {
                Ok(tx) => {
                    json!({ "ok": true, "id": tx.id().to_hex(), "isVerified": tx.is_verified() })
                }
                Err(error) => json!({ "ok": false, "error": { "class": decode_class(&error) } }),
            };
            Verdict::Output(out)
        }
        Err(error) => rejected(build_class(&error)),
    })
}

/// The votes the reference's `votesAsset` makes of its argument: an object of percentages, or a
/// list of names that share 100 % (`floor(10000 / n)` basis points each, one more for each of the
/// first `10000 mod n`).
fn builder_votes(argument: &Value) -> Outcome<Votes> {
    match argument {
        Value::Object(object) => object
            .iter()
            .map(|(name, percent)| {
                let percent = percent
                    .as_f64()
                    .ok_or_else(|| failed(format!("the share of {name} is not a number")))?;
                let basis_points = (percent * 100.0).round();
                if !(0.0..=f64::from(u16::MAX)).contains(&basis_points) {
                    return Err(SubjectError::Unsupported(format!(
                        "the share {percent} of {name} is out of range"
                    )));
                }
                // Whole and in range, checked above.
                Ok((name.clone(), basis_points as u16))
            })
            .collect(),
        Value::Array(names) => {
            let names = names
                .iter()
                .map(|name| {
                    name.as_str()
                        .ok_or_else(|| failed("a vote name is not text"))
                })
                .collect::<Outcome<Vec<&str>>>()?;
            let count = u16::try_from(names.len())
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| failed("votesAsset needs 1 to 65,535 names"))?;
            let remainder = usize::from(10_000 % count);
            Ok(names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    (
                        (*name).to_owned(),
                        10_000 / count + u16::from(index < remainder),
                    )
                })
                .collect())
        }
        other => Err(failed(format!("votesAsset of {other}"))),
    }
}

/// Votes as the oracle writes the reference's vote object: `[[name, percentage], ...]`.
fn vote_entries(votes: &Votes) -> Value {
    Value::Array(
        votes
            .iter()
            .map(|(name, &basis_points)| json!([name, f64::from(basis_points) / 100.0]))
            .collect(),
    )
}

/// `handler.activation`: the reference's handler class for the kept types; removed types have an
/// inactive handler, and any other pair none.
pub(super) fn handler_activation(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let group: u32 = integer(case.input, "typeGroup")?;
    let kind: u16 = integer(case.input, "type")?;
    Ok(match TransactionType::from_wire(group, kind) {
        Ok(kind) => Verdict::Output(json!({ "handler": handler_name(kind) })),
        Err(DecodeError::RemovedType { .. }) => rejected("DeactivatedTransactionHandlerError"),
        Err(error) => rejected(format!("{error:?}")),
    })
}

/// The reference's handler class for a type this implementation carries.
fn handler_name(kind: TransactionType) -> &'static str {
    match kind {
        TransactionType::Transfer => "TransferTransactionHandler",
        TransactionType::SecondSignature => "SecondSignatureRegistrationTransactionHandler",
        TransactionType::DelegateRegistration => "DelegateRegistrationTransactionHandler",
        TransactionType::DelegateResignation => "DelegateResignationTransactionHandler",
        TransactionType::Burn => "BurnTransactionHandler",
        TransactionType::Vote => "VoteTransactionHandler",
    }
}

/// `vote.validate`: the vote rules on a JSON vote object; names that are not delegate names are
/// removed, as the reference's schema removes them.
pub(super) fn vote_validate(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let votes = case.input["votes"]
        .as_object()
        .ok_or_else(|| failed("input.votes is not an object"))?;
    Ok(match votes_from_json(votes) {
        Ok(votes) => Verdict::Output(json!({ "votes": vote_entries(&votes) })),
        Err(_) => rejected("TransactionSchemaError"),
    })
}

/// `vote.sort`: the canonical vote order.
pub(super) fn vote_sort(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let votes = builder_votes(&case.input["votes"])?;
    Ok(Verdict::Output(
        json!({ "votes": vote_entries(&sort_votes(&votes)) }),
    ))
}

/// The vote asset bytes of `votes`, as the vote codec writes them.
fn vote_asset(votes: Votes, keys: &KeyPair, params: &Params) -> Outcome<Vec<u8>> {
    let data = TransactionData {
        version: 3,
        network: params.network(),
        kind: TransactionType::Vote,
        nonce: 1,
        sender_public_key: keys.public_key().clone(),
        fee: 0,
        memo: None,
        asset: Asset::Vote { votes },
        signature: None,
        second_signature: None,
    };
    let bytes = serialise(&data, SerialiseOptions::UNSIGNED, params)
        .map_err(|e| failed(format!("a vote does not serialise: {e}")))?;
    Ok(bytes.get(HEADER_SIZE + 1..).unwrap_or_default().to_vec())
}

/// `vote.splits`: every two-way split of 100 % in cents, as JavaScript numbers; the refused splits,
/// and SHA-256 over the verdicts and over the vote assets.
pub(super) fn vote_splits(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let names: Vec<&str> = case.input["names"]
        .as_array()
        .ok_or_else(|| failed("input.names is not a list"))?
        .iter()
        .map(|name| name.as_str().ok_or_else(|| failed("a name is not text")))
        .collect::<Outcome<_>>()?;
    let [first_name, second_name] = names.as_slice() else {
        return Err(failed("input.names needs two names"));
    };
    let from: u16 = integer(case.input, "from")?;
    let to: u16 = integer(case.input, "to")?;
    if from < 1 || to > 9_999 || from > to {
        return Ok(rejected("RangeError"));
    }
    let keys = KeyPair::from_passphrase("vote asset").map_err(|e| failed(e.to_string()))?;
    let (mut verdicts, mut assets, mut refused) = (Vec::new(), Vec::new(), Vec::new());
    for a in from..=to {
        let first = f64::from(a) / 100.0;
        let second = f64::from(10_000 - a) / 100.0;
        let object: Map<String, Value> = [
            ((*first_name).to_owned(), json!(first)),
            ((*second_name).to_owned(), json!(second)),
        ]
        .into_iter()
        .collect();
        let accepted = votes_from_json(&object).is_ok();
        if !accepted {
            refused.push(a);
        }
        verdicts.push(if accepted { b'1' } else { b'0' });
        let votes: Votes = [
            ((*first_name).to_owned(), a),
            ((*second_name).to_owned(), 10_000 - a),
        ]
        .into_iter()
        .collect();
        assets.extend(vote_asset(votes, &keys, params)?);
    }
    let splits = usize::from(to - from) + 1;
    Ok(Verdict::Output(json!({
        "splits": splits,
        "accepted": splits - refused.len(),
        "refused": refused,
        "verdictsSha256": hex::encode(&sha256(&verdicts)),
        "assetsSha256": hex::encode(&sha256(&assets)),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_of_names_shares_the_vote() {
        let votes = builder_votes(&json!(["a", "b", "c"])).unwrap();
        assert_eq!(
            votes.values().copied().collect::<Vec<_>>(),
            [3334, 3333, 3333]
        );
    }

    #[test]
    fn vote_entries_print_percentages() {
        let votes: Votes = [("a".to_owned(), 5_000u16), ("b".to_owned(), 4_999)]
            .into_iter()
            .collect();
        assert_eq!(vote_entries(&votes), json!([["a", 50.0], ["b", 49.99]]));
    }
}
