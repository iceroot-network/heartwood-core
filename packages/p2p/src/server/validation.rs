//! The server's request checks, as the reference's server makes them (and as the oracle's
//! `p2p.serveRequest` answers them): the route's payload limit (413), the codec's decode (400
//! "Payload deserialising failed"), the version gate on `headers.version` (400 "Validation
//! failed (invalid version)"), then the route's request schema, the reference's schemas of the
//! seven public routes and `emitEvent` (400 "Validation failed").
//!
//! The schemas read a decoded request as the reference's schema library reads its object:
//! getStatus, getPeers, getCommonBlocks and getBlocks are the protobuf library's messages, whose
//! absent fields read as their defaults, so a request without headers, or with headers without a
//! version, is refused; postBlock, getUnconfirmedTransactions and postTransactions are copies of
//! the message's own keys, so their headers are optional, and a `countOnly` key, false or true, is
//! one the schema does not allow. An empty payload is none at all to the reference's decoder.
//!
//! The internal routes' JSON is parsed from the payload's text, invalid UTF-8 replaced. A refusal
//! of the decoder names the reference's error class; the text after it is Heartwood's (the
//! reference's protobuf reader adds its offsets, and its JSON parser words its errors its own way).

use heartwood_crypto::Params;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::{get_blocks_decode_request, post_block_decode_request};
use heartwood_p2p_wire::codecs::peer::{
    get_common_blocks_decode_request, get_peers_decode_request, get_status_decode_request,
};
use heartwood_p2p_wire::codecs::transactions::{
    get_unconfirmed_transactions_decode_request, post_transactions_decode_request,
};
use heartwood_p2p_wire::errors::DecodeError;
use serde_json::Value;

use super::seams::ServerRefusal;
use crate::utils::is_valid_version::is_valid_version;

/// The payload limit of a route that sets none (the internal routes): the HTTP server's default.
pub const DEFAULT_MAX_BYTES: usize = 1_048_576;

/// The version gate's inputs: the milestone at the tip, the `minimumVersions` option and the
/// network's name, as the client's gate reads them.
pub struct VersionGate<'a> {
    /// The milestone.
    pub milestone: &'a Params,
    /// The configured version ranges.
    pub configured: &'a [String],
    /// The network's name.
    pub network_name: &'a str,
}

/// What the plugins after the checks read of an accepted request's headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checked {
    /// The version the headers carry, when it is not empty.
    pub version: Option<String>,
    /// The port the headers carry, when it is not 0.
    pub port: Option<u16>,
}

/// The payload limit of `route`: 413 when a payload of `length` bytes is above it.
pub fn check_size(route: RouteId, length: usize) -> Result<(), ServerRefusal> {
    let limit = (route
        .max_bytes()
        .and_then(|limit| usize::try_from(limit).ok()))
    .unwrap_or(DEFAULT_MAX_BYTES);
    if length > limit {
        let message = format!("Payload content length greater than maximum allowed: {limit}");
        return Err(ServerRefusal::new(413, message));
    }
    Ok(())
}

/// Check `payload`, a request on `route` within its size, as the reference's server does: the
/// codec, the version gate and the schema, `max_transactions` being the pool's
/// `maxTransactionsPerRequest`.
pub fn check_request(
    route: RouteId,
    payload: &[u8],
    gate: &VersionGate<'_>,
    max_transactions: usize,
) -> Result<Checked, ServerRefusal> {
    if payload.is_empty() && route.is_internal() {
        let text = "Cannot read properties of null (reading 'toString')";
        return Err(undecodable("TypeError", text));
    }
    if payload.is_empty() {
        return Err(undecodable("Error", "illegal buffer"));
    }
    if route.is_internal() {
        return check_json(route, payload, gate);
    }
    // The headers, whether the object may lack them, and the route's own fields' verdict.
    let (headers, optional, valid) = match route {
        RouteId::GetStatus => (
            decode(get_status_decode_request(payload))?.headers,
            false,
            true,
        ),
        RouteId::GetPeers => (
            decode(get_peers_decode_request(payload))?.headers,
            false,
            true,
        ),
        RouteId::GetCommonBlocks => {
            let request = decode(get_common_blocks_decode_request(payload))?;
            let hex = |id: &String| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
            let valid = (1..=10).contains(&request.ids.len()) && request.ids.iter().all(hex);
            (request.headers, false, valid)
        }
        RouteId::GetBlocks => {
            let request = decode(get_blocks_decode_request(payload))?;
            let height = request.last_block_height.unwrap_or_default() >= 1;
            let limit = (1..=400).contains(&request.block_limit.unwrap_or_default());
            (request.headers, false, height && limit)
        }
        RouteId::PostBlock => (
            decode(post_block_decode_request(payload))?.headers,
            true,
            true,
        ),
        RouteId::GetUnconfirmedTransactions => {
            let request = decode(get_unconfirmed_transactions_decode_request(payload))?.request;
            (request.headers, true, request.count_only.is_none())
        }
        _ => {
            let listed = decode(post_transactions_decode_request(payload))?;
            let valid = listed.transactions.len() <= max_transactions;
            (listed.message.headers, true, valid)
        }
    };
    let version = (headers.as_ref().and_then(|headers| headers.version.clone()))
        .filter(|version| !version.is_empty());
    if version
        .as_deref()
        .is_some_and(|version| !gate.allows(version))
    {
        return Err(invalid_version());
    }
    let port = headers.as_ref().and_then(|headers| headers.port);
    let headers_valid = match headers {
        None => optional,
        Some(_) => version.is_some() && port.unwrap_or_default() <= 65_535,
    };
    if !(headers_valid && valid) {
        return Err(ServerRefusal::new(400, "Validation failed"));
    }
    let port = port
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0);
    Ok(Checked { version, port })
}

impl VersionGate<'_> {
    /// Whether the gate lets `version` through.
    fn allows(&self, version: &str) -> bool {
        let (milestone, configured) = (self.milestone, self.configured);
        is_valid_version(Some(version), milestone, configured, self.network_name)
    }
}

/// An internal route's JSON: its `headers.version` through the gate when it is truthy (a value
/// that is not text is no version), and emitEvent's schema `{event: string, body: object}`.
fn check_json(
    route: RouteId,
    payload: &[u8],
    gate: &VersionGate<'_>,
) -> Result<Checked, ServerRefusal> {
    let text = String::from_utf8_lossy(payload);
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| undecodable("SyntaxError", &error.to_string()))?;
    let truthy = |value: &Value| match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    };
    let version = value
        .get("headers")
        .and_then(|headers| headers.get("version"));
    if version
        .is_some_and(|version| truthy(version) && !version.as_str().is_some_and(|v| gate.allows(v)))
    {
        return Err(invalid_version());
    }
    let emitted = |object: &serde_json::Map<String, Value>| {
        object.iter().all(|(key, value)| match key.as_str() {
            "event" => value.as_str().is_some_and(|event| !event.is_empty()),
            "body" => value.is_object(),
            _ => false,
        })
    };
    if route == RouteId::EmitEvent && !value.as_object().is_some_and(emitted) {
        return Err(ServerRefusal::new(400, "Validation failed"));
    }
    Ok(Checked::default())
}

fn invalid_version() -> ServerRefusal {
    ServerRefusal::new(400, "Validation failed (invalid version)")
}

fn decode<T>(decoded: Result<T, DecodeError>) -> Result<T, ServerRefusal> {
    decoded.map_err(|error| {
        let text = match error {
            DecodeError::IndexOutOfRange => "index out of range".to_owned(),
            DecodeError::InvalidWireType(wire_type) => format!("invalid wire type {wire_type}"),
            other => other.to_string(),
        };
        undecodable(error.js_class().unwrap_or("Error"), &text)
    })
}

fn undecodable(class: &str, text: &str) -> ServerRefusal {
    ServerRefusal::new(
        400,
        format!("Payload deserialising failed: {class}: {text}"),
    )
}
