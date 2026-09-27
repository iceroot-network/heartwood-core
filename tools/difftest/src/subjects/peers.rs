//! `peer.attest`: the attestation loop of a ping, answered with
//! `heartwood_p2p::attestation::attest`. The input is a checked status, the other peers' keys,
//! forks and headers, our last block, the current slot and the delegates; the output is
//! `{attributed, versions, moved}`.
//!
//! The status is decoded and checked as the communicator does before a ping's loop; the oracle
//! records only replies that pass the check. The pinged peer's header is the status's, and it
//! holds no key when the loop starts (the ping clears them).

use std::net::IpAddr;

use heartwood_crypto::PublicKeyBytes;
use heartwood_crypto::crypto::slots::SlotInfo;
use heartwood_p2p::attestation::{self, AttestContext, Holder};
use heartwood_p2p_wire::status;
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, hex_bytes, integer, text};
use crate::subject::{Case, Verdict};

/// The operations of the peers.
pub(super) const OPS: &[(&str, Run)] = &[("peer.attest", attest)];

fn address(value: &Value) -> Outcome<IpAddr> {
    let text = value["ip"]
        .as_str()
        .ok_or_else(|| failed("a peer's ip is not text"))?;
    text.parse()
        .map_err(|_| failed(format!("{text} is not an address")))
}

fn key(value: &Value) -> Outcome<PublicKeyBytes> {
    let text = value
        .as_str()
        .ok_or_else(|| failed("a public key is not text"))?;
    PublicKeyBytes::from_hex(text).map_err(|e| failed(format!("the public key {text}: {e}")))
}

fn keys(value: &Value) -> Outcome<Vec<PublicKeyBytes>> {
    value
        .as_array()
        .ok_or_else(|| failed("the public keys are not a list"))?
        .iter()
        .map(key)
        .collect()
}

/// Another peer of the repository.
fn holder(value: &Value) -> Outcome<Holder> {
    let header = &value["header"];
    let height = match &header["height"] {
        Value::Null => None,
        height => Some(
            height
                .as_u64()
                .and_then(|h| u32::try_from(h).ok())
                .ok_or_else(|| failed("a header height is not a height"))?,
        ),
    };
    Ok(Holder {
        ip: address(value)?,
        public_keys: keys(&value["publicKeys"])?,
        forked: value["forked"].as_bool().unwrap_or(false),
        header_height: height,
        header_id: header["id"].as_str().map(str::to_owned),
    })
}

fn attest(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let bytes = hex_bytes(input, "status")?.ok_or_else(|| failed("input.status is not hex"))?;
    let received =
        status::decode_response(&bytes).map_err(|e| failed(format!("the status: {e}")))?;
    let max_delegates: usize = integer(input, "maxDelegates")?;
    let checked = status::check_reply(received, max_delegates)
        .map_err(|e| failed(format!("the status fails its reply check: {e}")))?;
    let header = checked
        .message
        .state
        .as_ref()
        .and_then(|state| state.header.as_ref());
    let pinged = Holder {
        ip: address(&input["peer"])?,
        public_keys: Vec::new(),
        forked: input["peer"]["forked"].as_bool().unwrap_or(false),
        header_height: header.and_then(|header| header.height),
        header_id: header.and_then(|header| header.id.clone()),
    };
    let holders = input["peers"]
        .as_array()
        .ok_or_else(|| failed("input.peers is not a list"))?
        .iter()
        .map(holder)
        .collect::<Outcome<Vec<Holder>>>()?;
    let milestones = env.milestones(case)?;
    let slot = SlotInfo {
        start_time: 0,
        end_time: 0,
        block_time: milestones.at(1).block_time(),
        slot_number: input["slot"]
            .as_i64()
            .ok_or_else(|| failed("input.slot is not an integer"))?,
        forging_status: false,
    };
    let last = &input["lastBlock"];
    let delegate_keys = keys(&input["delegates"])?;
    let context = AttestContext {
        slot: &slot,
        last_height: integer(last, "height")?,
        last_id: text(last, "id")?,
        delegate_keys: &delegate_keys,
        milestones: &milestones,
    };
    let decided = attestation::attest(&checked, &pinged, &holders, &context);
    let hex = |key: &PublicKeyBytes| Value::String(key.to_hex());
    Ok(Verdict::Output(json!({
        "attributed": decided.attributed.iter().map(hex).collect::<Vec<_>>(),
        "versions": decided
            .versions
            .iter()
            .map(|record| json!({
                "publicKey": record.key.to_hex(),
                "round": record.round,
                "version": record.version,
            }))
            .collect::<Vec<_>>(),
        "moved": decided
            .moved
            .iter()
            .map(|(ip, key)| json!({"ip": ip.to_string(), "publicKey": key.to_hex()}))
            .collect::<Vec<_>>(),
    })))
}
