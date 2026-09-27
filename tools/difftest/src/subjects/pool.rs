//! The transaction pool: a whole pool session over a wallet fixture (`pool.run`), step by step:
//! batches of entries processed at a given time, the online record, re-adds, clean-ups, removals,
//! flushes, collations, a restart of the pool's storage and moves of the tip, with the pool's
//! contents in the reference's orders after every step. [`crate::levels::pool`] judges the
//! answer against the oracle's.
//!
//! The oracle's other pool operations (`pool.open` and the steps one by one) keep a session
//! between calls; the synthetic pool cases drive them ([`crate::synthetic_pool`]), and they have no
//! subject here.

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::{Milestones, Network, Transaction, TransactionId};
use heartwood_kernel::clock::ManualClock;
use heartwood_pool::{DelegateVersions, Entry, Options, Pool, PoolContext, Processor, collate};
use heartwood_state::WalletRepository;
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed, integer};
use crate::fixture::Fixture;
use crate::levels::pool::{VERSION, contents, options};
use crate::store::{ScratchDir, Unbuilt, guarded};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("pool.run", run)];

/// A pool session: the pool, its two processors, the online record and the tip.
struct Session<'a> {
    options: Options,
    pool: Pool,
    api: Processor,
    p2p: Processor,
    online: DelegateVersions,
    tip: u32,
    wallets: WalletRepository,
    milestones: &'a Milestones,
    network: &'a Network,
}

impl Session<'_> {
    fn decode(&self, text: &str) -> Result<Transaction, String> {
        let bytes = heartwood_crypto::utils::hex::decode(text).map_err(|e| e.to_string())?;
        let params = self.milestones.at(self.tip.saturating_add(1));
        Transaction::decode(&bytes, params).map_err(|e| format!("a step's transaction: {e}"))
    }

    fn online(&self) -> Value {
        let mut keys: Vec<String> = self
            .online
            .entries()
            .into_iter()
            .map(|e| e.public_key.to_hex())
            .collect();
        keys.sort();
        json!(keys)
    }

    /// Run one step; its result in the oracle's shape.
    fn step(&mut self, step: &Value) -> Result<Value, String> {
        let op = step
            .get("op")
            .and_then(Value::as_str)
            .ok_or("a step has no op")?;
        let ctx = PoolContext {
            wallets: &self.wallets,
            tip: self.tip,
            milestones: self.milestones,
            network: self.network,
            online: &self.online,
        };
        let storage = |e: heartwood_pool::StorageError| e.to_string();
        Ok(match op {
            "process" => {
                let now = step
                    .get("now")
                    .and_then(Value::as_i64)
                    .ok_or("process.now is not a time")?;
                let entries = step
                    .get("entries")
                    .and_then(Value::as_array)
                    .ok_or("process.entries is not a list")?
                    .iter()
                    .map(|e| {
                        if let Some(text) = e.get("hex").and_then(Value::as_str) {
                            heartwood_crypto::utils::hex::decode(text)
                                .map(Entry::Bytes)
                                .map_err(|e| e.to_string())
                        } else if let Some(json) = e.get("json") {
                            Ok(Entry::Data(json.clone()))
                        } else {
                            Err("an entry is neither hex nor json".into())
                        }
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let processor = match step.get("channel").and_then(Value::as_str) {
                    None | Some("api") => &mut self.api,
                    Some("p2p") => &mut self.p2p,
                    Some(other) => return Err(format!("unknown channel {other}")),
                };
                let clock = ManualClock::new(now);
                processor
                    .process(&mut self.pool, &ctx, &clock, entries)
                    .map_err(storage)?
                    .to_json()
            }
            "setOnline" => {
                for d in step
                    .get("delegates")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    let key = d
                        .get("publicKey")
                        .and_then(Value::as_str)
                        .and_then(|k| PublicKeyBytes::from_hex(k).ok())
                        .ok_or("a delegate has no public key")?;
                    let round = d.get("round").and_then(Value::as_u64).ok_or("no round")?;
                    self.online.set(key, round, VERSION.to_owned());
                }
                if let Some(round) = step.get("expireAt").and_then(Value::as_u64) {
                    self.online.expire(round);
                }
                json!({"online": self.online()})
            }
            "readd" => {
                let forged = step
                    .get("forged")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|t| self.decode(t.as_str().unwrap_or("")))
                    .collect::<Result<Vec<_>, String>>()?;
                let recheck = step.get("recheck") == Some(&Value::Bool(true));
                self.pool.readd(&ctx, &forged, recheck).map_err(storage)?;
                json!({})
            }
            "cleanUp" => {
                self.pool.clean_up(&ctx).map_err(storage)?;
                json!({})
            }
            "remove" | "removeForged" => {
                let transaction =
                    self.decode(step.get("hex").and_then(Value::as_str).unwrap_or(""))?;
                let sender = Address::from_public_key(
                    &transaction.data().sender_public_key,
                    self.network.pub_key_hash,
                );
                if op == "remove" {
                    self.pool
                        .remove_transaction(&ctx, &sender, transaction.id())
                        .map_err(storage)?;
                } else {
                    self.pool
                        .remove_forged_transaction(&sender, transaction.id())
                        .map_err(storage)?;
                }
                json!({})
            }
            "flush" => {
                self.pool.flush().map_err(storage)?;
                json!({})
            }
            "collate" => {
                let validate = step.get("validate") == Some(&Value::Bool(true));
                let exclude = step
                    .get("exclude")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|id| {
                        id.as_str()
                            .and_then(|t| TransactionId::from_hex(t).ok())
                            .ok_or_else(|| format!("{id} is not a transaction id"))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let collated =
                    collate(&mut self.pool, &ctx, validate, &exclude).map_err(storage)?;
                json!({"ids": collated.candidates.iter().map(|t| t.id().to_hex()).collect::<Vec<_>>()})
            }
            "restart" => {
                // A restarted node: a new pool over the same file, new processors, then the
                // start's re-add.
                let fresh = Pool::open(self.options.clone(), None).map_err(storage)?;
                drop(std::mem::replace(&mut self.pool, fresh));
                self.api = Processor::new();
                self.p2p = Processor::new();
                self.pool.readd(&ctx, &[], false).map_err(storage)?;
                json!({})
            }
            "tip" => {
                self.tip = step
                    .get("height")
                    .and_then(Value::as_u64)
                    .and_then(|h| u32::try_from(h).ok())
                    .ok_or("tip.height is not a height")?;
                json!({})
            }
            other => return Err(format!("unknown pool step {other}")),
        })
    }
}

/// `pool.run`: a pool session over a wallet fixture, every step's result and the contents after
/// it.
fn run(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let devnet = env.devnet()?;
    let milestones = env.milestones(case)?;
    let tip: u32 = integer(case.input, "lastBlockHeight")?;
    let params = milestones.at(tip.saturating_add(1));
    let fixture = Fixture::from_json(&case.input["fixture"]).map_err(|e| failed(e.to_string()))?;
    let wallets = fixture
        .to_wallets(params.network())
        .map_err(|e| failed(e.to_string()))?;
    let dir = ScratchDir::new("pool-subject").map_err(|e| failed(e.to_string()))?;
    let steps = case.input["steps"]
        .as_array()
        .ok_or_else(|| failed("input.steps is not a list"))?;
    let outcome = guarded(|| -> Result<Value, String> {
        let options = options(&case.input["options"], dir.path())?;
        let mut online = DelegateVersions::new();
        for d in case.input["online"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let key = d["publicKey"]
                .as_str()
                .and_then(|k| PublicKeyBytes::from_hex(k).ok())
                .ok_or("an online delegate has no public key")?;
            let round = d["round"]
                .as_u64()
                .ok_or("an online delegate has no round")?;
            online.set(key, round, VERSION.to_owned());
        }
        let pool = Pool::open(options.clone(), None).map_err(|e| e.to_string())?;
        let mut session = Session {
            options,
            pool,
            api: Processor::new(),
            p2p: Processor::new(),
            online,
            tip,
            wallets,
            milestones: &milestones,
            network: &devnet.network,
        };
        let mut results = Vec::new();
        for step in steps {
            let result = session.step(step)?;
            let mut entry = Map::new();
            entry.insert("result".into(), result);
            entry.insert("contents".into(), contents(&session.pool)?);
            results.push(Value::Object(entry));
        }
        Ok(json!({ "results": results }))
    });
    match outcome {
        Ok(Ok(output)) => Ok(Verdict::Output(output)),
        Ok(Err(message)) => Err(failed(message)),
        Err(Unbuilt(message)) => Err(SubjectError::Unsupported(format!(
            "the pool is not built yet: {message}"
        ))),
    }
}
