//! The cases of the pool: fixtures of 3 to 12 wallets (delegates, some of them online), random
//! options, and 1 to 8 steps of batches built from the fixture's keys and mutated, the clock
//! moving on, removals, re-adds and a restart of the storage, with a collation at the end.
//!
//! A case is one `pool.run` session. The options are the reference's defaults or, in a quarter of
//! the cases, small limits (the pool's size, the per-sender count, the age, the byte size, an
//! allowed sender); the fees are the milestone's, or with the milestone's dynamic fees off, the
//! pool's own floor or the static fees. A batch holds 1 to 6 entries: a transaction of a kept
//! type by a fixture wallet (transfers most often, votes for the fixture's delegates,
//! resignations, burns, second-signature and delegate registrations), with its nonce at the
//! sender's next one or off by one or two, a fee at the floor, one below it, above it or
//! anywhere, an amount around the sender's balance, sent as bytes or as JSON with its id, a wrong
//! id or none; some entries repeat an earlier one, carry a bad signature, another network byte,
//! a removed type or too few bytes.

use std::sync::Arc;

use heartwood_crypto::Aux;
use heartwood_crypto::Transaction;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, SigningDomain};
use heartwood_crypto::managers::Params;
use heartwood_crypto::transactions::builders::static_fee;
use heartwood_crypto::transactions::types::ResignationType;
use heartwood_crypto::utils::hex;
use heartwood_transactions::{minimum_fee, minimum_fee_with};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::transactions::{AUX, TxContext, TxSpec};
use super::{delegate_name, fixtures_at, second_keys, wallet_keys};
use crate::fixture::Fixture;

/// The time of the first batch, in milliseconds, a whole second.
pub const START_MS: i64 = 1_800_000_000_000;

/// How the fees are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeeMode {
    /// The milestone's dynamic fees.
    Milestone,
    /// The milestone's off, the pool's floor with these prices to enter and to be broadcast.
    Pool(u64, u64),
    /// The milestone's off and the pool's off: the static fees exactly.
    Static,
}

fn fee_mode() -> impl Strategy<Value = FeeMode> {
    prop_oneof![
        6 => Just(FeeMode::Milestone),
        2 => (0u64..=20_000, 0u64..=20_000).prop_map(|(a, b)| FeeMode::Pool(a, b)),
        1 => Just(FeeMode::Static),
    ]
}

/// Small limits: the pool's size, the per-sender count, the age, the byte size and whether the
/// first sender is allowed any count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Limits {
    pool: u8,
    per_sender: u8,
    age: u8,
    bytes: Option<u16>,
    allowed: bool,
}

fn limits() -> impl Strategy<Value = Option<Limits>> {
    prop::option::weighted(
        0.25,
        (
            1u8..=5,
            1u8..=3,
            1u8..=4,
            prop::option::weighted(0.3, 120u16..=400),
            prop::bool::weighted(0.2),
        )
            .prop_map(|(pool, per_sender, age, bytes, allowed)| Limits {
                pool,
                per_sender,
                age,
                bytes,
                allowed,
            }),
    )
}

/// One entry of a batch, as choices that the fixture turns into a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EntryPlan {
    sender: usize,
    kind: u8,
    target: usize,
    amount: u8,
    nonce: i8,
    fee: u8,
    form: u8,
    mutation: u8,
}

fn entry_plan() -> impl Strategy<Value = EntryPlan> {
    (
        0usize..12,
        0u8..10,
        0usize..12,
        0u8..4,
        prop_oneof![6 => Just(0i8), 1 => Just(-1i8), 1 => Just(1i8), 1 => Just(2i8)],
        0u8..6,
        prop_oneof![5 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8), 1 => Just(3u8)],
        prop_oneof![12 => Just(0u8), 2 => Just(1u8), 1 => Just(2u8), 1 => Just(3u8), 1 => Just(4u8), 1 => Just(5u8)],
    )
        .prop_map(
            |(sender, kind, target, amount, nonce, fee, form, mutation)| EntryPlan {
                sender,
                kind,
                target,
                amount,
                nonce,
                fee,
                form,
                mutation,
            },
        )
}

/// One step, as choices.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StepPlan {
    Process(Vec<EntryPlan>, u8, bool),
    Remove(usize),
    RemoveForged(usize),
    Readd(bool, bool),
    CleanUp,
    Flush,
    Restart,
    Tip(u8),
    SetOnline(usize, bool),
}

fn step_plan() -> impl Strategy<Value = StepPlan> {
    prop_oneof![
        12 => (prop::collection::vec(entry_plan(), 1..=6), 0u8..=40, prop::bool::weighted(0.2))
            .prop_map(|(entries, seconds, p2p)| StepPlan::Process(entries, seconds, p2p)),
        2 => (0usize..64).prop_map(StepPlan::Remove),
        1 => (0usize..64).prop_map(StepPlan::RemoveForged),
        2 => (any::<bool>(), prop::bool::weighted(0.3)).prop_map(|(recheck, forged)| StepPlan::Readd(recheck, forged)),
        1 => Just(StepPlan::CleanUp),
        1 => Just(StepPlan::Flush),
        1 => Just(StepPlan::Restart),
        1 => (1u8..=3).prop_map(StepPlan::Tip),
        1 => (0usize..12, any::<bool>()).prop_map(|(i, expire)| StepPlan::SetOnline(i, expire)),
    ]
}

/// A whole case, as choices.
#[derive(Debug, Clone)]
struct CasePlan {
    last: u32,
    fixture: Fixture,
    fees: FeeMode,
    limits: Option<Limits>,
    online: Vec<usize>,
    steps: Vec<StepPlan>,
    validate: bool,
    exclude: usize,
}

fn case_plan() -> impl Strategy<Value = CasePlan> {
    prop_oneof![3 => Just(999u32), 1 => 2u32..=20_000]
        .prop_flat_map(|last| {
            (
                Just(last),
                fixtures_at(last),
                fee_mode(),
                limits(),
                prop::collection::vec(0usize..12, 0..6),
                prop::collection::vec(step_plan(), 1..=8),
                any::<bool>(),
                prop_oneof![3 => Just(0usize), 1 => 1usize..=3],
            )
        })
        .prop_map(
            |(last, fixture, fees, limits, online, steps, validate, exclude)| CasePlan {
                last,
                fixture,
                fees,
                limits,
                online,
                steps,
                validate,
                exclude,
            },
        )
}

/// The pool's default `addonBytes` of a type key (the reference's pool defaults).
fn pool_addon_bytes(key: &str) -> i64 {
    match key {
        "secondSignature" | "ipfs" => 250,
        "delegateRegistration" => 400_000,
        "vote" | "delegateResignation" | "legacyTransfer" | "htlcLock" => 100,
        "multiSignature" | "transfer" => 500,
        _ => 0,
    }
}

/// The removed types of type group 1, which a mutation writes into a transaction.
const REMOVED: [u16; 7] = [0, 3, 4, 5, 8, 9, 10];

/// The bytes of `transaction` changed by `edit` and signed again by wallet `index` (the first
/// signature over the bytes before it; a second signature is dropped).
fn resigned(
    transaction: &Transaction,
    index: usize,
    edit: impl Fn(&mut Vec<u8>),
) -> Option<Vec<u8>> {
    let bytes = transaction.bytes();
    let signatures = if transaction.data().second_signature.is_some() {
        128
    } else {
        64
    };
    let mut unsigned = bytes.get(..bytes.len().checked_sub(signatures)?)?.to_vec();
    edit(&mut unsigned);
    let key = wallet_keys(index);
    let signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        &sha256(&unsigned),
        key.secret_key(),
        Aux::fixed(AUX),
    )
    .ok()?;
    unsigned.extend_from_slice(signature.as_bytes());
    Some(unsigned)
}

/// The header of a transaction before its memo: marker, version, network, type group, type,
/// nonce, sender key and fee.
const HEADER: usize = 58;

/// A legacy transfer (1/0, a removed type) in its canonical form, with the header, the nonce and
/// the fee of `transaction` and its first item, signed by wallet `index`: the header with type 0,
/// an empty memo, then the amount, an expiration of 0 and the recipient's 21 bytes.
fn legacy_transfer(transaction: &Transaction, index: usize) -> Option<Vec<u8>> {
    use heartwood_crypto::transactions::types::Asset;
    let Asset::Transfer { transfers } = &transaction.data().asset else {
        return None;
    };
    let item = transfers.first()?;
    let mut bytes = transaction.bytes().get(..HEADER)?.to_vec();
    bytes.get_mut(7..9)?.copy_from_slice(&0u16.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&item.amount.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(item.recipient.as_bytes());
    let key = wallet_keys(index);
    let signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        &sha256(&bytes),
        key.secret_key(),
        Aux::fixed(AUX),
    )
    .ok()?;
    bytes.extend_from_slice(signature.as_bytes());
    Some(bytes)
}

/// What the fixture and the choices build.
struct Builder<'a> {
    ctx: &'a TxContext,
    params: &'a Params,
    plan: &'a CasePlan,
    next_nonce: Vec<u64>,
    built: Vec<Transaction>,
    names_taken: usize,
}

impl Builder<'_> {
    /// The delegates of the fixture, by wallet index.
    fn delegates(&self) -> Vec<usize> {
        self.plan
            .fixture
            .wallets
            .iter()
            .enumerate()
            .filter(|(_, w)| w.delegate.is_some())
            .map(|(i, _)| i)
            .collect()
    }

    /// The fee of `plan` for `probe` (a transaction built with fee 1).
    fn fee(&self, probe: &Transaction, choice: u8) -> u64 {
        let floor = match self.plan.fees {
            FeeMode::Milestone => u64::try_from(minimum_fee(probe, self.params)).unwrap_or(0),
            FeeMode::Pool(min_pool, _) => u64::try_from(minimum_fee_with(
                probe,
                i64::try_from(min_pool).unwrap_or(0),
                pool_addon_bytes(probe.key()),
            ))
            .unwrap_or(0),
            FeeMode::Static => static_fee(probe.kind(), self.params).unwrap_or(0),
        };
        match choice {
            0 => floor.saturating_sub(1).max(1),
            1 => floor.max(1),
            2 => floor.saturating_add(1 + u64::try_from(self.names_taken % 7).unwrap_or(0)),
            3 => 50_000_000,
            4 => floor.saturating_add(floor / 2).max(1),
            _ => 1 + (floor.wrapping_mul(7919) % 1_000_000_000),
        }
    }

    /// The transaction of `entry`, and the index of its sender, or `None` when the builder
    /// refuses it.
    fn transaction(&mut self, entry: &EntryPlan) -> Option<(Transaction, usize)> {
        let wallets = &self.plan.fixture.wallets;
        // Three entries in four come from a wallet that can pay for them.
        let funded: Vec<usize> = wallets
            .iter()
            .enumerate()
            .filter(|(_, w)| w.balance >= 1_000_000_000)
            .map(|(i, _)| i)
            .collect();
        let sender = match funded.get(entry.sender % funded.len().max(1)) {
            Some(i) if !entry.sender.is_multiple_of(4) => *i,
            _ => entry.sender % wallets.len(),
        };
        let wallet = wallets.get(sender)?;
        let delegates = self.delegates();
        let balance = wallet.balance;
        let amount = match entry.amount {
            0 => 1 + (balance / 1000),
            1 => balance.saturating_sub(60_000_000),
            2 => balance,
            _ => balance.saturating_add(1),
        };
        let spec = match entry.kind {
            0..=4 => {
                let to = self
                    .ctx
                    .recipients
                    .get(entry.target % self.ctx.recipients.len())?;
                TxSpec::Transfer(vec![(*to, amount.max(1))])
            }
            5 if !delegates.is_empty() => {
                let first = delegates.get(entry.target % delegates.len())?;
                let second = delegates.get((entry.target + 1) % delegates.len())?;
                if first == second {
                    TxSpec::Vote(vec![(delegate_name(*first), 10_000)])
                } else {
                    TxSpec::Vote(vec![
                        (delegate_name(*first), 5_000),
                        (delegate_name(*second), 5_000),
                    ])
                }
            }
            6 if wallet.delegate.is_some() => TxSpec::Resignation(match entry.target % 3 {
                0 => None,
                1 => Some(ResignationType::PermanentResign),
                _ => Some(ResignationType::NotResigned),
            }),
            7 => TxSpec::Burn(amount.max(1)),
            8 => TxSpec::SecondSignature(format!("l1 pool second {}", entry.target)),
            9 => {
                self.names_taken += 1;
                TxSpec::DelegateRegistration(format!("l1p{}", entry.target % 4))
            }
            _ => TxSpec::Transfer(vec![(*self.ctx.recipients.first()?, 1)]),
        };
        let next = self.next_nonce.get_mut(sender)?;
        let nonce = next.checked_add_signed(i64::from(entry.nonce))?.max(1);
        if entry.nonce == 0 {
            *next = next.saturating_add(1);
        }
        let keys = wallet_keys(sender);
        let second = wallet.second_public_key.map(|_| second_keys(sender));
        let probe = spec.build(&keys, second.as_ref(), nonce, 1, self.params)?;
        let fee = self.fee(&probe, entry.fee);
        let transaction = spec.build(&keys, second.as_ref(), nonce, fee, self.params)?;
        Some((transaction, sender))
    }

    /// The entry of `plan` as the step's JSON (`{hex}` or `{json}`), or `None` to leave it out.
    fn entry(&mut self, plan: &EntryPlan) -> Option<Value> {
        if plan.mutation == 1 && !self.built.is_empty() {
            let again = self.built.get(plan.target % self.built.len())?;
            return Some(json!({"hex": hex::encode(again.bytes())}));
        }
        let (transaction, sender) = self.transaction(plan)?;
        self.built.push(transaction.clone());
        let bytes = match plan.mutation {
            2 => {
                let mut bytes = transaction.bytes().to_vec();
                let last = bytes.len().checked_sub(1)?;
                *bytes.get_mut(last)? ^= 1;
                bytes
            }
            3 => resigned(&transaction, sender, |b| {
                if let Some(network) = b.get_mut(2) {
                    *network = 30;
                }
            })?,
            4 if plan.target.is_multiple_of(2) => legacy_transfer(&transaction, sender)?,
            4 => {
                let removed = REMOVED[plan.target % REMOVED.len()];
                resigned(&transaction, sender, |b| {
                    if let Some(kind) = b.get_mut(7..9) {
                        kind.copy_from_slice(&removed.to_le_bytes());
                    }
                })?
            }
            5 => transaction.bytes().get(..40)?.to_vec(),
            _ => transaction.bytes().to_vec(),
        };
        let unchanged = !matches!(plan.mutation, 2..=5);
        if !unchanged || plan.form == 0 {
            return Some(json!({"hex": hex::encode(&bytes)}));
        }
        let mut data = transaction.to_json(self.params);
        match plan.form {
            2 => data["id"] = json!("00".repeat(32)),
            3 => {
                if let Some(o) = data.as_object_mut() {
                    o.shift_remove("id");
                }
            }
            _ => {}
        }
        Some(json!({"json": data}))
    }
}

/// The milestone file and the options of `plan`.
fn configuration(ctx: &TxContext, plan: &CasePlan) -> (Option<String>, Value) {
    let fees_off = || {
        let mut first: Value = serde_json::from_str::<Value>(&ctx.milestones_text)
            .ok()
            .and_then(|list| list.get(0).cloned())
            .unwrap_or(Value::Null);
        first["dynamicFees"]["enabled"] = json!(false);
        ctx.milestones_with("dynamicFees", first["dynamicFees"].clone())
    };
    let mut options = serde_json::Map::new();
    let milestones = match plan.fees {
        FeeMode::Milestone => None,
        FeeMode::Pool(min_pool, min_broadcast) => {
            options.insert(
                "dynamicFees".into(),
                json!({"enabled": true, "minFeePool": min_pool, "minFeeBroadcast": min_broadcast}),
            );
            Some(fees_off())
        }
        FeeMode::Static => {
            options.insert("dynamicFees".into(), json!({"enabled": false}));
            Some(fees_off())
        }
    };
    if let Some(l) = plan.limits {
        options.insert("maxTransactionsInPool".into(), json!(l.pool));
        options.insert("maxTransactionsPerSender".into(), json!(l.per_sender));
        options.insert("maxTransactionAge".into(), json!(l.age));
        if let Some(bytes) = l.bytes {
            options.insert("maxTransactionBytes".into(), json!(bytes));
        }
        if l.allowed
            && let Some(w) = plan.fixture.wallets.first()
        {
            options.insert("allowedSenders".into(), json!([w.address.to_base58()]));
        }
    }
    (milestones, Value::Object(options))
}

/// The case of `plan`, or `None` when its milestone file does not load.
fn realise(ctx: &TxContext, plan: &CasePlan) -> Option<OpCase> {
    let (milestones_text, options) = configuration(ctx, plan);
    let milestones = match &milestones_text {
        Some(text) => heartwood_crypto::Milestones::load(text, &ctx.network).ok()?,
        None => ctx.milestones.clone(),
    };
    let height = plan.last.checked_add(1)?;
    let params = milestones.at(height);
    let mut builder = Builder {
        ctx,
        params,
        plan,
        next_nonce: plan
            .fixture
            .wallets
            .iter()
            .map(|w| w.nonce.saturating_add(1))
            .collect(),
        built: Vec::new(),
        names_taken: 0,
    };
    let delegates = builder.delegates();
    let round = u64::from(height) / 53 + 1;
    let online: Vec<Value> = plan
        .online
        .iter()
        .filter_map(|i| delegates.get(i % delegates.len().max(1)))
        .map(|i| json!({"publicKey": wallet_keys(*i).public_key().to_hex(), "round": round}))
        .collect();
    let mut now = START_MS;
    let mut steps = Vec::new();
    for step in &plan.steps {
        let pick = |i: usize, built: &[Transaction]| {
            (!built.is_empty()).then(|| hex::encode(built[i % built.len()].bytes()))
        };
        let value = match step {
            StepPlan::Process(entries, seconds, p2p) => {
                now += i64::from(*seconds) * 1000;
                let entries: Vec<Value> = entries.iter().filter_map(|e| builder.entry(e)).collect();
                if entries.is_empty() {
                    continue;
                }
                let mut value = json!({"op": "process", "entries": entries, "now": now});
                if *p2p {
                    value["channel"] = json!("p2p");
                }
                value
            }
            StepPlan::Remove(i) => match pick(*i, &builder.built) {
                Some(hex_text) => json!({"op": "remove", "hex": hex_text}),
                None => continue,
            },
            StepPlan::RemoveForged(i) => match pick(*i, &builder.built) {
                Some(hex_text) => json!({"op": "removeForged", "hex": hex_text}),
                None => continue,
            },
            StepPlan::Readd(recheck, forged) => {
                let mut value = json!({"op": "readd", "recheck": recheck});
                if *forged && let Some(hex_text) = pick(0, &builder.built) {
                    value["forged"] = json!([hex_text]);
                }
                value
            }
            StepPlan::CleanUp => json!({"op": "cleanUp"}),
            StepPlan::Flush => json!({"op": "flush"}),
            StepPlan::Restart => json!({"op": "restart"}),
            StepPlan::Tip(n) => json!({"op": "tip", "height": height - 1 + u32::from(*n)}),
            StepPlan::SetOnline(i, expire) => {
                let Some(d) = delegates.get(i % delegates.len().max(1)) else {
                    continue;
                };
                let mut value = json!({"op": "setOnline", "delegates": [{"publicKey": wallet_keys(*d).public_key().to_hex(), "round": round}]});
                if *expire {
                    value["expireAt"] = json!(round + 6);
                }
                value
            }
        };
        steps.push(value);
    }
    let exclude: Vec<String> = builder
        .built
        .iter()
        .take(plan.exclude)
        .map(|t| t.id().to_hex())
        .collect();
    steps.push(json!({"op": "collate", "validate": plan.validate, "exclude": exclude}));
    let mut input = json!({
        "fixture": plan.fixture.to_json(),
        "lastBlockHeight": plan.last,
        "online": online,
        "steps": steps,
    });
    if let Some(text) = milestones_text {
        input["milestones"] = json!(text);
    }
    if options.as_object().is_some_and(|o| !o.is_empty()) {
        input["options"] = options;
    }
    Some(("pool.run", u64::from(height), input))
}

/// P10: whole pool sessions over the devnet of `context`.
pub fn pool_cases(context: Arc<TxContext>) -> BoxedStrategy<OpCase> {
    case_plan()
        .prop_filter_map("the milestone file does not load", move |plan| {
            realise(&context, &plan)
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    use super::*;

    fn context() -> Arc<TxContext> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/crypto/tests/vectors");
        let set = crate::vectors::VectorSet::load(&dir).unwrap();
        let devnet = crate::subjects::Devnet::from_vectors(&set)
            .unwrap()
            .unwrap();
        let text = crate::levels::l1::devnet_milestones(&set).unwrap();
        Arc::new(TxContext::new(devnet.network, text).unwrap())
    }

    #[test]
    fn pool_cases_are_sessions_with_a_collation_at_the_end() {
        let mut runner = TestRunner::deterministic();
        let strategy = pool_cases(context());
        let mut ops = std::collections::BTreeSet::new();
        let mut entries = 0;
        for _ in 0..200 {
            let (op, height, input) = strategy.new_tree(&mut runner).unwrap().current();
            assert_eq!(op, "pool.run");
            assert_eq!(height, input["lastBlockHeight"].as_u64().unwrap() + 1);
            let steps = input["steps"].as_array().unwrap();
            assert_eq!(steps.last().unwrap()["op"], json!("collate"));
            for s in steps {
                ops.insert(s["op"].as_str().unwrap().to_owned());
                if let Some(e) = s["entries"].as_array() {
                    assert!((1..=6).contains(&e.len()));
                    entries += e.len();
                }
            }
            Fixture::from_json(&input["fixture"]).unwrap();
        }
        assert!(entries > 200, "{entries}");
        for op in [
            "process",
            "remove",
            "readd",
            "restart",
            "collate",
            "setOnline",
        ] {
            assert!(ops.contains(op), "{op}: {ops:?}");
        }
    }
}
