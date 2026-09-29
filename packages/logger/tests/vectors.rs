//! The logger's lines against the reference (vector class V20-logger): every level, coloured and
//! plain, with emoji kept and removed, edge times, and the whole emoji table.
//!
//! A message with a character the logger escapes is the one documented difference: the reference
//! writes it as it is, over several lines when it holds line breaks. Its line is checked against
//! the reference's up to the level, then the escaped message, cyan when coloured.

use std::borrow::Cow;

use heartwood_logger::Level;
use heartwood_logger::format::{emojify, escape, line};
use serde_json::Value;

const V20: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/V20-logger.jsonl"
));

#[test]
fn v20_logger() {
    let mut lines = V20.lines();
    let meta: Value = serde_json::from_str(lines.next().expect("a meta record")).expect("JSON");
    assert_eq!(meta["class"], "V20-logger");
    let (mut count, mut escaped) = (0, 0);
    let mut failures = Vec::new();
    for (index, text) in lines.enumerate() {
        let record: Value = serde_json::from_str(text).expect("JSON");
        assert_eq!(record["op"], "logger.format");
        let input = &record["input"];
        let level = Level::from_name(input["level"].as_str().expect("a level")).expect("a level");
        let raw = input["message"].as_str().expect("a message");
        let message = emojify(raw, flag(input, "emojify"));
        let colour = flag(input, "colorize");
        let actual = if raw.is_empty() {
            None
        } else {
            line(
                level,
                input["time"].as_i64().expect("a time"),
                &message,
                colour,
            )
        };
        let mut expected = record["output"]["line"].as_str().map(str::to_owned);
        if let (Cow::Owned(message), Some(reference)) = (escape(&message), &expected) {
            let head = &reference[..reference.find(": ").expect("a level") + 2];
            expected = Some(if colour {
                format!("{head}\u{1b}[36m{message}\u{1b}[39m\n")
            } else {
                format!("{head}{message}\n")
            });
            escaped += 1;
        }
        if actual != expected {
            failures.push(format!(
                "V20-logger:{} {}: expected {expected:?}, got {actual:?}",
                index + 2,
                record["name"]
            ));
        }
        count += 1;
    }
    assert_eq!(meta["records"].as_u64(), Some(count));
    // Line breaks, colour codes and a line separator in the message, at five levels, four ways.
    assert_eq!(escaped, 60, "records with a character the logger escapes");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn flag(input: &Value, key: &str) -> bool {
    input[key].as_bool().expect("a boolean")
}
