//! The kernel against the reference (vector classes V09-rounds and V16-kernel): rounds, forging
//! slots and block chaining under the devnet and other milestone files; the status canonical
//! JSON, `.env` text, epoch timestamps, IP lists, node directories and pluralise.

#[macro_use]
mod common;

use std::path::Path;

use common::{Record, check_devnet, milestones, same_json};
use heartwood_crypto::BlockId;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use heartwood_kernel::config::dot_env::{self, EnvValue};
use heartwood_kernel::errors::{NotChained, TimestampError};
use heartwood_kernel::ip;
use heartwood_kernel::paths::resolve_paths;
use heartwood_kernel::timestamp::format_timestamp;
use heartwood_kernel::utils::pluralise::pluralise;
use heartwood_kernel::utils::stringify;
use heartwood_kernel::{
    ChainLink, calculate_forging_info, calculate_round, is_block_chained, is_new_round,
};
use serde_json::{Value, json};

/// Every record of `class`, run and compared; the failures are listed together.
fn check(class: &str, file: &common::VectorFile, run: fn(&Record) -> Result<Value, String>) {
    let failures: Vec<String> = file
        .records
        .iter()
        .filter_map(|record| {
            let actual = run(record);
            let same = match (&actual, &record.expected) {
                (Ok(actual), Ok(expected)) => same_json(actual, expected),
                (Err(actual), Err(expected)) => actual == expected,
                _ => false,
            };
            (!same).then(|| {
                format!(
                    "{}: expected {:?}, got {actual:?}",
                    record.label(class),
                    record.expected
                )
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(!file.records.is_empty());
}

fn height(input: &Value, key: &str) -> u32 {
    u32::try_from(input[key].as_u64().expect("a height")).expect("a height below 2^32")
}

fn rounds(record: &Record) -> Result<Value, String> {
    let milestones = milestones(record);
    let input = &record.input;
    let heights = || height(input, "from")..=height(input, "to");
    match record.op.as_str() {
        "round.calculate" if input.get("height").is_some() => {
            let info = calculate_round(height(input, "height"), &milestones);
            Ok(json!({
                "round": info.round,
                "roundHeight": info.round_height,
                "nextRound": info.next_round,
                "maxDelegates": info.max_delegates,
            }))
        }
        "round.calculate" => {
            let lines: String = heights()
                .map(|h| {
                    let info = calculate_round(h, &milestones);
                    let (r, rh, nr, n) = (
                        info.round,
                        info.round_height,
                        info.next_round,
                        info.max_delegates,
                    );
                    format!("{h} {r} {rh} {nr} {n}\n")
                })
                .collect();
            Ok(
                json!({"count": heights().count(), "sha256": hex::encode(&sha256(lines.as_bytes()))}),
            )
        }
        "round.isNew" if input.get("height").is_some() => {
            Ok(json!({"isNewRound": is_new_round(height(input, "height"), &milestones)}))
        }
        "round.isNew" => {
            let new: Vec<u32> = heights()
                .filter(|h| is_new_round(*h, &milestones))
                .collect();
            Ok(json!({"count": heights().count(), "newRounds": new}))
        }
        "forging.info" => {
            let timestamp = input["timestamp"].as_i64().expect("a timestamp");
            let info = calculate_forging_info(timestamp, height(input, "height"), &milestones);
            Ok(json!({
                "currentForger": info.current_forger,
                "nextForger": info.next_forger,
                "blockTimestamp": info.block_timestamp,
                "canForge": info.can_forge,
            }))
        }
        "block.isChained" => {
            let link = |block: &Value, previous: &str| ChainLink {
                id: BlockId::from_hex(block["id"].as_str().expect("an id")).expect("hex"),
                previous_block: block
                    .get(previous)
                    .map_or(BlockId::from_array([0; 32]), |id| {
                        BlockId::from_hex(id.as_str().expect("an id")).expect("hex")
                    }),
                height: height(block, "height"),
                timestamp: height(block, "timestamp"),
            };
            let previous = link(&input["previous"], "");
            let next = link(&input["next"], "previousBlock");
            Ok(match is_block_chained(&previous, &next, &milestones) {
                Ok(()) => json!({"isChained": true}),
                Err(NotChained::PreviousBlock) => {
                    json!({"isChained": false, "reason": "previousBlock"})
                }
                Err(NotChained::HeightNotPlusOne) => {
                    json!({"isChained": false, "reason": "height"})
                }
                Err(NotChained::SlotNotAfterPrevious { previous, next }) => json!({
                    "isChained": false, "reason": "slot", "previousSlot": previous, "nextSlot": next,
                }),
            })
        }
        other => panic!("unknown operation {other}"),
    }
}

fn kernel(record: &Record) -> Result<Value, String> {
    let input = &record.input;
    let text = |key: &str| input[key].as_str().expect("text");
    match record.op.as_str() {
        "kernel.stringify" => Ok(json!({"text": stringify::stringify(&input["value"])})),
        "kernel.dotenv" => {
            let pairs = dot_env::parse(text("text"));
            let listed: Vec<Value> = pairs
                .iter()
                .map(|(key, value)| match value {
                    EnvValue::Number(number) => json!([key, number]),
                    EnvValue::Bool(flag) => json!([key, flag]),
                    EnvValue::Text(text) => json!([key, text]),
                })
                .collect();
            Ok(json!({"pairs": listed, "text": dot_env::stringify(&pairs)}))
        }
        "kernel.formatTimestamp" => {
            let milestones = milestones(record);
            let epoch = milestones
                .at(u32::try_from(record.height).expect("a height"))
                .epoch();
            match format_timestamp(input["timestamp"].as_i64().expect("a timestamp"), epoch) {
                Ok(time) => {
                    Ok(json!({"epoch": time.epoch, "unix": time.unix, "human": time.human}))
                }
                Err(TimestampError::OutOfRange { .. }) => Err("RangeError".to_owned()),
                Err(error) => panic!("{error}"),
            }
        }
        "kernel.ip" => {
            let list: Vec<String> = serde_json::from_value(input["list"].clone()).expect("a list");
            let address = text("ip");
            Ok(json!({
                "whitelisted": ip::is_whitelisted(&list, address),
                "blacklisted": ip::is_blacklisted(&list, address),
                "valid": ip::is_valid_address(address),
                "ipv6": ip::is_ipv6_address(address),
                "normalised": ip::normalise_address(address),
                "cleaned": ip::clean_address(address),
            }))
        }
        "kernel.paths" => {
            let env = input.get("env").cloned().unwrap_or(json!({}));
            let lookup = |name: &str| env.get(name).and_then(Value::as_str).map(str::to_owned);
            let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or("/");
            let paths = resolve_paths(
                text("token"),
                text("network"),
                &lookup,
                Path::new(text("home")),
                Path::new(cwd),
            );
            let shown = |path: &Path| path.to_string_lossy().into_owned();
            Ok(json!({
                "data": shown(&paths.data),
                "config": shown(&paths.config),
                "cache": shown(&paths.cache),
                "log": shown(&paths.log),
                "temp": shown(&paths.temp),
            }))
        }
        "text.pluralise" => {
            let count = input["count"].as_i64().expect("a count");
            let inclusive = input["inclusive"].as_bool().expect("a flag");
            Ok(json!({"text": pluralise(text("word"), count.into(), inclusive)}))
        }
        other => panic!("unknown operation {other}"),
    }
}

#[test]
fn v09_rounds() {
    let file = vector_file!("V09-rounds");
    check_devnet(&file);
    check("V09-rounds", &file, rounds);
}

#[test]
fn v16_kernel() {
    let file = vector_file!("V16-kernel");
    check_devnet(&file);
    check("V16-kernel", &file, kernel);
}
