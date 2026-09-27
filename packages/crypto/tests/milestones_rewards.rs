//! Rewards, donations, the fee burn and the dynamic fee floor under the devnet milestones (vector
//! class V06), and a block's total amount, which counts top-level amounts only.

#[macro_use]
mod common;
mod blocks_common;

use blocks_common::{DEVNET, Tally, check_devnet, compare, fee_record, make_record, text};
use common::Record;
use heartwood_crypto::utils::burn::burned_fee;
use heartwood_crypto::utils::donations::calculate_donations;
use heartwood_crypto::utils::reward_calculator::calculate_reward;
use serde_json::{Value, json};

#[test]
fn v06_arithmetic() {
    let file = vector_file!("V06-arithmetic");
    check_devnet(&file);
    let mut tally = Tally::default();
    for record in &file.records {
        let label = record.label(&file.class);
        match record.op.as_str() {
            "fee.burned" => tally.check(label, burned_record(record)),
            "reward.calculate" => tally.check(label, reward_record(record)),
            "reward.donations" => tally.check(label, donations_record(record)),
            "block.make" => tally.check(label, make_record(record)),
            "fee.minimum" => tally.check(label, fee_record(record)),
            other => tally.check(label, Err(format!("unknown operation {other}"))),
        }
    }
    tally.finish(&file);
}

fn height(record: &Record) -> Result<u32, String> {
    u32::try_from(record.height).map_err(|error| error.to_string())
}

fn amount(input: &Value, key: &str) -> Result<u64, String> {
    text(input, key)?
        .parse()
        .map_err(|_| format!("{key} is not an amount"))
}

fn burned_record(record: &Record) -> Result<(), String> {
    let params = DEVNET.milestones.at(height(record)?);
    let fee = amount(&record.input, "fee")?;
    compare(
        record,
        Ok(json!({ "burnedFee": burned_fee(fee, params).to_string() })),
    )
}

fn reward_record(record: &Record) -> Result<(), String> {
    let params = DEVNET.milestones.at(height(record)?);
    let rank = record.input["rank"]
        .as_u64()
        .ok_or("rank is not an integer")?;
    let rank = u32::try_from(rank).map_err(|error| error.to_string())?;
    let actual = calculate_reward(params, rank)
        .map(|reward| json!({ "reward": reward.to_string() }))
        .map_err(|error| error.to_string());
    compare(record, actual)
}

fn donations_record(record: &Record) -> Result<(), String> {
    let params = DEVNET.milestones.at(height(record)?);
    let reward = amount(&record.input, "reward")?;
    let shares: Vec<Value> = calculate_donations(reward, params)
        .into_iter()
        .map(|(address, amount)| json!([address.to_base58(), amount.to_string()]))
        .collect();
    compare(record, Ok(json!({ "donations": shares })))
}
