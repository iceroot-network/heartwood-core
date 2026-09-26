//! Blocks, rewards, donations, the fee burn and the milestone loader.

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::blocks::{VerifyFailure, serialiser};
use heartwood_crypto::crypto::hash::{sha256, sha256_concat};
use heartwood_crypto::crypto::sig;
use heartwood_crypto::managers::Milestones;
use heartwood_crypto::transactions::serialiser::{SerialiseOptions, serialise};
use heartwood_crypto::utils::burn::burned_fee;
use heartwood_crypto::utils::donations::calculate_donations;
use heartwood_crypto::utils::reward_calculator::calculate_reward;
use heartwood_crypto::utils::{hex, js};
use heartwood_crypto::{
    Aux, Block, BlockHeader, KeyPair, SchemeId, Signature, SigningDomain, Transaction,
};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, amount, bytes32, failed, hex_bytes, integer, params, rejected, text};
use crate::subject::{Case, SubjectError, Verdict};

/// A refusal: blocks and milestone files have no observable error classes beyond the verdict.
fn refused(error: impl std::fmt::Debug) -> Verdict {
    rejected(format!("{error:?}"))
}

/// `block.make`: a block shaped as the forger makes it, from transactions read without checks at
/// the block's height, signed with BIP340 by the factory or with the legacy scheme over the same
/// hash (`input.scheme`), then assembled as the reference's `BlockFactory.fromData` does. Header
/// overrides (`version`, `payloadLength`, `totalAmount`) make refused blocks.
pub(super) fn make(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let input = case.input;
    let height: u32 = integer(input, "height")?;
    let params = milestones.at(height);
    let mut transactions = Vec::new();
    for item in input["transactions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let bytes = item
            .as_str()
            .and_then(|text| hex::decode(text).ok())
            .ok_or_else(|| failed("a transaction is not hex"))?;
        match Transaction::decode_trusted(&bytes, params) {
            Ok(tx) => transactions.push(tx),
            Err(error) => return Ok(refused(error)),
        }
    }
    let overrides = input.get("overrides").cloned().unwrap_or_else(|| json!({}));
    let count = u32::try_from(transactions.len()).map_err(|_| failed("too many transactions"))?;
    let total_amount = match overrides.get("totalAmount") {
        Some(_) => amount(&overrides, "totalAmount")?,
        None => match transactions
            .iter()
            .try_fold(0_u64, |sum, tx| sum.checked_add(tx.amount()))
        {
            Some(sum) => sum,
            None => return Ok(rejected("RangeError")),
        },
    };
    let Some(total_fee) = transactions
        .iter()
        .try_fold(0_u64, |sum, tx| sum.checked_add(tx.fee()))
    else {
        return Ok(rejected("RangeError"));
    };
    let draft = BlockDraft {
        version: match overrides.get("version") {
            Some(_) => integer(&overrides, "version")?,
            None => 0,
        },
        timestamp: integer(input, "timestamp")?,
        height,
        previous_block: bytes32(input, "previousBlock")?,
        number_of_transactions: count,
        total_amount,
        total_fee,
        reward: amount(input, "reward")?,
        payload_length: match overrides.get("payloadLength") {
            Some(_) => integer(&overrides, "payloadLength")?,
            None => count.saturating_mul(32),
        },
        payload_hash: sha256_concat(transactions.iter().map(|tx| tx.id().as_bytes().as_slice())),
    };
    let keys = KeyPair::from_passphrase(text(input, "passphrase")?)
        .map_err(|e| failed(format!("passphrase: {e}")))?;
    let aux = Aux::Fixed(bytes32(input, "aux")?);
    let block = match input
        .get("scheme")
        .and_then(Value::as_str)
        .unwrap_or("bip340")
    {
        "bip340" => factory::make(draft, transactions, &keys, aux, &milestones),
        "legacy" => {
            let mut header = BlockHeader {
                version: draft.version,
                timestamp: draft.timestamp,
                height: draft.height,
                previous_block: draft.previous_block,
                number_of_transactions: draft.number_of_transactions,
                total_amount: draft.total_amount,
                total_fee: draft.total_fee,
                reward: draft.reward,
                payload_length: draft.payload_length,
                payload_hash: draft.payload_hash,
                generator_public_key: keys.public_key().to_compressed(),
                block_signature: Signature::from_array([0; 64]),
            };
            header.block_signature = sig::sign(
                SchemeId::Secp256k1Legacy,
                SigningDomain::Block,
                &serialiser::signing_hash(&header),
                keys.secret_key(),
                Aux::Fixed([0; 32]),
            )
            .map_err(|e| failed(format!("legacy block signature: {e}")))?;
            factory::from_data(header, &transactions, &milestones)
        }
        other => return Err(failed(format!("unknown scheme {other}"))),
    };
    let summary = input["summary"].as_bool().unwrap_or(false);
    Ok(match block {
        Ok(block) => Verdict::Output(block_output(&block, &milestones, summary, env)?),
        Err(error) => refused(error),
    })
}

/// `block.fromBytes`: the block decoder, under the rules at the block's height.
pub(super) fn from_bytes(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    let summary = case.input["summary"].as_bool().unwrap_or(false);
    Ok(match Block::decode(&bytes, &milestones) {
        Ok(block) => Verdict::Output(block_output(&block, &milestones, summary, env)?),
        Err(error) => refused(error),
    })
}

/// The block's bytes as the oracle writes them: the signed header, then every transaction
/// serialised again from its fields. [`Block::encode`] keeps each transaction's received bytes
/// instead; the two differ only for a transaction in a non-canonical encoding (a version byte 0,
/// or vote names that the rules remove), and the block id covers neither.
fn reencoded(block: &Block, milestones: &Milestones) -> Outcome<Vec<u8>> {
    let params = milestones.at(block.height());
    let transactions = block
        .transactions()
        .iter()
        .map(|tx| {
            serialise(tx.data(), SerialiseOptions::FULL, params)
                .map_err(|e| failed(format!("a decoded transaction does not serialise: {e}")))
        })
        .collect::<Outcome<Vec<Vec<u8>>>>()?;
    let mut bytes = serialiser::serialise_header(block.header(), true);
    for tx in &transactions {
        let length = u32::try_from(tx.len()).map_err(|_| failed("a transaction over 4 GiB"))?;
        bytes.extend_from_slice(&length.to_le_bytes());
    }
    for tx in transactions {
        bytes.extend(tx);
    }
    Ok(bytes)
}

/// A block as the oracle writes it: id, signed header, bytes and JSON (or, with `summary`, the
/// SHA-256 and length of the bytes and of the canonical JSON text), and the verification.
fn block_output(
    block: &Block,
    milestones: &Milestones,
    summary: bool,
    env: &Env<'_>,
) -> Outcome<Value> {
    let bytes = reencoded(block, milestones)?;
    let json = block.to_json(milestones);
    let mut out = Map::new();
    out.insert("id".into(), json!(block.id().to_hex()));
    out.insert(
        "headerHex".into(),
        json!(hex::encode(&serialiser::serialise_header(
            block.header(),
            true
        ))),
    );
    if summary {
        out.insert("bytes".into(), digest(&bytes));
        out.insert("jsonText".into(), digest(canonical_json(&json).as_bytes()));
        out.insert("transactions".into(), json!(block.transactions().len()));
    } else {
        out.insert("hex".into(), json!(hex::encode(&bytes)));
        out.insert("json".into(), json);
    }
    let verify_time = env.devnet()?.verify_time;
    out.insert(
        "verification".into(),
        verification(block, milestones, verify_time),
    );
    Ok(Value::Object(out))
}

/// The verification as the reference reports it: its error texts, in its check order.
fn verification(block: &Block, milestones: &Milestones, verify_time: i64) -> Value {
    let verification = block.verify(milestones, verify_time);
    let mut errors = Vec::new();
    for failure in &verification.failures {
        match failure {
            VerifyFailure::UnverifiedTransactions(_) => {
                errors.push("One or more transactions are not verified:".to_owned());
                for tx in block.transactions().iter().filter(|tx| !tx.is_verified()) {
                    errors.push(format!("=> {}", hex::encode(tx.bytes())));
                }
            }
            other => errors.push(other.to_string()),
        }
    }
    json!({
        "verified": verification.is_verified(),
        "containsMultiSignatures": false,
        "errors": errors,
    })
}

fn digest(bytes: &[u8]) -> Value {
    json!({ "sha256": hex::encode(&sha256(bytes)), "length": bytes.len() })
}

/// JSON text with the keys of every object sorted by UTF-16 code units and no whitespace, as the
/// oracle writes it for its summaries.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            let entries: Vec<String> = entries
                .into_iter()
                .map(|(key, item)| {
                    format!("{}:{}", js::stringify(&json!(key), 0), canonical_json(item))
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        other => js::stringify(other, 0),
    }
}

/// `fee.burned`: the burned part of a fee at the case's height.
pub(super) fn fee_burned(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let fee = amount(case.input, "fee")?;
    Ok(Verdict::Output(
        json!({ "burnedFee": burned_fee(fee, params).to_string() }),
    ))
}

/// `fee.minimum`: the dynamic fee floor is a rule of the transaction handlers, which are not part
/// of `heartwood-crypto`. The crate's tests check the type key and size it is computed from.
pub(super) fn fee_minimum(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(
        "the dynamic fee floor belongs to the transaction handlers".into(),
    ))
}

/// `reward.calculate`: the rank table's entry, or the milestone's reward.
pub(super) fn reward_calculate(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let rank: u32 = integer(case.input, "rank")?;
    Ok(match calculate_reward(params, rank) {
        Ok(reward) => Verdict::Output(json!({ "reward": reward.to_string() })),
        Err(error) => refused(error),
    })
}

/// The donations of a reward at a height: `[[address, amount], ...]` in file order.
fn donation_shares(reward: u64, milestones: &Milestones, height: u32) -> Value {
    calculate_donations(reward, milestones.at(height))
        .into_iter()
        .map(|(address, amount)| json!([address.to_base58(), amount.to_string()]))
        .collect()
}

/// `reward.donations`.
pub(super) fn reward_donations(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let reward = amount(case.input, "reward")?;
    let height = super::height(case)?;
    Ok(Verdict::Output(
        json!({ "donations": donation_shares(reward, &milestones, height) }),
    ))
}

/// `config.load`: a milestone file (JSON text) loaded under the devnet network: the merged
/// milestones, and the reward and donations at the listed heights; or a refusal.
pub(super) fn config_load(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let devnet = env.devnet()?;
    let milestones = match Milestones::load(text(case.input, "milestones")?, &devnet.network) {
        Ok(milestones) => milestones,
        Err(error) => return Ok(refused(error)),
    };
    let merged: Vec<Value> = milestones
        .all()
        .iter()
        .map(|params| Value::Object(params.as_json().clone()))
        .collect();
    let pairs = |key: &str| -> Outcome<Vec<(Value, Value)>> {
        case.input
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|pair| match pair.as_array().map(Vec::as_slice) {
                Some([a, b]) => Ok((a.clone(), b.clone())),
                _ => Err(failed(format!(
                    "input.{key} holds a pair that is not a pair"
                ))),
            })
            .collect()
    };
    let at = |value: &Value| -> Outcome<u32> {
        value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| failed("a height is not a 32-bit integer"))
    };
    let rewards = pairs("rewards")?
        .into_iter()
        .map(|(height, rank)| {
            let rank_number = rank
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| failed("a rank is not a 32-bit integer"))?;
            Ok(
                match calculate_reward(milestones.at(at(&height)?), rank_number) {
                    Ok(reward) => json!([height, rank, reward.to_string()]),
                    // The reference raises a plain Error; its message is not compared.
                    Err(error) => json!([
                        height,
                        rank,
                        { "error": { "class": "Error", "message": error.to_string() } }
                    ]),
                },
            )
        })
        .collect::<Outcome<Vec<Value>>>()?;
    let donations = pairs("donations")?
        .into_iter()
        .map(|(height, reward)| {
            let amount: u64 = reward
                .as_str()
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| failed("a reward is not an amount"))?;
            Ok(json!([
                height,
                reward,
                donation_shares(amount, &milestones, at(&height)?)
            ]))
        })
        .collect::<Outcome<Vec<Value>>>()?;
    Ok(Verdict::Output(json!({
        "milestones": merged,
        "rewards": rewards,
        "donations": donations,
    })))
}
