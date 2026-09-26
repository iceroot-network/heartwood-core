//! Helpers shared by the integration tests: the golden vector files in `tests/vectors/` and the
//! devnet configuration they were generated under.
//!
//! A vector file is JSONL: a meta record first, then one record per case,
//! `{"op", "network", "height", "name"?, "input", "output"}`, or the same with `"error": {"class",
//! "message"}` in place of `"output"`. The files are compiled into the tests with `include_str!`,
//! so a missing file fails the build.

#![allow(dead_code)]

use std::sync::LazyLock;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::utils::{hex, js};
use serde_json::Value;

/// One case of a vector file.
#[derive(Debug, Clone)]
pub struct Record {
    /// 1-based line number in the file.
    pub line: usize,
    /// The oracle operation.
    pub op: String,
    /// The rule height the reference was set to.
    pub height: u64,
    /// The case label, if any.
    pub name: Option<String>,
    /// The operation's input.
    pub input: Value,
    /// The reference's output, or the class of the error it raised.
    pub expected: Result<Value, String>,
}

impl Record {
    /// `class:line op "name"`, for assertion messages.
    pub fn label(&self, class: &str) -> String {
        format!("{class}:{} {} {:?}", self.line, self.op, self.name)
    }
}

/// A parsed vector file.
#[derive(Debug, Clone)]
pub struct VectorFile {
    /// The vector class.
    pub class: String,
    /// The meta record.
    pub meta: Value,
    /// The cases, in file order.
    pub records: Vec<Record>,
}

impl VectorFile {
    /// Parse the text of the vector file of `class`. Panics, naming the line, on a malformed file.
    pub fn parse(class: &str, text: &str) -> VectorFile {
        let mut lines = text.lines().enumerate();
        let (_, first) = lines.next().expect("a meta record");
        let meta: Value = serde_json::from_str(first).expect("the meta record is JSON");
        assert_eq!(
            meta["op"], "meta",
            "{class}: the first record is not the meta record"
        );
        assert_eq!(
            meta["class"], class,
            "{class}: meta record of another class"
        );
        let records: Vec<Record> = lines
            .map(|(index, line)| {
                let value: Value = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("{class}:{}: not JSON: {e}", index + 1));
                let expected = match (value.get("output"), value.get("error")) {
                    (Some(output), None) => Ok(output.clone()),
                    (None, Some(error)) => {
                        Err(error["class"].as_str().expect("a class").to_owned())
                    }
                    _ => panic!("{class}:{}: neither output nor error", index + 1),
                };
                Record {
                    line: index + 1,
                    op: value["op"].as_str().expect("op").to_owned(),
                    height: value["height"].as_u64().expect("height"),
                    name: value.get("name").and_then(Value::as_str).map(str::to_owned),
                    input: value["input"].clone(),
                    expected,
                }
            })
            .collect();
        assert_eq!(meta["records"].as_u64(), Some(records.len() as u64));
        VectorFile {
            class: class.to_owned(),
            meta,
            records,
        }
    }
}

/// Load `tests/vectors/<class>.jsonl` (compiled in) as a [`VectorFile`].
#[allow(unused_macros)]
macro_rules! vector_file {
    ($class:literal) => {
        common::VectorFile::parse(
            $class,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/vectors/",
                $class,
                ".jsonl"
            )),
        )
    };
}

/// The devnet configuration of the vector files: the network and milestone files of the first
/// chain definition of `heartwood-crypto`'s genesis class, which the oracle ran under.
pub struct Devnet {
    /// The network description.
    pub network: Network,
    /// The milestones, as loaded.
    pub milestones: Milestones,
    /// SHA-256 of the milestone file, in hex.
    pub milestones_sha256: String,
}

pub static DEVNET: LazyLock<Devnet> = LazyLock::new(|| {
    let genesis = VectorFile::parse(
        "V10-genesis",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../crypto/tests/vectors/V10-genesis.jsonl"
        )),
    );
    let record = genesis.records.first().expect("a chain definition");
    let files = &record.expected.as_ref().expect("the chain definition")["files"];
    let text = |name: &str| files[name].as_str().expect(name).to_owned();
    let network = Network::load(&text("crypto/network.json")).expect("the devnet network loads");
    let milestones_text = text("crypto/milestones.json");
    let milestones = Milestones::load(&milestones_text, &network).expect("the milestones load");
    Devnet {
        network,
        milestones,
        milestones_sha256: hex::encode(&sha256(milestones_text.as_bytes())),
    }
});

/// Check that `file` was generated under the devnet milestones of [`DEVNET`].
pub fn check_devnet(file: &VectorFile) {
    assert_eq!(
        file.meta["network"]["milestonesSha256"].as_str(),
        Some(DEVNET.milestones_sha256.as_str()),
        "{}: generated under other devnet milestones",
        file.class
    );
}

/// The milestones a record runs under: its own milestone file, or the devnet's.
pub fn milestones(record: &Record) -> Milestones {
    match record.input.get("milestones") {
        None => DEVNET.milestones.clone(),
        Some(text) => Milestones::load(text.as_str().expect("milestones text"), &DEVNET.network)
            .expect("the record's milestones load"),
    }
}

/// Whether two JSON values are equal, numbers compared as the doubles `JSON.parse` reads.
pub fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => js::number_value(x) == js::number_value(y),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((kx, vx), (ky, vy))| kx == ky && same_json(vx, vy))
        }
        _ => a == b,
    }
}
