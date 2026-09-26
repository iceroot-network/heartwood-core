//! The nes frame records of vector class V13, both ways: every `nes.parse` record is parsed and
//! compared field by field with the reference's result or error class, every `nes.stringify`
//! record is written and compared byte for byte, and each frame the reference sends on the
//! peer-to-peer port (records named `sent: ...`) is also built with its constructor.
//!
//! The vector file is read from `tests/vectors`, or from the directory in `WIRE_VECTORS` (a
//! relative path is taken from the workspace root), for example a local oracle run in
//! `target/vectors/local`. Without `WIRE_VECTORS`, a missing file is reported and the test passes,
//! so the crate's tests run before its vectors are installed.

use std::path::{Path, PathBuf};

use heartwood_p2p_wire::nes::{Heartbeat, Kind, Message};
use heartwood_p2p_wire::routes::RouteId;
use serde_json::{Value, json};

const CLASS: &str = "V13-p2p-wire";

/// The records of the V13 file, without its meta record; `None` when the file is not installed.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let explicit = std::env::var_os("WIRE_VECTORS").map(PathBuf::from);
    let dir = match &explicit {
        Some(dir) => manifest.join("../..").join(dir),
        None => manifest.join("tests/vectors"),
    };
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "frames: {} is not installed; nothing checked",
                file.display()
            );
            return None;
        }
        Err(error) => panic!("{}: {error}", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    Some(lines.collect())
}

fn unhex(text: &Value) -> Vec<u8> {
    hex::decode(text.as_str().unwrap()).unwrap()
}

/// The reference's result for a record: its output, or its error class.
fn expected(record: &Value) -> Result<&Value, &str> {
    match record.get("error") {
        Some(error) => Err(error["class"].as_str().unwrap()),
        None => Ok(&record["output"]),
    }
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
        "method": "post",
        "statusCode": message.status_code,
        "path": message.path,
        "socket": message.socket,
        "heartbeat": { "interval": message.heartbeat.interval, "timeout": message.heartbeat.timeout },
        "payload": hex::encode(&message.payload),
    })
}

/// The [`Message`] of a `nes.stringify` input: absent fields as the reference's writer defaults
/// them.
fn message(value: &Value) -> Message {
    let number = |key: &str| value[key].as_u64().unwrap_or(0);
    let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
    let kind = match value["type"].as_str().unwrap() {
        "hello" => Kind::Hello,
        "ping" => Kind::Ping,
        "request" => Kind::Request,
        "error" => Kind::Error,
        other => panic!("type {other}"),
    };
    let heartbeat = |key: &str| u16::try_from(value["heartbeat"][key].as_u64().unwrap_or(0));
    Message {
        kind,
        version: value["version"].as_str().map_or(0, |v| v.parse().unwrap()),
        id: u32::try_from(number("id")).unwrap(),
        status_code: u16::try_from(number("statusCode")).unwrap(),
        path: text("path"),
        socket: text("socket"),
        heartbeat: Heartbeat {
            interval: heartbeat("interval").unwrap(),
            timeout: heartbeat("timeout").unwrap(),
        },
        payload: value.get("payload").map(unhex).unwrap_or_default(),
    }
}

/// The frame a `sent: ...` record describes, built with the constructor for it.
fn constructed(name: &str, input: &Message) -> (&'static str, Message) {
    let payload = input.payload.clone();
    let what = name.trim_start_matches("sent: ");
    if what.starts_with("client hello") {
        ("client hello", Message::client_hello(input.id))
    } else if what.starts_with("request") {
        let route = RouteId::from_id(&input.path).expect("a route id");
        ("request", Message::request(input.id, route, payload))
    } else if what.starts_with("server hello") {
        let socket = input.socket.clone();
        (
            "server hello",
            Message::server_hello(input.id, input.heartbeat, socket),
        )
    } else if what.starts_with("response") {
        ("response", Message::response(input.id, payload))
    } else if what.contains("ping") {
        ("ping", Message::ping(input.id))
    } else if what.starts_with("graceful error") {
        (
            "graceful error",
            Message::graceful_error(input.id, &payload),
        )
    } else {
        panic!("no constructor for {name:?}")
    }
}

#[test]
fn frame_records_both_ways() {
    let Some(records) = records() else { return };
    let (mut parsed, mut written, mut sent) = (0, 0, std::collections::BTreeSet::new());
    for (index, record) in records.iter().enumerate() {
        let name = record["name"].as_str().unwrap_or_default();
        let label = format!("{CLASS}:{} {} {name:?}", index + 2, record["op"]);
        match record["op"].as_str().unwrap() {
            "nes.parse" => {
                let actual = Message::parse(&unhex(&record["input"]["hex"]));
                match (expected(record), actual) {
                    (Ok(output), Ok(message)) => {
                        assert_eq!(&frame_value(&message), output, "{label}")
                    }
                    (Err(class), Err(error)) => assert_eq!(error.js_class(), class, "{label}"),
                    (expected, actual) => panic!("{label}: expected {expected:?}, got {actual:?}"),
                }
                parsed += 1;
            }
            "nes.stringify" => {
                let message = message(&record["input"]["message"]);
                let actual = message.encode();
                match (expected(record), &actual) {
                    (Ok(output), Ok(bytes)) => assert_eq!(bytes, &unhex(&output["hex"]), "{label}"),
                    (Err(class), Err(error)) => assert_eq!(error.js_class(), class, "{label}"),
                    (expected, actual) => panic!("{label}: expected {expected:?}, got {actual:?}"),
                }
                if name.starts_with("sent: ") {
                    let (frame, built) = constructed(name, &message);
                    assert_eq!(built.encode(), actual, "{label}: the {frame} constructor");
                    sent.insert(frame);
                }
                written += 1;
            }
            _ => {}
        }
    }
    assert!(parsed > 0 && written > 0, "{CLASS} has no nes records");
    assert_eq!(sent.len(), 6, "the frames the reference sends: {sent:?}");
}
