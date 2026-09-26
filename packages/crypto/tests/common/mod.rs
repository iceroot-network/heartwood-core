//! Helpers shared by the integration tests: the golden vector files in `tests/vectors/`.
//!
//! A vector file is JSONL: a meta record first, then one record per case,
//! `{"op", "network", "height", "name"?, "input", "output"}`, or the same with `"error": {"class",
//! "message"}` in place of `"output"`. A test file loads one with
//!
//! ```text
//! #[macro_use]
//! mod common;
//!
//! let file = vector_file!("V01-keys-addresses");
//! for record in file.op("keys.fromPassphrase") { ... }
//! ```
//!
//! The file is compiled into the test with `include_str!`, so a missing file fails the build.

#![allow(dead_code)]

use serde_json::Value;

/// One case of a vector file.
#[derive(Debug, Clone)]
pub struct Record {
    /// 1-based line number in the file.
    pub line: usize,
    /// The oracle operation, for example `tx.build`.
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
        match &self.name {
            Some(name) => format!("{class}:{} {} {name:?}", self.line, self.op),
            None => format!("{class}:{} {}", self.line, self.op),
        }
    }
}

/// A parsed vector file.
#[derive(Debug, Clone)]
pub struct VectorFile {
    /// The vector class, for example `V01-keys-addresses`.
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
        let (_, first) = lines
            .next()
            .unwrap_or_else(|| panic!("{class}: empty vector file"));
        let meta: Value = serde_json::from_str(first)
            .unwrap_or_else(|e| panic!("{class}:1: meta record is not JSON: {e}"));
        assert_eq!(meta["op"], "meta", "{class}:1: the first record is not the meta record");
        assert_eq!(meta["class"], class, "{class}:1: meta record of another class");

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

    /// The records of operation `op`, in file order.
    pub fn op<'a>(&'a self, op: &'a str) -> impl Iterator<Item = &'a Record> + 'a {
        self.records.iter().filter(move |record| record.op == op)
    }
}

fn parse_record(class: &str, line: usize, text: &str) -> Record {
    let value: Value =
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{class}:{line}: not JSON: {e}"));
    let field = |key: &str| {
        value
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("{class}:{line}: no {key:?}"))
    };
    let op = field("op")
        .as_str()
        .unwrap_or_else(|| panic!("{class}:{line}: op is not a string"))
        .to_owned();
    let height = field("height")
        .as_u64()
        .unwrap_or_else(|| panic!("{class}:{line}: height is not an integer"));
    let name = value.get("name").and_then(Value::as_str).map(str::to_owned);
    let expected = match (value.get("output"), value.get("error")) {
        (Some(output), None) => Ok(output.clone()),
        (None, Some(error)) => Err(error["class"]
            .as_str()
            .unwrap_or_else(|| panic!("{class}:{line}: error.class is not a string"))
            .to_owned()),
        _ => panic!("{class}:{line}: a record has exactly one of output and error"),
    };
    Record {
        line,
        op,
        height,
        name,
        input: field("input"),
        expected,
    }
}

/// Load `tests/vectors/<class>.jsonl` (compiled in) as a [`VectorFile`].
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
