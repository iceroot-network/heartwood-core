//! Rust-side implementations of oracle operations ("subjects") and their registry.
//!
//! A subject answers one oracle operation (`keys.fromPassphrase`, `tx.build`, ...) with
//! Heartwood's code. The same subject serves L0 (a golden-vector record) and L1 (a random case
//! sent to `oracle-server.js`), because both describe a case as operation, network, height and
//! input. [`heartwood`] registers the subjects of [`crate::subjects`], which answer with
//! `heartwood-crypto`.

use std::collections::BTreeMap;

use serde_json::Value;
use thiserror::Error;

use crate::errors::Error;
use crate::subjects::{self, Devnet};

/// Heartwood's answer to one case.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Accepted, with the output in the oracle's JSON shape.
    Output(Value),
    /// Rejected. `class` is Heartwood's error class; classes are compared only where they are
    /// observable, and messages never are.
    Rejected { class: String },
    /// Rejected on purpose where the reference accepts: a documented difference. `reason` says
    /// what the reference does instead. When the reference rejects the case too, the verdicts
    /// agree.
    RefusedByDesign { class: String, reason: String },
}

/// A subject could not produce a verdict.
#[derive(Debug, Error)]
pub enum SubjectError {
    /// The input uses something this subject does not model yet; the case is skipped, not failed.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// The subject itself failed (a harness bug, not a Heartwood verdict).
    #[error("{0}")]
    Failed(String),
}

/// One case as both levels describe it.
#[derive(Debug, Clone, Copy)]
pub struct Case<'a> {
    pub op: &'a str,
    pub network: &'a str,
    pub height: u64,
    pub input: &'a Value,
}

/// A Rust implementation of one oracle operation.
pub trait Subject: Send + Sync {
    /// The oracle operation this subject answers.
    fn op(&self) -> &'static str;
    /// Run the case with Heartwood's code.
    fn run(&self, case: &Case<'_>) -> Result<Verdict, SubjectError>;
}

/// The subjects available in a build, by operation.
#[derive(Default)]
pub struct Registry {
    subjects: BTreeMap<&'static str, Box<dyn Subject>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    pub fn register(&mut self, subject: Box<dyn Subject>) -> Result<(), Error> {
        let op = subject.op();
        if self.subjects.contains_key(op) {
            return Err(Error::DuplicateSubject(op.to_string()));
        }
        self.subjects.insert(op, subject);
        Ok(())
    }

    pub fn get(&self, op: &str) -> Option<&dyn Subject> {
        self.subjects.get(op).map(|s| s.as_ref())
    }

    pub fn ops(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.subjects.keys().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.subjects.is_empty()
    }
}

/// Heartwood's subjects, one per oracle operation that `heartwood-crypto` implements.
///
/// `devnet` is the configuration the cases were generated under (see [`Devnet::from_vectors`]).
/// Without it, the subjects that need the devnet's rules skip their cases.
pub fn heartwood(devnet: Option<Devnet>) -> Registry {
    let mut registry = Registry::new();
    subjects::register(&mut registry, devnet);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Subject for Echo {
        fn op(&self) -> &'static str {
            "echo"
        }
        fn run(&self, case: &Case<'_>) -> Result<Verdict, SubjectError> {
            Ok(Verdict::Output(case.input.clone()))
        }
    }

    #[test]
    fn registry_finds_subjects_and_refuses_duplicates() {
        let mut r = Registry::new();
        assert!(r.is_empty());
        r.register(Box::new(Echo)).unwrap();
        assert!(r.get("echo").is_some());
        assert!(r.get("other").is_none());
        assert_eq!(r.ops().collect::<Vec<_>>(), vec!["echo"]);
        assert!(
            matches!(r.register(Box::new(Echo)), Err(Error::DuplicateSubject(op)) if op == "echo")
        );
    }

    #[test]
    fn heartwood_registers_the_crate_operations() {
        let registry = heartwood(None);
        let ops: Vec<&str> = registry.ops().collect();
        for op in [
            "keys.fromPassphrase",
            "sig.bip340.sign",
            "sig.legacy.verify",
            "tx.build",
            "tx.fromBytes",
            "block.make",
            "config.load",
            "genesis.generate",
        ] {
            assert!(ops.contains(&op), "{op} is not registered");
        }
    }
}
