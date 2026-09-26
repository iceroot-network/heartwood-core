//! Milestone configuration (vector class V12): the merge, the start-up checks and their refusals,
//! the reward and donations each merged milestone gives, and transaction rules taken at the
//! block's height.

#[macro_use]
mod common;
mod blocks_common;

use blocks_common::{
    DEVNET, Tally, check_devnet, compare, make_record, same_json, text, transaction_record,
};
use common::Record;
use heartwood_crypto::errors::ConfigError;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::TransferItem;
use heartwood_crypto::utils::donations::calculate_donations;
use heartwood_crypto::utils::hex;
use heartwood_crypto::utils::reward_calculator::calculate_reward;
use heartwood_crypto::{Address, Aux, KeyPair, Milestones, Transaction};
use serde_json::{Value, json};

const V12: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/V12-milestone-config.jsonl"
));

#[test]
fn v12_milestone_config() {
    let file = vector_file!("V12-milestone-config");
    check_devnet(&file);
    let mut tally = Tally::default();
    for record in &file.records {
        let outcome = match record.op.as_str() {
            "config.load" => load_record(record),
            "tx.build" => build_record(record, &file.meta),
            "tx.fromBytes" => transaction_record(record),
            "block.make" => make_record(record),
            other => Err(format!("unknown operation {other}")),
        };
        tally.check(record.label(&file.class), outcome);
    }
    tally.finish(&file);
}

/// The error message of a refused record, which the shared loader does not keep.
fn error_message(record: &Record) -> String {
    let line = V12.lines().nth(record.line - 1).unwrap_or_default();
    let value: Value = serde_json::from_str(line).unwrap_or_default();
    value["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The height a refusal of the reference names: `Bad milestone at height: N.`
fn refused_height(message: &str) -> Option<u64> {
    let rest = message.strip_prefix("Bad milestone at height: ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A `config.load` record: the merged milestones and the reward and donations at the listed
/// heights, or a refusal at the same height as the reference's.
fn load_record(record: &Record) -> Result<(), String> {
    let loaded = Milestones::load(text(&record.input, "milestones")?, &DEVNET.network);
    match (&record.expected, loaded) {
        (Ok(expected), Ok(milestones)) => {
            let merged: Vec<Value> = milestones
                .all()
                .iter()
                .map(|params| Value::Object(params.as_json().clone()))
                .collect();
            let rewards = pairs(&record.input, "rewards")?
                .into_iter()
                .map(|(height, rank)| {
                    let rank = u32::try_from(rank).map_err(|error| error.to_string())?;
                    Ok(match calculate_reward(milestones.at(height), rank) {
                        Ok(reward) => json!([height, rank, reward.to_string()]),
                        Err(error) => json!([height, rank, { "error": error.to_string() }]),
                    })
                })
                .collect::<Result<Vec<Value>, String>>()?;
            let donations = record.input["donations"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|pair| {
                    let height = pair[0].as_u64().ok_or("a height is not an integer")?;
                    let height = u32::try_from(height).map_err(|error| error.to_string())?;
                    let reward: u64 = pair[1]
                        .as_str()
                        .and_then(|reward| reward.parse().ok())
                        .ok_or("a reward is not an amount")?;
                    let shares: Vec<Value> = calculate_donations(reward, milestones.at(height))
                        .into_iter()
                        .map(|(address, amount)| json!([address.to_base58(), amount.to_string()]))
                        .collect();
                    Ok(json!([height, reward.to_string(), shares]))
                })
                .collect::<Result<Vec<Value>, String>>()?;

            let mut failures = Vec::new();
            if !same_json(&Value::Array(merged.clone()), &expected["milestones"]) {
                failures.push(format!(
                    "merged milestones differ\n  expected {}\n  actual   {}",
                    expected["milestones"],
                    Value::Array(merged)
                ));
            }
            // A refused reward is compared as a refusal; the reference's error text is its own.
            let expected_rewards: Vec<Value> = expected["rewards"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|entry| match &entry[2] {
                    Value::Object(_) => json!([entry[0], entry[1], { "error": "refused" }]),
                    _ => entry.clone(),
                })
                .collect();
            let actual_rewards: Vec<Value> = rewards
                .into_iter()
                .map(|entry| match &entry[2] {
                    Value::Object(_) => json!([entry[0], entry[1], { "error": "refused" }]),
                    _ => entry,
                })
                .collect();
            if !same_json(&json!(actual_rewards), &json!(expected_rewards)) {
                failures.push(format!(
                    "rewards differ: expected {expected_rewards:?}, actual {actual_rewards:?}"
                ));
            }
            if !same_json(&json!(donations), &expected["donations"]) {
                failures.push(format!(
                    "donations differ: expected {}, actual {}",
                    expected["donations"],
                    json!(donations)
                ));
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(failures.join("; "))
            }
        }
        (Err(class), Err(error)) => {
            let message = error_message(record);
            match (class.as_str(), &error) {
                ("SyntaxError", ConfigError::Json(_)) => Ok(()),
                ("InvalidMilestoneConfigurationError", _)
                    if error.height().is_some() && error.height() == refused_height(&message) =>
                {
                    Ok(())
                }
                _ => Err(format!(
                    "refused for another reason: the reference raises {class} ({message}), this \
                     implementation {error:?}"
                )),
            }
        }
        (Ok(_), Err(error)) => Err(format!("refused, but the reference accepts: {error}")),
        (Err(class), Ok(_)) => Err(format!(
            "accepted, but the reference raises {class}: {}",
            error_message(record)
        )),
    }
}

/// `[height, n]` pairs of the input field `key`.
fn pairs(input: &Value, key: &str) -> Result<Vec<(u32, u64)>, String> {
    input[key]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|pair| {
            let height = pair[0].as_u64().ok_or("a height is not an integer")?;
            let n = pair[1].as_u64().ok_or("not an integer")?;
            Ok((u32::try_from(height).map_err(|error| error.to_string())?, n))
        })
        .collect()
}

/// A `tx.build` record: a transfer built with the builder under the devnet milestones, with the
/// oracle's fixed aux.
fn build_record(record: &Record, meta: &Value) -> Result<(), String> {
    let height = u32::try_from(record.height).map_err(|error| error.to_string())?;
    let params = DEVNET.milestones.at(height);
    let aux = Aux::Fixed(
        hex::decode_array(meta["aux"].as_str().ok_or("no aux")?).map_err(|e| e.to_string())?,
    );
    if text(&record.input, "builder")? != "transfer" {
        return Err("only transfers are built in this class".into());
    }
    let calls = record.input["calls"]
        .as_array()
        .ok_or("calls is not a list")?;
    let mut transfers = Vec::new();
    let mut nonce = None;
    for call in calls {
        let argument = |index: usize| call[index].as_str().ok_or("an argument is not text");
        match argument(0)? {
            "addTransfer" => transfers.push(TransferItem {
                recipient: Address::from_base58(argument(1)?).map_err(|e| e.to_string())?,
                amount: argument(2)?.parse().map_err(|_| "not an amount")?,
            }),
            "nonce" => nonce = Some(argument(1)?.parse::<u64>().map_err(|_| "not a nonce")?),
            other => return Err(format!("builder method {other} is not used in this class")),
        }
    }
    let mut builder = TransactionBuilder::transfer(transfers);
    if let Some(nonce) = nonce {
        builder = builder.nonce(nonce);
    }
    let keys = KeyPair::from_passphrase(text(&record.input, "sign")?).map_err(|e| e.to_string())?;
    let built = builder
        .build(&keys, aux, params)
        .map_err(|error| error.to_string())?;
    let strict = match Transaction::decode(built.bytes(), params) {
        Ok(decoded) => json!({
            "ok": true,
            "id": decoded.id().to_hex(),
            "isVerified": decoded.is_verified(),
        }),
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    };
    let actual = json!({
        "id": built.id().to_hex(),
        "hex": hex::encode(built.bytes()),
        "json": built.to_json(params),
        "isVerified": built.is_verified(),
        "strict": strict,
    });
    compare(record, Ok(actual))
}
