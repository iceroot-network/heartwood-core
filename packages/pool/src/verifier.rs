//! The processor's first loop: each entry decoded and its signature checked, on up to
//! `worker_count` threads, the results in entry order.
//!
//! Bytes are decoded as the reference's strict `fromBytes` does, JSON as its `fromData` does,
//! under the rules of the tip plus one. A failure is `ERR_BAD_DATA` with the message
//! `Invalid transaction data: <reason>`. A signature that does not verify does not refuse the
//! entry here: the sender state refuses it later
//! ([`heartwood_crypto::Transaction::is_verified`]). A transaction of a removed type is kept as
//! [`Checked::Removed`], which the second loop refuses with `ERR_OTHER`, where the reference's
//! handler registry refuses it.
//!
//! The reference's worker hands the main process the bytes, id and signature verdict, which
//! decodes them again without the rules (`fromBytesUnsafe`). The pool's transaction is decoded
//! the same way, so its fields are the wire's even where the rules drop a vote name.

use std::thread;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::errors::{BuildError, DecodeError};
use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Params, Transaction};
use serde_json::Value;

use crate::errors::{PoolCode, Refusal};
use crate::processor::Entry;

/// An entry after the first loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// A decoded transaction.
    Decoded(Box<Transaction>),
    /// A transaction of a removed type.
    Removed {
        /// Its id: the SHA-256 of the received bytes, or for JSON the given `id` (the entry's
        /// index when it has none).
        id: String,
        /// The type group.
        type_group: u32,
        /// The type.
        kind: u16,
        /// The transaction as the reference's `toString` writes it.
        display: String,
    },
}

impl Checked {
    /// The id that the cooldown and the second loop use.
    pub fn id(&self) -> String {
        match self {
            Checked::Decoded(transaction) => transaction.id().to_hex(),
            Checked::Removed { id, .. } => id.clone(),
        }
    }

    /// The transaction as the reference's `toString` writes it, the sender's address derived
    /// with the node's address byte `network`.
    pub fn display(&self, network: u8) -> String {
        match self {
            Checked::Decoded(transaction) => transaction.display(network).to_string(),
            Checked::Removed { display, .. } => display.clone(),
        }
    }
}

/// Decode and check `entries` under `params` on up to `worker_count` threads; the results are in
/// entry order and equal to those of one thread.
pub fn check_entries(
    entries: &[Entry],
    params: &Params,
    worker_count: usize,
) -> Vec<Result<Checked, Refusal>> {
    let check = |start: usize, part: &[Entry]| -> Vec<Result<Checked, Refusal>> {
        (start..)
            .zip(part)
            .map(|(index, entry)| check_entry(index, entry, params))
            .collect()
    };
    let workers = worker_count.clamp(1, entries.len().max(1));
    if workers == 1 {
        return check(0, entries);
    }
    let size = entries.len().div_ceil(workers);
    thread::scope(|scope| {
        let parts: Vec<_> = entries
            .chunks(size)
            .enumerate()
            .map(|(n, part)| {
                let start = n.saturating_mul(size);
                let worker = thread::Builder::new()
                    .spawn_scoped(scope, move || check(start, part))
                    .ok();
                (start, part, worker)
            })
            .collect();
        parts
            .into_iter()
            .flat_map(|(start, part, worker)| match worker {
                // A worker cannot panic on any input; should one, its entries are refused.
                Some(worker) => worker.join().unwrap_or_else(|_| {
                    part.iter()
                        .map(|_| Err(invalid_data("the worker failed")))
                        .collect()
                }),
                // No thread could be started: this thread does the part.
                None => check(start, part),
            })
            .collect()
    })
}

/// Decode entry `index`.
fn check_entry(index: usize, entry: &Entry, params: &Params) -> Result<Checked, Refusal> {
    let decoded = match entry {
        Entry::Bytes(bytes) => match Transaction::decode(bytes, params) {
            Err(DecodeError::RemovedType {
                type_group,
                type_id,
            }) => return removed_from_bytes(bytes, type_group, type_id, params),
            other => other.map_err(invalid_data),
        },
        Entry::Data(json) => match Transaction::from_json(json, params) {
            Err(BuildError::Decode(DecodeError::RemovedType {
                type_group,
                type_id,
            })) => return removed_from_json(json, index, type_group, type_id, params),
            other => other.map_err(invalid_data),
        },
    }?;
    // The main process's `fromBytesUnsafe`: the wire's fields, the worker's signature verdict.
    let transaction = if decoded.is_verified() {
        Transaction::decode_trusted(decoded.bytes(), params)
            .ok()
            .filter(|trusted| trusted.id() == decoded.id())
            .unwrap_or(decoded)
    } else {
        decoded
    };
    Ok(Checked::Decoded(Box::new(transaction)))
}

/// `ERR_BAD_DATA` for an entry that does not decode.
fn invalid_data(reason: impl std::fmt::Display) -> Refusal {
    Refusal::new(
        PoolCode::BadData,
        format!("Invalid transaction data: {reason}"),
    )
}

/// The placeholder of the bytes of a removed type, read from the header that every type shares:
/// the version at byte 1, the nonce at bytes 9 to 16 and the sender's key at bytes 17 to 49.
/// A version other than 3 (or 0, which reads as 3) is refused, as the decoder refuses it; the
/// placeholder's text shows the wire byte, as the text of a decoded transaction does.
fn removed_from_bytes(
    bytes: &[u8],
    type_group: u32,
    kind: u16,
    params: &Params,
) -> Result<Checked, Refusal> {
    let version = bytes.get(1).copied().unwrap_or(0);
    if version != 0 && version != 3 {
        return Err(invalid_data(DecodeError::UnsupportedVersion(version)));
    }
    let nonce = bytes
        .get(9..17)
        .and_then(|field| <[u8; 8]>::try_from(field).ok())
        .map(u64::from_le_bytes);
    let key = bytes
        .get(17..50)
        .and_then(|field| PublicKey::from_bytes(field).ok());
    let (Some(nonce), Some(key)) = (nonce, key) else {
        return Err(invalid_data("the header of a removed type is incomplete"));
    };
    let id = hex::encode(&sha256(bytes));
    Ok(removed(
        id,
        type_group,
        kind,
        &key,
        &nonce.to_string(),
        version,
        params,
    ))
}

/// The placeholder of the JSON of a removed type. Its id is the given one, or the entry's index
/// when it is `null`; the id, the sender's key and the nonce must pass the rules the JSON of a
/// kept type passes.
fn removed_from_json(
    json: &Value,
    index: usize,
    type_group: u32,
    kind: u16,
    params: &Params,
) -> Result<Checked, Refusal> {
    let field = |name: &str| json.get(name);
    let id = match field("id") {
        Some(Value::Null) => index.to_string(),
        Some(Value::String(id)) if id.len() == 64 && hex::decode(id).is_ok() => id.clone(),
        Some(_) => return Err(invalid_data("invalid id: must be 64 hex digits or null")),
        None => return Err(invalid_data("missing id")),
    };
    let key = field("senderPublicKey")
        .and_then(Value::as_str)
        .and_then(|text| PublicKey::from_hex(text).ok())
        .ok_or_else(|| invalid_data("invalid senderPublicKey"))?;
    let nonce = match field("nonce") {
        Some(Value::String(text)) => text.parse::<u64>().ok(),
        Some(Value::Number(number)) => number.as_u64(),
        _ => None,
    }
    .ok_or_else(|| invalid_data("invalid nonce"))?;
    let version = match field("version").and_then(Value::as_u64) {
        Some(2) => 2,
        _ => 3,
    };
    Ok(removed(
        id,
        type_group,
        kind,
        &key,
        &nonce.to_string(),
        version,
        params,
    ))
}

/// A placeholder with the reference's `toString` of the removed type.
fn removed(
    id: String,
    type_group: u32,
    kind: u16,
    key: &PublicKey,
    nonce: &str,
    version: u8,
    params: &Params,
) -> Checked {
    let name = match kind {
        0 => "LegacyTransfer",
        3 => "LegacyVote",
        4 => "MultiSignature",
        5 => "Ipfs",
        8 => "HtlcLock",
        9 => "HtlcClaim",
        _ => "HtlcRefund",
    };
    let sender = Address::from_public_key(key, params.network());
    let tail = id.get(id.len().saturating_sub(8)..).unwrap_or_default();
    Checked::Removed {
        display: format!("{sender}#{nonce} {tail} {name} v{version}"),
        id,
        type_group,
        kind,
    }
}
