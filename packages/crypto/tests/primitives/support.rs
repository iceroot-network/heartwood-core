//! Typed access to the fields of a vector record, with the record named in every failure.

use std::collections::BTreeMap;

use heartwood_crypto::utils::hex;
use serde_json::Value;

use crate::common::{Record, VectorFile};

/// A record of a vector class.
#[derive(Clone, Copy)]
pub struct Case<'a> {
    pub class: &'a str,
    pub record: &'a Record,
}

impl<'a> Case<'a> {
    /// `class:line op "name"`.
    pub fn label(&self) -> String {
        self.record.label(self.class)
    }

    /// The case label, or an empty text.
    pub fn name(&self) -> &'a str {
        self.record.name.as_deref().unwrap_or("")
    }

    /// The input field `key` as text.
    pub fn text(&self, key: &str) -> &'a str {
        self.record.input[key]
            .as_str()
            .unwrap_or_else(|| panic!("{}: input.{key} is not a string", self.label()))
    }

    /// The input field `key`, hex decoded.
    pub fn bytes(&self, key: &str) -> Vec<u8> {
        hex::decode(self.text(key))
            .unwrap_or_else(|e| panic!("{}: input.{key} is not hex: {e}", self.label()))
    }

    /// The input field `key` as a byte value, if present.
    pub fn optional_byte(&self, key: &str) -> Option<u8> {
        self.record.input.get(key).map(|value| {
            value
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .unwrap_or_else(|| panic!("{}: input.{key} is not a byte", self.label()))
        })
    }

    /// The expected output; the record must not expect an error.
    pub fn output(&self) -> &'a Value {
        match &self.record.expected {
            Ok(output) => output,
            Err(class) => panic!("{}: expects the error {class}", self.label()),
        }
    }

    /// The output field `key` as text.
    pub fn output_text(&self, key: &str) -> &'a str {
        self.output()[key]
            .as_str()
            .unwrap_or_else(|| panic!("{}: output.{key} is not a string", self.label()))
    }

    /// The output field `valid`.
    pub fn valid(&self) -> bool {
        self.output()["valid"]
            .as_bool()
            .unwrap_or_else(|| panic!("{}: output.valid is not a boolean", self.label()))
    }
}

/// The records of `op` in `file`, as cases.
pub fn cases<'a>(file: &'a VectorFile, op: &'a str) -> impl Iterator<Item = Case<'a>> + 'a {
    file.op(op).map(|record| Case {
        class: &file.class,
        record,
    })
}

/// The network byte the reference was configured with when it wrote `file`.
pub fn configured_network(file: &VectorFile) -> u8 {
    file.meta["network"]["pubKeyHash"]
        .as_u64()
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or_else(|| panic!("{}: meta.network.pubKeyHash is not a byte", file.class))
}

/// Records per operation in `file`.
pub fn op_counts(file: &VectorFile) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for record in &file.records {
        *counts.entry(record.op.as_str()).or_default() += 1;
    }
    counts
}

/// `ops`, split into the operations this build checks and the legacy Schnorr ones it leaves out
/// when the feature `legacy-schnorr` is off, each of those with the reason.
pub fn split_legacy<'a>(ops: &[&'a str]) -> (Vec<&'a str>, Vec<(&'a str, &'static str)>) {
    let legacy = |op: &&str| op.starts_with("sig.legacy.");
    if cfg!(feature = "legacy-schnorr") {
        (ops.to_vec(), Vec::new())
    } else {
        let checked = ops.iter().filter(|op| !legacy(op)).copied().collect();
        let skipped = ops
            .iter()
            .filter(|op| legacy(op))
            .map(|&op| (op, "needs the feature legacy-schnorr"))
            .collect();
        (checked, skipped)
    }
}

/// Checks that every record of `file` has an operation in `checked` or in `elsewhere`, and that
/// each listed operation occurs. `elsewhere` names operations another test file owns, each with
/// the reason.
pub fn assert_coverage(file: &VectorFile, checked: &[&str], elsewhere: &[(&str, &str)]) {
    let counts = op_counts(file);
    for (op, count) in &counts {
        let known = checked.contains(op) || elsewhere.iter().any(|(other, _)| other == op);
        assert!(
            known,
            "{}: {count} records of {op} are not checked",
            file.class
        );
    }
    for op in checked.iter().chain(elsewhere.iter().map(|(op, _)| op)) {
        assert!(
            counts.contains_key(op),
            "{}: no records of {op}",
            file.class
        );
    }
}
