//! Rust-side implementations of oracle operations ("subjects") and their registry.
//!
//! A subject answers one oracle operation (`keys.fromPassphrase`, `tx.build`, ...) with
//! Heartwood's code. The same subject serves L0 (a golden-vector record) and L1 (a random case
//! sent to `oracle-server.js`), because both describe a case as operation, network, height and
//! input. [`heartwood`] registers the subjects of [`crate::subjects`], which answer with
//! Heartwood's crates.

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
    /// Rejected. `class` is the reference's error class for this refusal where the subject models
    /// it, and L0 then compares it with the oracle's; `None` where the class is not observable (the
    /// reference raises a generic error, or refuses at another stage), and only the verdict is
    /// compared. `detail` names Heartwood's error, for reports.
    Rejected {
        /// The reference's error class, when it is observable.
        class: Option<&'static str>,
        /// Heartwood's error.
        detail: String,
    },
    /// Rejected on purpose where the reference accepts: a documented difference. `holds` says
    /// whether the input really has the documented form, checked on the input itself; L0 fails the
    /// case when it does not. When the reference rejects the case too, the verdicts agree.
    RefusedByDesign {
        /// The documented difference.
        difference: Documented,
        /// Whether the input has the documented form.
        holds: bool,
    },
}

/// A form of input that Heartwood refuses on purpose where the reference accepts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Documented {
    /// A transaction of a removed type (type group 1, types 0, 3, 4, 5, 8, 9 and 10).
    RemovedType,
    /// A transaction with the extended header (header type 1), which carries the sender address.
    ExtendedHeader,
    /// A transaction with 65-byte multi-signature entries after its signatures.
    MultiSignature,
    /// A transaction whose memo is not valid UTF-8.
    NonUtf8Memo,
    /// A transaction in JSON form whose `asset.votes` is a non-empty array or string.
    VotesWithIndices,
    /// A milestone file with a `dynamicFees` or `blocksToRevokeDelegateResignation` value of a
    /// type the reference coerces or fails on when it checks a fee or a revoke.
    FeeKeyValueType,
}

impl Documented {
    /// What the reference does instead, and why the difference is harmless or accepted.
    pub fn reason(self) -> &'static str {
        match self {
            Documented::RemovedType => {
                "a removed type: the reference decodes it and refuses it when it is applied (its \
                 handler is inactive), so a block that holds one is refused either way"
            }
            Documented::ExtendedHeader => {
                "the extended header: the reference decodes it and refuses it when it is applied, \
                 so a block that holds one is refused either way"
            }
            Documented::MultiSignature => {
                "multi-signature entries after the signatures: the reference reads them and \
                 refuses the transaction when it is applied (no wallet can register a \
                 multi-signature), so a block that holds one is refused either way"
            }
            Documented::NonUtf8Memo => {
                "a memo that is not valid UTF-8: the reference reads it as replacement characters \
                 and accepts it, with the id and signature of the replaced text, in a block too; \
                 a known consensus difference (the copy with the replacement characters has the \
                 same ids, and both accept it)"
            }
            Documented::VotesWithIndices => {
                "asset.votes as a non-empty array or string in JSON: the reference writes its \
                 indices as vote names and removes them after it has computed the id (the JSON \
                 path only; the wire form is exact)"
            }
            Documented::FeeKeyValueType => {
                "a fee table or revoke delay of a type the reference coerces or fails on later: \
                 Heartwood refuses the milestone file at load, the reference loads it (a \
                 configuration-only difference; generated devnets never contain one)"
            }
        }
    }
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
    /// The oracle operation.
    pub op: &'a str,
    /// The network the case runs on.
    pub network: &'a str,
    /// The rule height.
    pub height: u64,
    /// The operation's input, in the oracle's JSON shape.
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
    /// An empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Add `subject`; an operation can have one subject only.
    pub fn register(&mut self, subject: Box<dyn Subject>) -> Result<(), Error> {
        let op = subject.op();
        if self.subjects.contains_key(op) {
            return Err(Error::DuplicateSubject(op.to_string()));
        }
        self.subjects.insert(op, subject);
        Ok(())
    }

    /// The subject of `op`, if one is registered.
    pub fn get(&self, op: &str) -> Option<&dyn Subject> {
        self.subjects.get(op).map(|s| s.as_ref())
    }

    /// The registered operations, in order.
    pub fn ops(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.subjects.keys().copied()
    }

    /// Whether no subject is registered.
    pub fn is_empty(&self) -> bool {
        self.subjects.is_empty()
    }
}

/// Heartwood's subjects, one per oracle operation that Heartwood implements.
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
