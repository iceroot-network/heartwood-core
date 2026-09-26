//! The golden vectors these tests read: vector files are JSONL, a meta record first, then one
//! record per case, `{"op", "network", "height", "name"?, "input", "output"}` or the same with
//! `"error": {"class", "message"}` in place of `"output"`. The devnet configuration they were made
//! under is the chain definition of `heartwood-crypto`'s genesis class (V10).

#![allow(dead_code)]

use std::sync::LazyLock;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::utils::hex;
use serde_json::Value;

/// V06 of `heartwood-crypto`, whose `fee.minimum` records this crate answers.
pub const V06: &str = include_str!("../../../crypto/tests/vectors/V06-arithmetic.jsonl");
/// V19 of this crate: the fee floor.
pub const V19: &str = include_str!("../vectors/V19-fee-floor.jsonl");
/// V10 of `heartwood-crypto`: the devnet chain definitions.
const V10: &str = include_str!("../../../crypto/tests/vectors/V10-genesis.jsonl");

/// One case of a vector file.
#[derive(Debug, Clone)]
pub struct Record {
    /// 1-based line number in the file.
    pub line: usize,
    /// The oracle operation.
    pub op: String,
    /// The rule height.
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
        match &self.name {
            Some(name) => format!("{class}:{} {} {name:?}", self.line, self.op),
            None => format!("{class}:{} {}", self.line, self.op),
        }
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
    /// Parse the text of the vector file of `class`; panics, naming the line, on a malformed file.
    pub fn parse(class: &str, text: &str) -> VectorFile {
        let mut lines = text.lines().enumerate();
        let (_, first) = lines.next().unwrap_or_else(|| panic!("{class}: empty"));
        let meta: Value = serde_json::from_str(first).unwrap_or_else(|e| panic!("{class}:1: {e}"));
        assert_eq!(meta["op"], "meta", "{class}:1: no meta record");
        assert_eq!(
            meta["class"], class,
            "{class}:1: meta record of another class"
        );
        let records: Vec<Record> = lines
            .map(|(index, line)| parse_record(class, index + 1, line))
            .collect();
        assert_eq!(
            meta["records"].as_u64(),
            Some(records.len() as u64),
            "{class}:1: the meta record counts another number of records"
        );
        VectorFile {
            class: class.to_owned(),
            meta,
            records,
        }
    }
}

fn parse_record(class: &str, line: usize, text: &str) -> Record {
    let value: Value = serde_json::from_str(text).unwrap_or_else(|e| panic!("{class}:{line}: {e}"));
    let expected = match (value.get("output"), value.get("error")) {
        (Some(output), None) => Ok(output.clone()),
        (None, Some(error)) => Err(error["class"].as_str().unwrap_or_default().to_owned()),
        _ => panic!("{class}:{line}: a record has exactly one of output and error"),
    };
    Record {
        line,
        op: value["op"]
            .as_str()
            .unwrap_or_else(|| panic!("{class}:{line}: no op"))
            .to_owned(),
        height: value["height"]
            .as_u64()
            .unwrap_or_else(|| panic!("{class}:{line}: no height")),
        name: value.get("name").and_then(Value::as_str).map(str::to_owned),
        input: value["input"].clone(),
        expected,
    }
}

/// The devnet configuration of the vector files.
pub struct Devnet {
    /// The network description.
    pub network: Network,
    /// The milestones, loaded.
    pub milestones: Milestones,
    /// SHA-256 of the milestone file, in hex.
    pub milestones_sha256: String,
}

static DEVNET: LazyLock<(String, String)> = LazyLock::new(|| {
    let file = VectorFile::parse("V10-genesis", V10);
    let record = file.records.first().expect("V10 has a chain definition");
    let files = &record.expected.as_ref().expect("a chain definition")["files"];
    let text = |name: &str| files[name].as_str().expect("a file").to_owned();
    (text("crypto/network.json"), text("crypto/milestones.json"))
});

impl Devnet {
    /// The devnet of the first chain definition in V10.
    pub fn load() -> Devnet {
        let (network, milestones) = &*DEVNET;
        let network = Network::load(network).expect("the devnet network loads");
        Devnet {
            milestones: Milestones::load(milestones, &network).expect("the devnet milestones load"),
            network,
            milestones_sha256: hex::encode(&sha256(milestones.as_bytes())),
        }
    }

    /// Check that `file` was made under this devnet.
    pub fn check(&self, file: &VectorFile) {
        assert_eq!(
            file.meta["network"]["milestonesSha256"].as_str(),
            Some(self.milestones_sha256.as_str()),
            "{}: made under other devnet milestones",
            file.class
        );
    }

    /// The milestones a record runs under: its own file (`input.milestones`), or the devnet's.
    pub fn milestones_of(&self, input: &Value) -> Result<Milestones, String> {
        match input.get("milestones") {
            None | Some(Value::Null) => Ok(self.milestones.clone()),
            Some(text) => {
                let text = text.as_str().ok_or("milestones is not text")?;
                Milestones::load(text, &self.network).map_err(|error| error.to_string())
            }
        }
    }
}
