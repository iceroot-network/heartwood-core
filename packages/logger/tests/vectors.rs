//! The logger's lines against the reference (vector class V20-logger): every level, coloured and
//! plain, with emoji kept and removed, edge times, and the whole emoji table.

use heartwood_logger::Level;
use heartwood_logger::format::{emojify, line};
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
    let mut count = 0;
    let mut failures = Vec::new();
    for (index, text) in lines.enumerate() {
        let record: Value = serde_json::from_str(text).expect("JSON");
        assert_eq!(record["op"], "logger.format");
        let input = &record["input"];
        let level = Level::from_name(input["level"].as_str().expect("a level")).expect("a level");
        let message = input["message"].as_str().expect("a message");
        let flag = |key: &str| input[key].as_bool().expect("a boolean");
        let actual = if message.is_empty() {
            None
        } else {
            let message = emojify(message, flag("emojify"));
            line(
                level,
                input["time"].as_i64().expect("a time"),
                &message,
                flag("colorize"),
            )
        };
        let expected = record["output"]["line"].as_str().map(str::to_owned);
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
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
