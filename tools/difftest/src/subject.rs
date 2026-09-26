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
use crate::vectors::Expected;

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
    /// Heartwood took the path of a documented difference whose reference side only the oracle's
    /// answer shows (a JavaScript prototype, the depth of the Node stack, an integer of any size).
    /// When the oracle's answer shows it ([`Documented::shown_by`]), the case is a documented
    /// difference; otherwise `verdict` is judged as usual.
    DiffersWhereShown {
        /// The documented difference.
        difference: Documented,
        /// Heartwood's verdict, judged when the oracle's answer does not show the difference.
        verdict: Box<Verdict>,
    },
}

/// A form of input that Heartwood treats differently from the reference on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Documented {
    /// A getStatus reply with a plugin named `__proto__`.
    PluginsPrototype,
    /// Protobuf groups nested deeper than the reference's stack allows.
    DeepGroups,
    /// An amount in a peer-to-peer payload whose value needs more than 128 bits.
    WideAmount,
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
}

impl Documented {
    /// What the reference does instead, and why the difference is harmless or accepted.
    pub fn reason(self) -> &'static str {
        match self {
            Documented::PluginsPrototype => {
                "a status plugin named __proto__: the reference's decoder replaces the plugins \
                 object's prototype, and its reply check then sees the prototype's properties; \
                 Heartwood drops the entry and checks the own names only (crafted replies only)"
            }
            Documented::DeepGroups => {
                "protobuf groups nested beyond the reference's stack: the reference fails with a \
                 stack overflow, Heartwood skips groups at any depth (no honest message has a \
                 group, and the reference's result depends on its stack size)"
            }
            Documented::WideAmount => {
                "an amount beyond 128 bits: the reference reads an integer of any size and \
                 refuses it at a later step, Heartwood refuses it when it decodes (crafted \
                 payloads only; reading it is unbounded work)"
            }
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
        }
    }
}

/// The key of an oracle output that holds the oracle's notes: what it observed of the reference
/// that the output's value cannot show, for [`Documented::shown_by`]. L0 does not compare it.
pub const NOTES: &str = "notes";

/// The start of the message of the `RangeError` that Node raises when its stack overflows.
const STACK_OVERFLOW: &str = "Maximum call stack size exceeded";

impl Documented {
    /// Whether the oracle's answer `expected` shows the reference's side of this difference: a
    /// `true` note in the output's `notes` (`pluginsPrototype`, `stackOverflow`, `wideAmount`),
    /// or, for deep groups, a `RangeError` for the overflow of the Node stack. The differences
    /// that Heartwood checks on the input ([`Verdict::RefusedByDesign`]) are never shown.
    pub fn shown_by(self, expected: &Expected) -> bool {
        let note = |name: &str| match expected {
            Expected::Output(output) => output[NOTES][name] == Value::Bool(true),
            Expected::Error(_) => false,
        };
        match self {
            Documented::PluginsPrototype => note("pluginsPrototype"),
            Documented::DeepGroups => {
                note("stackOverflow")
                    || matches!(expected, Expected::Error(error)
                        if error.class == "RangeError" && error.message.starts_with(STACK_OVERFLOW))
            }
            Documented::WideAmount => note("wideAmount"),
            _ => false,
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
