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

use heartwood_crypto::Transaction;
use heartwood_crypto::utils::js::{number_to_string, number_value, property_order};
use heartwood_kernel::clock::Clock;
use heartwood_kernel::utils::pluralise::pluralise;
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::dynamic_fee_matcher::check_broadcast;
use crate::errors::{AddError, PoolCode, Refusal, StorageError};
use crate::service::{Pool, PoolContext};
use crate::verifier::{Checked, check_entries};

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
    /// `errors` (`{id: {type, message}}`) when something was refused, its keys in the order
    /// JavaScript enumerates them (entry indexes first, in numeric order, then the others in the
    /// order they were first refused).
    pub fn to_json(&self) -> Value {
        let list = |ids: &[String]| Value::from(ids.to_vec());
        let mut out = Map::new();
        out.insert("accept".into(), list(&self.accept));
        out.insert("broadcast".into(), list(&self.broadcast));
        out.insert("invalid".into(), list(&self.invalid));
        out.insert("excess".into(), list(&self.excess));
        if let Some(errors) = &self.errors {
            let errors: Map<String, Value> = errors
                .iter()
                .map(|(id, refusal)| {
                    let mut error = Map::new();
                    error.insert("type".into(), Value::from(refusal.code.as_str()));
                    error.insert("message".into(), Value::from(refusal.message.as_str()));
                    (id.clone(), Value::Object(error))
                })
                .collect();
            let ordered = property_order(&errors)
                .into_iter()
                .map(|(id, error)| (id.clone(), error.clone()))
                .collect();
            out.insert("errors".into(), Value::Object(ordered));
        }
        Value::Object(out)
    }

    /// Record the refusal of entry `id`.
    fn refuse(&mut self, id: String, refusal: Refusal) {
        if refusal.code == PoolCode::ExceedsMaxCount {
            self.excess.push(id.clone());
        }
        self.invalid.push(id.clone());
        // A repeated entry id keeps its first place and its last refusal, as a JavaScript object.
        self.errors.get_or_insert_default().insert(id, refusal);
    }
}

/// A processor, with its cooldown: the time in seconds at which each id was last seen.
#[derive(Debug, Clone, Default)]
pub struct Processor {
    cooldown: IndexMap<String, i64>,
}

impl Processor {
    /// A processor with an empty cooldown.
    pub fn new() -> Processor {
        Processor::default()
    }

    /// Process `entries` into `pool` at the time `clock` shows.
    ///
    /// The first loop decodes every entry ([`check_entries`], on the pool's `worker_count`
    /// threads), then refuses in entry order each id seen in the last 30 seconds and records the
    /// others as seen now. The second loop adds the rest in order. A storage failure is fatal
    /// and ends the batch.
    pub fn process(
        &mut self,
        pool: &mut Pool,
        ctx: &PoolContext<'_>,
        clock: &dyn Clock,
        entries: Vec<Entry>,
    ) -> Result<ProcessorResult, StorageError> {
        let processed = self.process_to_broadcast(pool, ctx, clock, entries);
        processed.map(|(result, _)| result)
    }

    /// [`Processor::process`], with the admitted transactions that pass the broadcast fee rule,
    /// in the result's order: the reference keeps them as they were admitted and broadcasts them
    /// after the batch, so one that a later entry of the batch evicts is broadcast too.
    pub fn process_to_broadcast(
        &mut self,
        pool: &mut Pool,
        ctx: &PoolContext<'_>,
        clock: &dyn Clock,
        entries: Vec<Entry>,
    ) -> Result<(ProcessorResult, Vec<Transaction>), StorageError> {
        let ms = clock.now_ms();
        let now = ms
            .div_euclid(1000)
            .saturating_add(i64::from(ms.rem_euclid(1000) != 0));
        self.cooldown
            .retain(|_, seen| now.saturating_sub(*seen) < COOLDOWN_SECONDS);
        let network = ctx.network.pub_key_hash;
        let checked = check_entries(&entries, ctx.params(), pool.options().worker_count);
        let (mut result, mut admitted, mut to_broadcast) =
            (ProcessorResult::default(), Vec::new(), Vec::new());
        for (index, (entry, checked)) in entries.iter().zip(checked).enumerate() {
            let entry_id = entry_id(entry, index);
            let checked = match checked {
                Ok(checked) => checked,
                Err(refusal) => {
                    result.refuse(entry_id, refusal);
                    continue;
                }
            };
            let id = checked.id();
            match self.cooldown.get(&id) {
                Some(seen) => {
                    let wait = COOLDOWN_SECONDS.saturating_sub(now.saturating_sub(*seen));
                    let message = format!(
                        "{} must wait another {} before trying again",
                        checked.display(network),
                        pluralise("second", wait.into(), true)
                    );
                    result.refuse(entry_id, Refusal::new(PoolCode::Cooldown, message));
                }
                None => {
                    self.cooldown.insert(id, now);
                    admitted.push(checked);
                }
            }
        }
        for checked in admitted {
            let entry_id = checked.id();
            let broadcast = match &checked {
                Checked::Decoded(transaction) => check_broadcast(ctx, pool.options(), transaction)
                    .is_ok()
                    .then(|| Transaction::clone(transaction)),
                Checked::Removed { .. } => None,
            };
            match pool.add_transaction(ctx, checked) {
                Ok(()) => {
                    if let Some(transaction) = broadcast {
                        result.broadcast.push(entry_id.clone());
                        to_broadcast.push(transaction);
                    }
                    result.accept.push(entry_id);
                }
                Err(AddError::Refused(refusal)) => result.refuse(entry_id, refusal),
                Err(AddError::Storage(error)) => return Err(error),
            }
        }
        Ok((result, to_broadcast))
    }
}

/// The id of entry `index` in the first loop: the index for bytes; for JSON its `id` as
/// JavaScript's `String` writes it, or the index when it has none or `null`.
fn entry_id(entry: &Entry, index: usize) -> String {
    match entry {
        Entry::Data(Value::Object(object)) => match object.get("id") {
            None | Some(Value::Null) => index.to_string(),
            Some(id) => js_string(id),
        },
        Entry::Data(_) | Entry::Bytes(_) => index.to_string(),
    }
}

/// `value` as JavaScript's `String(value)` writes it.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number_to_string(number_value(number)),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_string(item),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}
