//! The synthetic pool cases on a corpus chain's tip: transactions added on both sides, blocks
//! forged from the collated candidates and applied, the synced node's sequence after each block
//! (remove forged, clean up, re-add), reverts with the re-add of their transactions, the online
//! record's expiry, a rule change at the next height, and a restart of the pool's storage.
//!
//! Both sides start from the corpus's tip: the oracle's chain session with the reference's pool
//! bound in its node (`chain.open` with `pool`), Heartwood's state with its pool, its processor
//! and its online record. Every step runs on both sides and is compared: a batch's result and the
//! pool's contents with the pool's rules ([`crate::levels::pool`]), a collation's ids, a block's
//! verdict and digest line (as the other synthetic cases compare them) and the contents after the
//! pool's steps. A block is forged from the reference's collation. Where Heartwood's pool is
//! still a stub, its side of each pool step is skipped, and the chain's steps are still compared.
//!
//! - `pool-forge`: transfers from three senders with mixed fees, three blocks forged from
//!   validated collations, each followed by the synced sequence;
//! - `pool-competing-nonce`: a block holds another transaction for a pending nonce, and the re-add
//!   drops the pending transaction;
//! - `pool-revert-readd`: a block of pool transactions reverted, its transactions re-added;
//! - `pool-online-expiry`: a vote for a delegate before and after it is online, and the expiry of
//!   its entry at a round start, which the re-add then applies;
//! - `pool-rule-next-height`: a milestone at the tip plus two raises the fee floor: a transfer at
//!   the old floor enters, and the re-add after the next block drops it;
//! - `pool-storage-restart`: the pool's storage opened again and its transactions re-added.

use std::collections::BTreeMap;

use heartwood_blockchain::online;
use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::Votes;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, Transaction, TransactionId};
use heartwood_database::{Database, Settings};
use heartwood_kernel::clock::ManualClock;
use heartwood_kernel::utils::round_calculator::is_new_round;
use heartwood_pool::{DelegateVersions, Entry, Options, Pool, PoolContext, Processor, collate};
use serde_json::{Value, json};

use crate::corpus::Corpus;
use crate::errors::{Error, Result};
use crate::levels::l2::call;
use crate::levels::pool::judge;
use crate::levels::pool::{VERSION, contents, options};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};
use crate::store::{ScratchDir, StartProblem, Unbuilt, guarded};
use crate::synthetic::{FEE, Keys, Sides, corpus_keys, transfer};

/// A synthetic pool case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolCase {
    /// `pool-forge`.
    Forge,
    /// `pool-competing-nonce`.
    CompetingNonce,
    /// `pool-revert-readd`.
    RevertReadd,
    /// `pool-online-expiry`.
    OnlineExpiry,
    /// `pool-rule-next-height`.
    RuleAtNextHeight,
    /// `pool-storage-restart`.
    StorageRestart,
}

impl PoolCase {
    /// Every case.
    pub const ALL: [PoolCase; 6] = [
        PoolCase::Forge,
        PoolCase::CompetingNonce,
        PoolCase::RevertReadd,
        PoolCase::OnlineExpiry,
        PoolCase::RuleAtNextHeight,
        PoolCase::StorageRestart,
    ];

    /// The case's name in reports.
    pub const fn name(self) -> &'static str {
        match self {
            PoolCase::Forge => "pool-forge",
            PoolCase::CompetingNonce => "pool-competing-nonce",
            PoolCase::RevertReadd => "pool-revert-readd",
            PoolCase::OnlineExpiry => "pool-online-expiry",
            PoolCase::RuleAtNextHeight => "pool-rule-next-height",
            PoolCase::StorageRestart => "pool-storage-restart",
        }
    }
}

/// The time of a case's first batch, in milliseconds; each batch comes 31 seconds after the one
/// before it, past the cooldown.
const START_MS: i64 = 1_800_000_000_000;

/// Heartwood's pool in a case.
struct PoolSide {
    scratch: ScratchDir,
    options: Options,
    pool: Pool,
    processor: Processor,
    online: DelegateVersions,
}

impl PoolSide {
    fn open(given: &Value) -> std::result::Result<PoolSide, StartProblem> {
        let scratch =
            ScratchDir::new("synthetic-pool").map_err(|e| StartProblem::Failed(e.to_string()))?;
        let opened = guarded(|| -> std::result::Result<(Options, Pool), String> {
            let options = options(given, scratch.path())?;
            let pool = Pool::open(options.clone(), None).map_err(|e| e.to_string())?;
            Ok((options, pool))
        })?;
        let (options, pool) = opened.map_err(StartProblem::Failed)?;
        Ok(PoolSide {
            scratch,
            options,
            pool,
            processor: Processor::new(),
            online: DelegateVersions::new(),
        })
    }
}

/// A case: both sides with their pools.
struct PoolSides<'a> {
    s: Sides<'a>,
    pool: Option<PoolSide>,
    problem: Option<StartProblem>,
    now: i64,
    /// Every transaction sent, by id.
    sent: BTreeMap<String, Transaction>,
}

impl<'a> PoolSides<'a> {
    fn record(&mut self, what: &str, outcome: Outcome) {
        self.s.record(what, outcome);
    }

    /// Record `outcome` of a step of Heartwood's pool, or a skip when the pool is a stub.
    fn skipped(&mut self, what: &str) {
        let why = match &self.problem {
            Some(StartProblem::Unbuilt(m)) => format!("not built yet: {m}"),
            Some(StartProblem::Failed(m)) => m.clone(),
            None => "no pool".into(),
        };
        self.record(what, Outcome::Skipped(why));
    }

    /// Run `f` on Heartwood's pool with the context of the state's tip; `None` when there is no
    /// pool (a skip is recorded) or it is a stub.
    fn heartwood<T>(
        &mut self,
        what: &str,
        f: impl FnOnce(&mut PoolSide, &PoolContext<'_>) -> std::result::Result<T, String>,
    ) -> Option<T> {
        let Some(side) = self.pool.as_mut() else {
            self.skipped(what);
            return None;
        };
        let online = side.online.clone();
        let ctx = PoolContext {
            wallets: self.s.state.wallets(),
            tip: self.s.state.height(),
            milestones: &self.s.milestones,
            network: &self.s.corpus.network,
            online: &online,
        };
        match guarded(|| f(side, &ctx)) {
            Ok(Ok(value)) => Some(value),
            Ok(Err(message)) => {
                self.record(what, Outcome::HarnessError(message));
                None
            }
            Err(Unbuilt(message)) => {
                self.problem = Some(StartProblem::Unbuilt(message));
                self.pool = None;
                self.skipped(what);
                None
            }
        }
    }

    /// Compare one step: the oracle's result and contents against Heartwood's, with the pool's
    /// rules.
    fn compare(&mut self, what: &str, step: Value, theirs: Value, ours: Option<Value>) {
        let Some(ours) = ours else {
            return;
        };
        let input = json!({ "steps": [step] });
        let outcome = judge(
            &input,
            &json!({ "results": [theirs] }),
            &json!({ "results": [ours] }),
        );
        self.record(what, outcome);
    }

    /// The oracle's contents now.
    fn oracle_contents(&self, server: &mut OracleServer) -> Result<Value> {
        Ok(call(
            server,
            "pool.contents",
            1,
            &json!({"session": self.s.session}),
        )?["contents"]
            .clone())
    }

    /// Send `transactions` as one batch to both processors.
    fn process(
        &mut self,
        what: &str,
        transactions: &[Transaction],
        server: &mut OracleServer,
    ) -> Result<Value> {
        self.now += 31_000;
        let now = self.now;
        for t in transactions {
            self.sent.insert(t.id().to_hex(), t.clone());
        }
        let entries: Vec<Value> = transactions
            .iter()
            .map(|t| json!({"hex": hex::encode(t.bytes())}))
            .collect();
        let mut result = call(
            server,
            "pool.process",
            1,
            &json!({"session": self.s.session, "entries": entries, "now": now}),
        )?;
        let theirs = json!({"result": result.clone(), "contents": self.oracle_contents(server)?});
        let batch: Vec<Entry> = transactions
            .iter()
            .map(|t| Entry::Bytes(t.bytes().to_vec()))
            .collect();
        let ours = self.heartwood(what, |side, ctx| {
            let clock = ManualClock::new(now);
            let result = side
                .processor
                .process(&mut side.pool, ctx, &clock, batch)
                .map_err(|e| e.to_string())?;
            Ok(json!({"result": result.to_json(), "contents": contents(&side.pool)?}))
        });
        self.compare(
            what,
            json!({"op": "process", "entries": entries, "now": now}),
            theirs,
            ours,
        );
        if let Some(o) = result.as_object_mut() {
            o.shift_remove("notes");
        }
        Ok(result)
    }

    /// Collate on both sides; the reference's candidates, which a block is forged from.
    fn collate(&mut self, what: &str, server: &mut OracleServer) -> Result<Vec<Transaction>> {
        let theirs = call(
            server,
            "pool.collate",
            1,
            &json!({"session": self.s.session, "validate": true, "exclude": []}),
        )?;
        let ids: Vec<String> = theirs["ids"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let ours = self.heartwood(what, |side, ctx| {
            let collated = collate(&mut side.pool, ctx, true, &[]).map_err(|e| e.to_string())?;
            let ids: Vec<String> = collated
                .candidates
                .iter()
                .map(|t| t.id().to_hex())
                .collect();
            Ok(json!({"result": {"ids": ids}, "contents": contents(&side.pool)?}))
        });
        self.compare(
            what,
            json!({"op": "collate", "validate": true, "exclude": []}),
            json!({"result": {"ids": ids.clone()}, "contents": theirs["contents"].clone()}),
            ours,
        );
        ids.iter()
            .map(|id| {
                self.sent.get(id).cloned().ok_or_else(|| {
                    Error::Oracle(format!(
                        "the reference collated {id}, which the case did not send"
                    ))
                })
            })
            .collect()
    }

    /// Apply the block `bytes` on both sides and run the synced node's pool steps after it:
    /// the forged transactions removed, the clean-up, the online record's expiry at a round
    /// start, and the re-add with its re-check.
    fn apply(
        &mut self,
        what: &str,
        bytes: &[u8],
        expect: bool,
        server: &mut OracleServer,
    ) -> Result<bool> {
        let (accepted, theirs) =
            self.s
                .apply_by(what, bytes, expect, server, "pool.blockApplied")?;
        if !accepted {
            return Ok(false);
        }
        let block = Block::receive(bytes, &self.s.milestones)
            .map_err(|e| Error::Oracle(format!("the block: {e}")))?;
        let network = self.s.corpus.network.pub_key_hash;
        let height = block.height();
        let new_round = is_new_round(height + 1, &self.s.milestones);
        let round = heartwood_kernel::calculate_round(height + 1, &self.s.milestones).round;
        let ours = self.heartwood(&format!("{what}: the pool after the block"), |side, ctx| {
            if new_round {
                side.online.expire(round);
            }
            let online = side.online.clone();
            let ctx = PoolContext {
                online: &online,
                ..*ctx
            };
            for t in block.transactions() {
                let sender = Address::from_public_key(&t.data().sender_public_key, network);
                side.pool
                    .remove_forged_transaction(&sender, t.id())
                    .map_err(|e| e.to_string())?;
            }
            side.pool.clean_up(&ctx).map_err(|e| e.to_string())?;
            side.pool.readd(&ctx, &[]).map_err(|e| e.to_string())?;
            Ok(json!({"result": {}, "contents": contents(&side.pool)?}))
        });
        self.compare(
            &format!("{what}: the pool after the block"),
            json!({"op": "readd", "recheck": true}),
            json!({"result": {}, "contents": theirs["contents"].clone()}),
            ours,
        );
        Ok(true)
    }

    /// Revert `count` blocks on both sides and re-add their transactions, in chain order.
    fn revert(&mut self, what: &str, count: u32, server: &mut OracleServer) -> Result<()> {
        let (theirs, blocks) = self.s.revert_by(what, count, server, "pool.revert")?;
        let transactions: Vec<Transaction> = blocks
            .iter()
            .rev()
            .flat_map(|b| b.transactions().iter().cloned())
            .collect();
        let ours = self.heartwood(
            &format!("{what}: the pool after the revert"),
            |side, ctx| {
                side.pool
                    .readd(ctx, &transactions)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"result": {}, "contents": contents(&side.pool)?}))
            },
        );
        self.compare(
            &format!("{what}: the pool after the revert"),
            json!({"op": "readd"}),
            json!({"result": {}, "contents": theirs["contents"].clone()}),
            ours,
        );
        Ok(())
    }

    /// Set online entries on both sides, then expire at `expire_at` when given.
    fn set_online(
        &mut self,
        what: &str,
        delegates: &[(PublicKeyBytes, u64)],
        expire_at: Option<u64>,
        server: &mut OracleServer,
    ) -> Result<()> {
        let list: Vec<Value> = delegates
            .iter()
            .map(|(k, r)| json!({"publicKey": k.to_hex(), "round": r}))
            .collect();
        let mut args = json!({"session": self.s.session, "delegates": list});
        if let Some(round) = expire_at {
            args["expireAt"] = json!(round);
        }
        let theirs = call(server, "pool.setOnline", 1, &args)?;
        if let Some(side) = self.pool.as_mut() {
            for (key, round) in delegates {
                side.online.set(*key, *round, VERSION.to_owned());
            }
            if let Some(round) = expire_at {
                guarded(|| side.online.expire(round)).ok();
            }
            let mut ours: Vec<String> = side
                .online
                .entries()
                .into_iter()
                .map(|e| e.public_key.to_hex())
                .collect();
            ours.sort();
            let outcome = if theirs["online"] == json!(ours) {
                Outcome::Pass
            } else {
                Outcome::Mismatch(crate::compare::diff(&theirs["online"], &json!(ours)))
            };
            self.record(what, outcome);
        }
        Ok(())
    }

    /// Restart the pools' storage on both sides.
    fn restart_pool(&mut self, what: &str, server: &mut OracleServer) -> Result<()> {
        let theirs = call(
            server,
            "pool.restart",
            1,
            &json!({"session": self.s.session}),
        )?;
        let ours = self.heartwood(what, |side, ctx| {
            // The online record goes through the node's store, as across a node's restart.
            let text = |e: &dyn std::fmt::Display| e.to_string();
            let data = side.scratch.path().join("node");
            let mut database =
                Database::open(&Settings::in_data_dir(&data)).map_err(|e| text(&e))?;
            database
                .put_delegate_versions(&online::rows(&side.online))
                .map_err(|e| text(&e))?;
            let rows = database.delegate_versions().map_err(|e| text(&e))?;
            side.online = online::record(rows).map_err(|e| text(&e))?;
            if side.online != *ctx.online {
                return Err("the online record did not come back from the store".into());
            }
            let fresh = Pool::open(side.options.clone(), None).map_err(|e| e.to_string())?;
            drop(std::mem::replace(&mut side.pool, fresh));
            side.processor = Processor::new();
            side.pool.readd(ctx, &[]).map_err(|e| e.to_string())?;
            Ok(json!({"result": {}, "contents": contents(&side.pool)?}))
        });
        self.compare(
            what,
            json!({"op": "restart"}),
            json!({"result": {}, "contents": theirs["contents"].clone()}),
            ours,
        );
        Ok(())
    }

    /// Whether the oracle's pool holds the transaction `id`.
    fn holds(&self, id: &TransactionId, server: &mut OracleServer) -> Result<bool> {
        let contents = self.oracle_contents(server)?;
        Ok(contents["all"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(&id.to_hex()))))
    }
}

/// Funded wallets of the seed with keys, by the seed's order, `n` of them.
fn senders<'k>(
    s: &Sides<'k>,
    keys: &'k Keys,
    n: usize,
) -> Vec<&'k heartwood_crypto::genesis::DerivedKey> {
    keys.wallets()
        .filter(|k| s.funded(k, 10_000_000_000))
        .take(n)
        .collect()
}

fn run_case(
    case: PoolCase,
    corpus: &Corpus,
    keys: &Keys,
    server: &mut OracleServer,
) -> Result<Vec<CaseResult>> {
    let milestones_text = match case {
        PoolCase::RuleAtNextHeight => {
            let path = corpus.dir.join("crypto/milestones.json");
            let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
            let mut list: Value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
                path: path.clone(),
                message: e.to_string(),
            })?;
            let first = list.get(0).cloned().unwrap_or(Value::Null);
            let mut fees = first["dynamicFees"].clone();
            let min_fee = fees["minFee"].as_i64().unwrap_or(1).max(1);
            fees["minFee"] = json!(min_fee.saturating_mul(2));
            if let Some(items) = list.as_array_mut() {
                items.push(json!({"height": corpus.tip() + 2, "dynamicFees": fees}));
            }
            Some(list.to_string())
        }
        _ => None,
    };
    let options = json!({});
    let s = Sides::open(
        corpus,
        keys,
        server,
        case.name(),
        false,
        milestones_text,
        Some(options.clone()),
    )?;
    let (pool, problem) = match PoolSide::open(&options) {
        Ok(side) => (Some(side), None),
        Err(problem) => (None, Some(problem)),
    };
    let mut p = PoolSides {
        s,
        pool,
        problem,
        now: START_MS,
        sent: BTreeMap::new(),
    };
    let recipients: Vec<Address> = keys.wallets().map(|k| k.address).collect();
    let tx = |p: &PoolSides<'_>,
              from: &heartwood_crypto::genesis::DerivedKey,
              offset: u64,
              fee: u64,
              to: usize| {
        p.s.build(
            transfer(&[(recipients[to % recipients.len()], 1_000 + offset)])
                .nonce(p.s.nonce(&from.address) + 1 + offset)
                .fee(fee),
            from,
        )
    };
    match case {
        PoolCase::Forge => {
            let from = senders(&p.s, keys, 3);
            if from.len() < 3 {
                return Err(Error::Oracle("fewer than three funded wallets".into()));
            }
            let batch = vec![
                tx(&p, from[0], 0, 30_000_000, 5)?,
                tx(&p, from[0], 1, 20_000_000, 6)?,
                tx(&p, from[1], 0, 50_000_000, 7)?,
                tx(&p, from[2], 0, 10_000_000, 8)?,
                tx(&p, from[2], 1, 60_000_000, 9)?,
            ];
            p.process("transfers from three senders", &batch, server)?;
            for n in 1..=3 {
                let candidates = p.collate(&format!("the candidates of block {n}"), server)?;
                let bytes = p.s.next_block(candidates, 0, None)?;
                p.apply(
                    &format!("block {n} of the candidates"),
                    &bytes,
                    true,
                    server,
                )?;
                if let Some(next) = from.get(n).filter(|_| n < 3) {
                    let more = vec![tx(
                        &p,
                        next,
                        0,
                        25_000_000 + u64::try_from(n).unwrap_or(0),
                        10 + n,
                    )?];
                    p.process(&format!("a transfer after block {n}"), &more, server)?;
                }
            }
        }
        PoolCase::CompetingNonce => {
            let from = senders(&p.s, keys, 1);
            let a = *from
                .first()
                .ok_or_else(|| Error::Oracle("no funded wallet".into()))?;
            let pending = vec![tx(&p, a, 0, 20_000_000, 3)?, tx(&p, a, 1, 20_000_000, 4)?];
            p.process("two pending transfers", &pending, server)?;
            let competing = tx(&p, a, 0, 30_000_000, 12)?;
            let bytes = p.s.next_block(vec![competing], 0, None)?;
            p.apply(
                "a block with another transaction for the pending nonce",
                &bytes,
                true,
                server,
            )?;
            let dropped = !p.holds(pending[0].id(), server)?;
            p.s.premise(
                dropped,
                "the re-add dropped the pending transaction of the forged nonce",
            );
        }
        PoolCase::RevertReadd => {
            let from = senders(&p.s, keys, 2);
            let batch: Vec<Transaction> = from
                .iter()
                .enumerate()
                .map(|(i, k)| tx(&p, k, 0, 20_000_000, 14 + i))
                .collect::<Result<_>>()?;
            p.process("transfers", &batch, server)?;
            let candidates = p.collate("the candidates", server)?;
            let bytes = p.s.next_block(candidates, 0, None)?;
            p.apply("a block of the pool's transactions", &bytes, true, server)?;
            p.revert("the revert of that block", 1, server)?;
            let back = p.holds(batch[0].id(), server)?;
            p.s.premise(back, "the revert re-added the block's transactions");
        }
        PoolCase::OnlineExpiry => {
            let voter = *senders(&p.s, keys, 1)
                .first()
                .ok_or_else(|| Error::Oracle("no funded wallet".into()))?;
            let target =
                p.s.state
                    .wallets()
                    .delegates()
                    .filter_map(|w| {
                        w.delegate().map(|d| {
                            (
                                d.username().to_owned(),
                                d.resigned.is_none(),
                                w.public_key().map(|k| k.to_compressed()),
                            )
                        })
                    })
                    .find(|(_, active, key)| *active && key.is_some())
                    .ok_or_else(|| Error::Oracle("no delegate".into()))?;
            let (name, _, key) = target;
            let key = key.ok_or_else(|| Error::Oracle("no key".into()))?;
            let vote = |p: &PoolSides<'_>, offset: u64| {
                let mut votes = Votes::new();
                votes.insert(name.clone(), 10_000);
                p.s.build(
                    TransactionBuilder::vote(votes)
                        .nonce(p.s.nonce(&voter.address) + 1)
                        .fee(FEE + offset),
                    voter,
                )
            };
            let first = vote(&p, 0)?;
            p.process("a vote for a delegate without a version", &[first], server)?;
            let round =
                heartwood_kernel::calculate_round(p.s.state.height() + 1, &p.s.milestones).round;
            p.set_online(
                "the delegate announces a version",
                &[(key, round)],
                None,
                server,
            )?;
            let second = vote(&p, 1)?;
            p.process(
                "the vote once the delegate is online",
                std::slice::from_ref(&second),
                server,
            )?;
            // An entry of five rounds ago expires at the next round start.
            p.set_online(
                "the entry dated five rounds back",
                &[(key, round.saturating_sub(5))],
                None,
                server,
            )?;
            let mut guard = 0;
            loop {
                let bytes = p.s.next_block(Vec::new(), 0, None)?;
                let starts = is_new_round(p.s.state.height() + 2, &p.s.milestones);
                p.apply(
                    "an empty block towards the round start",
                    &bytes,
                    true,
                    server,
                )?;
                guard += 1;
                if starts || guard > 60 {
                    break;
                }
            }
            let dropped = !p.holds(second.id(), server)?;
            p.s.premise(dropped, "the re-add after the round start dropped the vote");
            let third = vote(&p, 2)?;
            p.process("a vote after the entry expired", &[third], server)?;
        }
        PoolCase::RuleAtNextHeight => {
            let a = *senders(&p.s, keys, 1)
                .first()
                .ok_or_else(|| Error::Oracle("no funded wallet".into()))?;
            let probe = tx(&p, a, 0, 1, 16)?;
            let params = p.s.milestones.at(p.s.state.height() + 1);
            let floor =
                u64::try_from(heartwood_transactions::minimum_fee(&probe, params)).unwrap_or(0);
            let at_floor = tx(&p, a, 0, floor, 16)?;
            p.process(
                "a transfer at the floor of the tip plus one",
                std::slice::from_ref(&at_floor),
                server,
            )?;
            let admitted = p.holds(at_floor.id(), server)?;
            p.s.premise(admitted, "the transfer entered the pool");
            let bytes = p.s.next_block(Vec::new(), 0, None)?;
            p.apply(
                "an empty block before the higher floor",
                &bytes,
                true,
                server,
            )?;
            let dropped = !p.holds(at_floor.id(), server)?;
            p.s.premise(
                dropped,
                "the re-add under the higher floor dropped the transfer",
            );
        }
        PoolCase::StorageRestart => {
            let from = senders(&p.s, keys, 2);
            let mut batch = Vec::new();
            for (i, k) in from.iter().enumerate() {
                batch.push(tx(
                    &p,
                    k,
                    0,
                    20_000_000 + u64::try_from(i).unwrap_or(0),
                    18,
                )?);
                batch.push(tx(&p, k, 1, 20_000_000, 19)?);
            }
            p.process("transfers before the restart", &batch, server)?;
            p.restart_pool("a restart of the pool's storage", server)?;
            if let Some(k) = from.first() {
                let more = vec![tx(&p, k, 2, 20_000_000, 20)?];
                p.process("a transfer after the restart", &more, server)?;
            }
        }
    }
    call(server, "chain.close", 1, &json!({"session": p.s.session}))?;
    Ok(std::mem::take(&mut p.s.results))
}

/// Run every synthetic pool case on top of `corpus` against the oracle `server`.
pub fn run(corpus: &Corpus, server: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let keys = corpus_keys(corpus)?;
    let mut out = Vec::new();
    for case in PoolCase::ALL {
        match run_case(case, corpus, &keys, server) {
            Ok(results) => out.extend(results),
            Err(e) => out.push(CaseResult {
                file: format!("{}/synthetic", corpus.name()),
                line: 0,
                op: case.name().to_owned(),
                label: case.name().to_owned(),
                height: u64::from(corpus.tip()),
                outcome: Outcome::HarnessError(e.to_string()),
            }),
        }
    }
    Ok(out)
}
