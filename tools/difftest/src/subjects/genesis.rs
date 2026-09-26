//! The devnet chain definition, built from a genesis profile, a seed and an epoch.

use std::path::Path;

use heartwood_crypto::blocks::factory;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::genesis::DerivedKey;
use heartwood_crypto::utils::hex;
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Milestones, Network, Transaction};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, failed, rejected, text};
use crate::genesis::{Built, CRYPTO_FILES, Profile};
use crate::subject::{Case, Verdict};

/// The fields of a derived key as the devnet generator writes them.
fn key_json(key: &DerivedKey) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("role".into(), json!(key.role));
    out.insert("address".into(), json!(key.address.to_base58()));
    out.insert("publicKey".into(), json!(key.keys.public_key().to_hex()));
    out.insert("passphrase".into(), json!(key.passphrase.as_str()));
    out
}

fn with(mut fields: Map<String, Value>, extra: Value) -> Value {
    if let Value::Object(extra) = extra {
        fields.extend(extra);
    }
    Value::Object(fields)
}

/// `genesis.generate`: the four crypto files, the key material and the summary figures of the
/// chain definition, and the files read back as the reference's check reads them: the block from
/// its JSON, verified, with every transaction decoded under the genesis rules.
pub(super) fn generate(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let origin = Path::new("the case's genesis profile");
    let profile: Profile = serde_json::from_value(case.input["profile"].clone())
        .map_err(|e| failed(format!("input.profile: {e}")))?;
    let built = match Built::from_profile(
        origin,
        profile,
        text(case.input, "seed")?,
        text(case.input, "epoch")?,
    ) {
        Ok(built) => built,
        Err(error) => return Ok(rejected(format!("{error:?}"))),
    };
    let definition = &built.definition;
    let input = &built.input;
    let texts = [
        &definition.network_json,
        &definition.milestones_json,
        &definition.genesis_block_json,
        &definition.exceptions_json,
    ];
    let files: Map<String, Value> = CRYPTO_FILES
        .iter()
        .zip(texts)
        .map(|(name, text)| ((*name).to_owned(), json!(text)))
        .collect();
    let file_digests: Map<String, Value> = CRYPTO_FILES
        .iter()
        .zip(texts)
        .map(|(name, text)| {
            (
                (*name).to_owned(),
                json!(hex::encode(&sha256(text.as_bytes()))),
            )
        })
        .collect();
    let summary = &definition.summary;
    let delegates: Vec<Value> = input
        .delegates
        .iter()
        .zip(&definition.delegates)
        .map(|(delegate, key)| {
            let fields = key_json(key);
            json!({
                "username": delegate.username,
                "role": fields["role"],
                "address": fields["address"],
                "publicKey": fields["publicKey"],
                "privateKey": key.keys.secret_key().to_hex().as_str(),
                "passphrase": fields["passphrase"],
            })
        })
        .collect();
    let genesis_wallets: Vec<Value> = definition
        .allocations
        .iter()
        .zip(&input.allocations)
        .map(|(key, allocation)| {
            with(
                key_json(key),
                json!({ "genesisBalance": allocation.amount.to_string() }),
            )
        })
        .collect();
    let donation_wallets: Vec<Value> = definition
        .donations
        .iter()
        .zip(&input.donations)
        .map(|(key, share)| {
            with(
                key_json(key),
                json!({
                    "genesisBalance": "0",
                    "basisPoints": share.basis_points,
                    "fromHeight": input.donations_from_height,
                }),
            )
        })
        .collect();
    let test_wallets: Vec<Value> = definition
        .wallets
        .iter()
        .map(|key| Value::Object(key_json(key)))
        .collect();
    Ok(Verdict::Output(json!({
        "files": files,
        "fileSha256": file_digests,
        "nethash": hex::encode(&summary.nethash),
        "genesisId": summary.genesis_id.to_hex(),
        "transactions": summary.transactions,
        "issued": summary.issued.to_string(),
        "seedSha256": hex::encode(&sha256(input.seed.as_bytes())),
        "delegates": delegates,
        "genesisWallets": genesis_wallets,
        "donationWallets": donation_wallets,
        "testWallets": test_wallets,
        "generator": with(
            key_json(&definition.generator),
            json!({ "issued": summary.issued.to_string() }),
        ),
        "check": read_back(&built)?,
    })))
}

/// The written files read back as the reference's check reads them: `genesisBlock.json` with the
/// block factory's JSON reader (the genesis rules apply at height 1), the block verified, and each
/// transaction decoded from its bytes under the genesis rules.
fn read_back(built: &Built) -> Outcome<Value> {
    let definition = &built.definition;
    let network = Network::load(&definition.network_json)
        .map_err(|e| failed(format!("the built network.json: {e}")))?;
    let milestones = Milestones::load(&definition.milestones_json, &network)
        .map_err(|e| failed(format!("the built milestones.json: {e}")))?;
    let json: Value = serde_json::from_str(&definition.genesis_block_json)
        .map_err(|e| failed(format!("the built genesisBlock.json: {e}")))?;
    let block = factory::from_json(&json, &milestones)
        .map_err(|e| failed(format!("the built genesis block does not read back: {e}")))?;
    let verification = block.verify(&milestones, 0);
    let errors: Vec<String> = verification
        .failures
        .iter()
        .map(ToString::to_string)
        .collect();
    let params = milestones.at(1);
    let strictly_verified = block
        .transactions()
        .iter()
        .filter(|tx| {
            Transaction::decode_with(tx.bytes(), params, Mode::GENESIS)
                .is_ok_and(|decoded| decoded.is_verified())
        })
        .count();
    Ok(json!({
        "id": block.id().to_hex(),
        "verified": verification.is_verified(),
        "errors": errors,
        "transactions": block.transactions().len(),
        "strictlyVerified": strictly_verified,
        "payloadLength": block.header().payload_length,
    }))
}
