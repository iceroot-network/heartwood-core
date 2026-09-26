//! `nes.parse` and `nes.stringify`: frames from and to the oracle's JSON, `{version, type, id,
//! method, statusCode, path, socket, heartbeat, payload}` with the payload as hex.
//!
//! A message of `nes.stringify` is read as a [`Message`]: its `type` names the kind (`request`
//! with the post method, or no method), and its other fields default as the reference's writer
//! defaults them. The reference cuts a long path of a message without the post method, and
//! Heartwood cuts that of every frame but a request; a message where the two differ is not a frame
//! either implementation writes, and is refused as a malformed case.

use heartwood_p2p_wire::nes::{Heartbeat, Kind, Message};
use serde_json::{Value, json};

use super::bytes_value;
use crate::subject::{Case, Verdict};
use crate::subjects::{Env, Outcome, failed, hex_bytes, rejected};

/// The longest path the reference's writer keeps without the post method, in UTF-16 code units.
const MAX_PATH_UNITS: usize = 255;

/// `nes.parse`: the frame in `input.hex`, or the reference's error class.
pub(super) fn parse(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    Ok(match Message::parse(&bytes) {
        Ok(message) => Verdict::Output(frame_value(&message)),
        Err(error) => rejected(error.js_class()),
    })
}

/// `nes.stringify`: the bytes of `input.message`, or the reference's error class.
pub(super) fn stringify(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let message = message(&case.input["message"])?;
    Ok(match message.encode() {
        Ok(bytes) => Verdict::Output(json!({ "hex": bytes_value(&bytes) })),
        Err(error) => rejected(error.js_class()),
    })
}

/// A parsed frame in the oracle's shape.
fn frame_value(message: &Message) -> Value {
    let kind = match message.kind {
        Kind::Hello => "hello",
        Kind::Ping => "ping",
        Kind::Request => "request",
        Kind::Error => "error",
    };
    json!({
        "version": message.version.to_string(),
        "type": kind,
        "id": message.id,
        // Without the extended types, every frame the parser accepts has the post method.
        "method": "post",
        "statusCode": message.status_code,
        "path": message.path,
        "socket": message.socket,
        "heartbeat": {
            "interval": message.heartbeat.interval,
            "timeout": message.heartbeat.timeout,
        },
        "payload": bytes_value(&message.payload),
    })
}

/// The [`Message`] of a `nes.stringify` input.
fn message(value: &Value) -> Outcome<Message> {
    let field = |key: &str| value.get(key).filter(|v| !v.is_null());
    let number = |key: &str| -> Outcome<u64> {
        field(key).map_or(Ok(0), |v| {
            v.as_u64()
                .ok_or_else(|| failed(format!("input.message.{key} is not an integer")))
        })
    };
    let string = |key: &str| -> Outcome<String> {
        field(key).map_or(Ok(String::new()), |v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| failed(format!("input.message.{key} is not text")))
        })
    };
    let method_post = match field("method") {
        None => false,
        Some(method) => method
            .as_str()
            .ok_or_else(|| failed("input.message.method is not text"))?
            .eq_ignore_ascii_case("post"),
    };
    let kind = match string("type")?.as_str() {
        "hello" => Kind::Hello,
        "ping" => Kind::Ping,
        "error" => Kind::Error,
        "request" if method_post || field("method").is_none() => Kind::Request,
        other => {
            return Err(failed(format!(
                "input.message: type {other:?} is not a frame of the peer-to-peer port"
            )));
        }
    };
    let path = string("path")?;
    if path.encode_utf16().count() > MAX_PATH_UNITS && (kind == Kind::Request) != method_post {
        return Err(failed(
            "input.message: a long path that only one implementation cuts",
        ));
    }
    let heartbeat = field("heartbeat").unwrap_or(&Value::Null);
    let small = |n: u64, key: &str| {
        u16::try_from(n).map_err(|_| failed(format!("input.message.{key} is over 65535")))
    };
    let payload = match field("payload") {
        None => Vec::new(),
        Some(_) => hex_bytes(value, "payload")?
            .ok_or_else(|| failed("input.message.payload is not hex"))?,
    };
    Ok(Message {
        kind,
        version: match field("version") {
            None => 0,
            Some(_) => string("version")?
                .parse()
                .map_err(|_| failed("input.message.version is not a byte's decimal text"))?,
        },
        id: u32::try_from(number("id")?)
            .map_err(|_| failed("input.message.id is over 2^32 - 1"))?,
        status_code: small(number("statusCode")?, "statusCode")?,
        path,
        socket: string("socket")?,
        heartbeat: Heartbeat {
            interval: small(heartbeat["interval"].as_u64().unwrap_or(0), "heartbeat")?,
            timeout: small(heartbeat["timeout"].as_u64().unwrap_or(0), "heartbeat")?,
        },
        payload,
    })
}
