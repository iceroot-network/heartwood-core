//! The processor: a batch of entries from the API or a peer, decoded and added in two loops, with
//! the processor's own cooldown.
//!
//! Everything runs under the rules of the tip plus one, at `now = ⌈clock / 1000⌉` seconds.
//! Cooldown entries at least 30 seconds old are dropped first. The first loop decodes each entry
//! ([`crate::verifier`]) and refuses an id seen in the last 30 seconds (`ERR_COOLDOWN`); the
//! entry id is the entry's index for bytes, and `id` (or the index) for JSON. The second loop
//! adds each decoded transaction to the pool ([`crate::Pool::add_transaction`]); the entry id is
//! then the transaction id. A refused entry goes to `invalid` and `errors`, first-loop refusals
//! first; an accepted one to `accept`, and to `broadcast` when the broadcast fee rule passes.
//!
//! The API and P2P each own a processor, as in the reference, so each has its own cooldown.

use heartwood_kernel::clock::Clock;
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::errors::{Refusal, StorageError};
use crate::service::{Pool, PoolContext};

/// How long the processor refuses an id it has seen, in seconds.
pub const COOLDOWN_SECONDS: i64 = 30;

/// One entry of a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A transaction in the reference's JSON form.
    Data(Value),
    /// A transaction's bytes.
    Bytes(Vec<u8>),
}

/// The result of a batch, as the reference's processor returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessorResult {
    /// The entry ids of the admitted transactions, in order.
    pub accept: Vec<String>,
    /// The entry ids of the admitted transactions that pass the broadcast fee rule.
    pub broadcast: Vec<String>,
    /// The entry ids of the refused entries, first-loop refusals first.
    pub invalid: Vec<String>,
    /// The entry ids refused with `ERR_EXCEEDS_MAX_COUNT`.
    pub excess: Vec<String>,
    /// Each refused entry's code and message, by entry id; `None` when nothing was refused.
    pub errors: Option<IndexMap<String, Refusal>>,
}

impl ProcessorResult {
    /// The result in the reference's JSON form: `accept`, `broadcast`, `invalid`, `excess`, and
    /// `errors` (`{id: {type, message}}`) when something was refused.
    pub fn to_json(&self) -> Value {
        let list = |ids: &[String]| Value::from(ids.to_vec());
        let mut out = Map::new();
        out.insert("accept".into(), list(&self.accept));
        out.insert("broadcast".into(), list(&self.broadcast));
        out.insert("invalid".into(), list(&self.invalid));
        out.insert("excess".into(), list(&self.excess));
        if let Some(errors) = &self.errors {
            let errors = errors
                .iter()
                .map(|(id, refusal)| {
                    let mut error = Map::new();
                    error.insert("type".into(), Value::from(refusal.code.as_str()));
                    error.insert("message".into(), Value::from(refusal.message.as_str()));
                    (id.clone(), Value::Object(error))
                })
                .collect();
            out.insert("errors".into(), Value::Object(errors));
        }
        Value::Object(out)
    }
}

/// A processor, with its cooldown: the time in seconds at which each id was last seen.
#[derive(Debug, Clone, Default)]
pub struct Processor {
    #[expect(dead_code, reason = "the processor's loops read it")]
    cooldown: IndexMap<String, i64>,
}

impl Processor {
    /// A processor with an empty cooldown.
    pub fn new() -> Processor {
        Processor::default()
    }

    /// Process `entries` into `pool` at the time `clock` shows.
    pub fn process(
        &mut self,
        pool: &mut Pool,
        ctx: &PoolContext<'_>,
        clock: &dyn Clock,
        entries: Vec<Entry>,
    ) -> Result<ProcessorResult, StorageError> {
        let _ = (pool, ctx, clock, entries);
        todo!("the two loops")
    }
}
