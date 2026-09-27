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
    /// A milestone file with a `dynamicFees` or `blocksToRevokeDelegateResignation` value of a
    /// type the reference coerces or fails on when it checks a fee or a revoke.
    FeeKeyValueType,
    /// The reason after "Invalid transaction data: " in the pool's answer to an entry that does
    /// not decode.
    DecodeReason,
    /// A full pool and a new transaction with a fee above the lowest, which the reference refuses
    /// only after it has evicted the lowest-priority transaction.
    FullPoolEviction,
    /// A restart from the reference's saved state with a delegate name that reads as a number.
    SavedStateUsername,
    /// A restart in a round whose round rows a revert to the last block of the round before
    /// deleted.
    RoundRowsAfterRevert,
    /// A count in a message, written by the reference in a default locale that is not English.
    Locale,
    /// A pool session whose online record changed while a sender's pool view that had read the
    /// delegate's wallet before the change lived on.
    OnlineView,
    /// Crypto files that the strict schema refuses: an unknown key, a `null`, a height twice, a
    /// removed feature turned on, a missing file, or a non-empty `exceptions.json`.
    StrictSchema,
    /// A peer's fork segment whose fetched headers do not chain (previous id, height + 1).
    UnlinkedForkSegment,
    /// A block refused in its apply after a gap of missed slots: the reference has written the
    /// gap's missed rows before it validated the block.
    MissedRowsOfRefusedBlock,
    /// A served route's documented difference.
    Api(ApiDifference),
}

/// A served route's documented difference from the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiDifference {
    /// A store error or a read's timeout on a list: the reference answers an empty 200 (a lookup
    /// an unlogged 500); Heartwood answers a logged 500.
    StoreFailure,
    /// A block height above the tip, or a block's transactions of a value that is neither a full
    /// id nor a height at or below the tip: the reference matches an id prefix; Heartwood answers
    /// 404 "Block not found".
    HeightAboveTip,
    /// Requests beyond the semaphore's bound: the reference's semaphore is inert; Heartwood's
    /// bounds the concurrency and answers 429 when its queue is full.
    Semaphore,
    /// The peers' `ip` filter: the reference validates it and ignores it; Heartwood filters.
    PeersIpFilter,
    /// Ties of a custom order and the order of text: the reference's depend on the database's
    /// plan and locale; Heartwood breaks ties by the unique key and orders text by bytes.
    TieOrder,
    /// Pagination links under another base path than `/api`: the reference strips a hard-coded
    /// `/api`; Heartwood strips the configured base path.
    BasePathLinks,
}

impl Documented {
    /// What the reference does instead, and why the difference is harmless or accepted.
    pub fn reason(self) -> &'static str {
        match self {
            Documented::PluginsPrototype => {
                "a status plugin named __proto__ with a value: the reference's decoder makes the \
                 plugin the prototype of the plugins object, and its reply check then refuses the \
                 plugin's properties; Heartwood drops the entry and checks the own names only \
                 (crafted replies only)"
            }
            Documented::DeepGroups => {
                "protobuf groups nested beyond the reference's stack: the reference fails with a \
                 stack overflow, Heartwood skips groups at any depth (no honest message has a \
                 group, and the reference's result depends on its stack size)"
            }
            Documented::WideAmount => {
                "an amount beyond 128 bits: the reference reads an integer of any size, refuses a \
                 getBlocks response at a later step and accepts the totals of a getStatus reply at \
                 height 1; Heartwood refuses the payload when it decodes (crafted payloads only; \
                 reading such an integer is unbounded work)"
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
            Documented::FeeKeyValueType => {
                "a fee table or revoke delay of a type the reference coerces or fails on later: \
                 Heartwood refuses the milestone file at load, the reference loads it (a \
                 configuration-only difference; generated devnets never contain one)"
            }
            Documented::DecodeReason => {
                "the reason text of a pool entry that does not decode: Heartwood names its own \
                 decoder's reason where the reference names its decoder's or its schema \
                 validator's; the code, the entry id, the place in the lists and the verdict are \
                 the same (malformed input only)"
            }
            Documented::FullPoolEviction => {
                "a full pool: the reference evicts the lowest-priority transaction before it \
                 checks the new one, so an invalid transaction with a higher fee evicts a valid \
                 one; Heartwood checks the new transaction first and evicts nothing when it \
                 refuses it, so its pool keeps the transaction the reference evicted"
            }
            Documented::SavedStateUsername => {
                "a delegate name such as 123, 0x10 or 1e5 after a restart from the reference's \
                 saved state: its reviver turns the name into a number, or makes the file \
                 unloadable; Heartwood's wallet records keep the name as text on every restart \
                 path"
            }
            Documented::RoundRowsAfterRevert => {
                "a restart after a revert to the last block of a round: the reference's revert \
                 deletes the next round's rows, which its running node saved with that block and \
                 does not save again, so its restart in that round reads no delegate for it and \
                 keeps an empty forging list, or, when the store holds no missed-block row, rolls \
                 the chain back; Heartwood deletes the same rows but restarts from its state, \
                 which holds the round"
            }
            Documented::Locale => {
                "a count in a message: the reference groups its digits in the process's default \
                 locale; Heartwood always writes the English grouping (node-local text only)"
            }
            Documented::OnlineView => {
                "a delegate's online record in a sender's pool view: the reference's view keeps \
                 the delegate's wallet, with its version, as it copied it the first time it read \
                 it, until the view is rebuilt; Heartwood's pool reads the node's online record \
                 when it checks (node-local policy, whose changes depend on the network's timing)"
            }
            Documented::StrictSchema => {
                "crypto files that the strict schema refuses: Heartwood refuses to start and names \
                 the file and key, the reference loads them or falls back to its built-in preset \
                 (a configuration-only difference; generated devnets never contain one)"
            }
            Documented::UnlinkedForkSegment => {
                "a fork segment whose headers do not chain: the reference checks each fetched \
                 header's signature, height and round delegate only, Heartwood also requires each \
                 to chain to the one before it and refuses the peer (crafted peers only; every \
                 honest segment chains)"
            }
            Documented::MissedRowsOfRefusedBlock => {
                "a block refused in its apply after missed slots: the reference writes the missed \
                 rows before it validates the block and keeps them, Heartwood writes missed rows \
                 only with an applied block (a faulty or adversarial forger only)"
            }
            Documented::Api(difference) => difference.reason(),
        }
    }
}

impl ApiDifference {
    /// What the reference does instead, and why the difference is accepted.
    pub fn reason(self) -> &'static str {
        match self {
            ApiDifference::StoreFailure => {
                "a store failure on a list: the reference answers an empty 200, Heartwood a logged \
                 500 with a Boom body (a fault or a timeout only)"
            }
            ApiDifference::HeightAboveTip => {
                "a height above the tip: the reference matches an id prefix and answers an \
                 unrelated block or a 500, Heartwood answers 404 \"Block not found\""
            }
            ApiDifference::Semaphore => {
                "the semaphore: the reference's is inert, Heartwood bounds the concurrency with \
                 the same settings and answers 429 when its queue is full (heavy load only)"
            }
            ApiDifference::PeersIpFilter => {
                "the peers' ip filter: the reference validates it and ignores it, Heartwood \
                 filters by it"
            }
            ApiDifference::TieOrder => {
                "ties of a custom order and text order: the reference's depend on its database's \
                 plan and locale, Heartwood breaks ties by the unique key and orders text by bytes"
            }
            ApiDifference::BasePathLinks => {
                "pagination links under another base path: the reference strips a hard-coded \
                 /api, Heartwood strips the configured base path"
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
    /// `true` note in the output's `notes` (`pluginsPrototype`, `stackOverflow`, `wideAmount`,
    /// `fullPoolEviction`, `savedStateUsername`, `roundRowsMissing`, `locale`), or, for deep
    /// groups, a `RangeError` for the overflow of the Node stack. A replaced prototype is shown
    /// only with the reply check's refusal of the plugins, the one step it changes. The
    /// differences that Heartwood checks on the input ([`Verdict::RefusedByDesign`]) are never
    /// shown.
    pub fn shown_by(self, expected: &Expected) -> bool {
        let note = |name: &str| match expected {
            Expected::Output(output) => output[NOTES][name] == Value::Bool(true),
            Expected::Error(_) => false,
        };
        match self {
            Documented::PluginsPrototype => {
                note("pluginsPrototype")
                    && matches!(expected, Expected::Output(output)
                        if output["valid"] == Value::Bool(false) && output["refused"] == "plugins")
            }
            Documented::DeepGroups => {
                note("stackOverflow")
                    || matches!(expected, Expected::Error(error)
                        if error.class == "RangeError" && error.message.starts_with(STACK_OVERFLOW))
            }
            Documented::WideAmount => note("wideAmount"),
            Documented::FullPoolEviction => note("fullPoolEviction"),
            Documented::SavedStateUsername => note("savedStateUsername"),
            Documented::RoundRowsAfterRevert => note("roundRowsMissing"),
            Documented::Locale => note("locale"),
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
    /// The operation's subject is registered but not written yet; the case is reported as having
    /// no Rust implementation, as an unregistered operation is.
    #[error("no Rust implementation yet")]
    NotImplemented,
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
