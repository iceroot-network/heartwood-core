//! Passphrase keys, the public-key acceptance set and addresses.

use heartwood_crypto::utils::{base58, hex};
use heartwood_crypto::{Address, KeyPair, PublicKey};
use serde_json::{Value, json};

use super::{Env, Outcome, hex_bytes, rejected, text};
use crate::subject::{Case, Verdict};

/// The network byte of a case: `input.networkByte`, or the devnet's.
fn network_byte(env: &Env<'_>, input: &Value) -> Outcome<u8> {
    match input.get("networkByte") {
        None | Some(Value::Null) => Ok(env.devnet()?.network.pub_key_hash),
        Some(value) => value
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| super::failed("input.networkByte is not a byte")),
    }
}

/// `keys.fromPassphrase`: the secret key is the SHA-256 of the passphrase.
pub(super) fn from_passphrase(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    Ok(
        match KeyPair::from_passphrase(text(case.input, "passphrase")?) {
            Ok(keys) => Verdict::Output(json!({
                "privateKey": keys.secret_key().to_hex().as_str(),
                "publicKey": keys.public_key().to_hex(),
            })),
            Err(error) => rejected(format!("{error:?}")),
        },
    )
}

/// `address.fromPublicKey`: a key the reference refuses is a `PublicKeyError`.
pub(super) fn address_from_public_key(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let network = network_byte(env, case.input)?;
    let key = hex_bytes(case.input, "publicKey")?.and_then(|b| PublicKey::from_bytes(&b).ok());
    Ok(match key {
        Some(key) => Verdict::Output(json!({
            "address": Address::from_public_key(&key, network).to_base58(),
        })),
        None => rejected("PublicKeyError"),
    })
}

/// `address.validate`: the checksum and the network byte, not the payload length.
pub(super) fn address_validate(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let network = network_byte(env, case.input)?;
    let valid = Address::validate(text(case.input, "address")?, network);
    Ok(Verdict::Output(json!({ "valid": valid })))
}

/// `address.toBuffer`: the Base58Check payload of any length, with the network-byte complaint
/// beside it rather than raised; a bad checksum is refused.
pub(super) fn address_to_buffer(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let network = env.devnet()?.network.pub_key_hash;
    Ok(match base58::decode_check(text(case.input, "address")?) {
        Ok(payload) => {
            let complaint = match payload.first() {
                Some(&byte) if byte == network => Value::Null,
                Some(byte) => json!(format!(
                    "Expected address network byte {network}, but got {byte}."
                )),
                None => json!(format!(
                    "Expected address network byte {network}, but got undefined."
                )),
            };
            Verdict::Output(json!({
                "addressBuffer": hex::encode(&payload),
                "addressError": complaint,
            }))
        }
        Err(_) => rejected("Error"),
    })
}

/// `publicKey.verify`: whether the reference accepts the encoding.
pub(super) fn public_key_verify(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    Ok(match hex_bytes(case.input, "publicKey")? {
        Some(bytes) => Verdict::Output(json!({ "valid": PublicKey::is_valid(&bytes) })),
        None => rejected("TypeError"),
    })
}
