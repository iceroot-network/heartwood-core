//! The peer-to-peer wire: the oracle's operations of vector class V13, answered with
//! `heartwood-p2p-wire`.
//!
//! - `nes`: `nes.parse` and `nes.stringify`, the frames.
//! - `peer`: getPeers, getCommonBlocks and the getStatus request.
//! - `blocks` and `transactions`: the block and transaction routes.
//! - `status`: the getStatus response, `status.sign` and `status.verify`.
//!
//! `wire.encode` takes `route` (the reference's codec name, such as `getStatus`), `direction`
//! (`request` or `response`) and `value` in the oracle's JSON shape: present keys only, bytes and
//! transactions as hex, amounts as decimal text. It answers `{hex}`. `wire.decode` takes `route`,
//! `direction` and `hex`, and answers `{value}`: the decoded object's own keys in the reference's
//! order, bytes as hex, amounts as decimal text. `wire.decodeSelected` takes `hex`, whose first
//! byte (0 for an empty input) selects the codec ([`Codec::select`]) and whose rest is the
//! payload; it answers `{route, direction, value}`. A refusal compares the reference's error
//! class.

mod blocks;
mod nes;
mod peer;
mod status;
mod transactions;

use heartwood_crypto::utils::hex;
use heartwood_p2p_wire::{Codec, DecodeError, Direction, Proto, RouteId};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed, hex_bytes, rejected, text, unclassified};
use crate::subject::{Case, Documented, Verdict};

/// Every operation of the wire, with the function that answers it.
pub(super) const OPS: &[(&str, Run)] = &[
    ("nes.parse", nes::parse),
    ("nes.stringify", nes::stringify),
    ("wire.encode", encode),
    ("wire.decode", decode),
    ("wire.decodeSelected", decode_selected),
    ("status.sign", status::sign),
    ("status.verify", status::verify),
];

/// The subject file that answers a codec.
#[derive(Debug, Clone, Copy)]
enum Area {
    Peer,
    Status,
    Blocks,
    Transactions,
}

fn area(codec: Codec) -> Area {
    match codec {
        Codec::GetPeersRequest
        | Codec::GetPeersResponse
        | Codec::GetCommonBlocksRequest
        | Codec::GetCommonBlocksResponse
        | Codec::GetStatusRequest => Area::Peer,
        Codec::GetStatusResponse => Area::Status,
        Codec::GetBlocksRequest
        | Codec::GetBlocksResponse
        | Codec::PostBlockRequest
        | Codec::PostBlockResponse => Area::Blocks,
        Codec::GetUnconfirmedTransactionsRequest
        | Codec::GetUnconfirmedTransactionsResponse
        | Codec::PostTransactionsRequest
        | Codec::PostTransactionsResponse => Area::Transactions,
    }
}

/// The codec that `input.route` and `input.direction` name.
fn codec(input: &Value) -> Outcome<Codec> {
    let name = text(input, "route")?;
    let route = RouteId::ALL
        .into_iter()
        .find(|route| route.name() == name)
        .ok_or_else(|| failed(format!("input.route: no route named {name}")))?;
    let direction = match text(input, "direction")? {
        "request" => Direction::Request,
        "response" => Direction::Response,
        other => return Err(failed(format!("input.direction: {other}"))),
    };
    Codec::of(route, direction).ok_or_else(|| failed(format!("{name} has no protobuf codec")))
}

/// `wire.encode`: the payload of `input.value`.
fn encode(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let codec = codec(case.input)?;
    let value = &case.input["value"];
    let bytes = match area(codec) {
        Area::Peer => peer::encode(codec, value)?,
        Area::Status => status::encode_response(value)?,
        Area::Blocks => blocks::encode(codec, value)?,
        Area::Transactions => transactions::encode(codec, value)?,
    };
    Ok(Verdict::Output(json!({ "hex": hex::encode(&bytes) })))
}

/// The verdict on `bytes` decoded with `codec`, whose output is the bare decoded value.
fn decode_with(codec: Codec, bytes: &[u8]) -> Outcome<Verdict> {
    match area(codec) {
        Area::Peer => peer::decode(codec, bytes),
        Area::Status => status::decode_response(bytes),
        Area::Blocks => blocks::decode(codec, bytes),
        Area::Transactions => transactions::decode(codec, bytes),
    }
}

/// `wire.decode`: `{value}`.
fn decode(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let codec = codec(case.input)?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    Ok(map_output(
        decode_with(codec, &bytes)?,
        &|value| json!({ "value": value }),
    ))
}

/// `wire.decodeSelected`: `{route, direction, value}`.
fn decode_selected(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    let (first, payload) = bytes.split_first().map_or((0, &[][..]), |(b, p)| (*b, p));
    let codec = Codec::select(first);
    Ok(map_output(decode_with(codec, payload)?, &|value| {
        json!({
            "route": codec.route().name(),
            "direction": codec.direction().name(),
            "value": value,
        })
    }))
}

/// `verdict` with `wrap` applied to its output, where it has one.
fn map_output(verdict: Verdict, wrap: &dyn Fn(Value) -> Value) -> Verdict {
    match verdict {
        Verdict::Output(value) => Verdict::Output(wrap(value)),
        Verdict::DiffersWhereShown {
            difference,
            verdict,
        } => Verdict::DiffersWhereShown {
            difference,
            verdict: Box::new(map_output(*verdict, wrap)),
        },
        other => other,
    }
}

// Shared by the area files. A helper keeps its `expect(dead_code)` until an area first calls it.

/// Bytes in the oracle's JSON: lowercase hex text.
fn bytes_value(bytes: &[u8]) -> Value {
    Value::String(hex::encode(bytes))
}

/// The JSON object of a decoded `message`: its own keys in the reference's order
/// ([`Proto::own_keys`]), each with the value that `field` gives for its (JavaScript) name.
fn object<M: Proto>(
    message: &M,
    mut field: impl FnMut(&'static str) -> Outcome<Value>,
) -> Outcome<Value> {
    let mut object = Map::new();
    for key in message.own_keys() {
        object.insert(key.to_owned(), field(key)?);
    }
    Ok(Value::Object(object))
}

/// Heartwood's verdict on a payload it decoded to `value`. Heartwood skips protobuf groups at any
/// depth, where the reference's stack overflows: an accepted payload whose oracle answer is that
/// overflow is the documented difference, and any other is compared as usual.
fn accepted(value: Value) -> Verdict {
    Verdict::DiffersWhereShown {
        difference: Documented::DeepGroups,
        verdict: Box::new(Verdict::Output(value)),
    }
}

/// Heartwood's verdict on a payload it refused with `error`: the reference's error class, or, for
/// an amount beyond 128 bits, the documented difference where the oracle shows it.
fn refused(error: DecodeError) -> Verdict {
    match error.js_class() {
        Some(class) => rejected(class),
        None => Verdict::DiffersWhereShown {
            difference: Documented::WideAmount,
            verdict: Box::new(unclassified(error)),
        },
    }
}
