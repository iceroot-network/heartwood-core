//! Signed transactions of the six kept types: the fee-floor cases of P1 and the handler cases of
//! P7, built with Heartwood's builders from the harness's fixed passphrases and a fixed aux.

use std::sync::Arc;

use heartwood_crypto::identities::{Address, KeyPair};
use heartwood_crypto::managers::{Milestones, Network, Params};
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, Transaction};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::{MAX_SUPPLY, address_of, fixtures_at, keys, second_keys, wallet_keys};
use crate::fixture::Fixture;

/// The aux of every signature the strategies make.
pub const AUX: [u8; 32] = [0x42; 32];
/// The revoke delay of the devnet, in blocks.
pub const REVOKE_DELAY: u32 = 106;

/// What the transaction strategies build with: the devnet's network and milestone file.
#[derive(Debug, Clone)]
pub struct TxContext {
    /// The network description.
    pub network: Network,
    /// The devnet's milestone file, JSON text.
    pub milestones_text: String,
    /// The devnet's milestones, loaded.
    pub milestones: Milestones,
    /// Recipients outside every fixture.
    pub recipients: Vec<Address>,
}

impl TxContext {
    /// The context of the devnet `network` and milestone file `milestones_text`.
    pub fn new(network: Network, milestones_text: String) -> Result<TxContext, String> {
        let milestones = Milestones::load(&milestones_text, &network).map_err(|e| e.to_string())?;
        let recipients = (0..256)
            .map(|i| address_of(keys(&format!("l1 recipient {i}")).public_key()))
            .collect();
        Ok(TxContext {
            network,
            milestones_text,
            milestones,
            recipients,
        })
    }

    /// The milestone file with the first milestone's `key` set to `value`.
    pub fn milestones_with(&self, key: &str, value: Value) -> String {
        let mut list: Value = serde_json::from_str(&self.milestones_text).unwrap_or(Value::Null);
        if let Some(first) = list.get_mut(0).and_then(Value::as_object_mut) {
            first.insert(key.to_owned(), value);
        }
        list.to_string()
    }

    fn load(&self, text: &str) -> Option<Milestones> {
        Milestones::load(text, &self.network).ok()
    }
}

/// A transaction's type and asset, before its sender, nonce and fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxSpec {
    /// 1/6: items as (recipient, amount).
    Transfer(Vec<(Address, u64)>),
    /// 1/1: the second key of this passphrase.
    SecondSignature(String),
    /// 1/2: the username.
    DelegateRegistration(String),
    /// 1/7: the resignation type (`None`: temporary, written as no byte).
    Resignation(Option<ResignationType>),
    /// 2/0: the burned amount.
    Burn(u64),
    /// 2/2: the votes, in wire order.
    Vote(Vec<(String, u16)>),
}

impl TxSpec {
    /// The reference's key of the type, as the fee table names it.
    pub fn key(&self) -> &'static str {
        match self {
            TxSpec::Transfer(_) => "transfer",
            TxSpec::SecondSignature(_) => "secondSignature",
            TxSpec::DelegateRegistration(_) => "delegateRegistration",
            TxSpec::Resignation(_) => "delegateResignation",
            TxSpec::Burn(_) => "burn",
            TxSpec::Vote(_) => "vote",
        }
    }

    fn builder(&self) -> TransactionBuilder {
        match self {
            TxSpec::Transfer(items) => TransactionBuilder::transfer(
                items
                    .iter()
                    .map(|(recipient, amount)| TransferItem {
                        amount: *amount,
                        recipient: *recipient,
                    })
                    .collect(),
            ),
            TxSpec::SecondSignature(passphrase) => {
                TransactionBuilder::second_signature(keys(passphrase).public_key().to_compressed())
            }
            TxSpec::DelegateRegistration(name) => TransactionBuilder::delegate_registration(name),
            TxSpec::Resignation(kind) => TransactionBuilder::delegate_resignation(*kind),
            TxSpec::Burn(amount) => TransactionBuilder::burn(*amount),
            TxSpec::Vote(votes) => {
                let mut map = Votes::new();
                for (name, bps) in votes {
                    map.insert(name.clone(), *bps);
                }
                TransactionBuilder::vote(map)
            }
        }
    }

    /// Build and sign the transaction; `None` when the builder refuses it.
    pub fn build(
        &self,
        sender: &KeyPair,
        second: Option<&KeyPair>,
        nonce: u64,
        fee: u64,
        params: &Params,
    ) -> Option<Transaction> {
        let builder = self
            .builder()
            .network(params.network())
            .nonce(nonce)
            .fee(fee);
        match second {
            None => builder.build(sender, Aux::fixed(AUX), params).ok(),
            Some(second) => builder
                .build_second_signed(sender, Aux::fixed(AUX), second, Aux::fixed(AUX), params)
                .ok(),
        }
    }
}

/// The dynamic fee floor of the first milestone of `milestones_text` for a transaction of
/// `key` with `length` bytes, as the reference computes it (a JavaScript sum, rounded half up);
/// 0 when dynamic fees are off. Used only to aim fees at the floor.
pub fn aim_floor(milestones_text: &str, key: &str, length: usize) -> u64 {
    let list: Value = serde_json::from_str(milestones_text).unwrap_or(Value::Null);
    let fees = &list[0]["dynamicFees"];
    if fees["enabled"] != Value::Bool(true) || matches!(key, "burn" | "delegateResignation") {
        return 0;
    }
    let min_fee = fees["minFee"].as_i64().unwrap_or(0).max(1);
    let addon = fees["addonBytes"][key].as_i64().unwrap_or(0);
    let half = i64::try_from(length.div_ceil(2)).unwrap_or(0);
    u64::try_from((addon + half).saturating_mul(min_fee)).unwrap_or(0)
}

fn username() -> impl Strategy<Value = String> {
    "[a-z0-9!@$&_.]{1,20}"
}

fn memo_free_specs(ctx: &Arc<TxContext>) -> BoxedStrategy<TxSpec> {
    let recipients = ctx.recipients.clone();
    let transfer = prop_oneof![
        4 => 1usize..=3,
        2 => 1usize..=64,
        1 => 250usize..=256,
    ]
    .prop_flat_map(move |n| {
        let recipients = recipients.clone();
        prop::collection::vec((0usize..256, 1u64..=1_000_000_000), n).prop_map(move |items| {
            TxSpec::Transfer(
                items
                    .into_iter()
                    .map(|(i, amount)| (recipients[i], amount))
                    .collect(),
            )
        })
    });
    let votes = (1usize..=53).prop_flat_map(|n| {
        (super::split(n), prop::collection::vec("[a-z0-9_]{1,20}", n)).prop_map(
            |(bps, mut names)| {
                names.sort();
                names.dedup();
                let n = names.len();
                let mut bps = bps;
                bps.truncate(n);
                let sum: u32 = bps.iter().map(|b| u32::from(*b)).sum();
                if let Some(last) = bps.last_mut() {
                    *last = u16::try_from(u32::from(*last) + 10_000 - sum).unwrap_or(*last);
                }
                TxSpec::Vote(names.into_iter().zip(bps).collect())
            },
        )
    });
    prop_oneof![
        3 => transfer,
        1 => "[a-z]{1,12}".prop_map(|p| TxSpec::SecondSignature(format!("l1 second {p}"))),
        1 => username().prop_map(TxSpec::DelegateRegistration),
        1 => prop::option::of(prop_oneof![Just(ResignationType::PermanentResign), Just(ResignationType::NotResigned)])
            .prop_map(TxSpec::Resignation),
        1 => (1u64..=100_000_000_000).prop_map(TxSpec::Burn),
        2 => votes,
        1 => Just(TxSpec::Vote(Vec::new())),
    ]
    .boxed()
}

/// A dynamic-fee configuration inside the domain both sides accept: `enabled` a boolean,
/// `minFee` a safe integer, `addonBytes` safe integers under any keys.
fn dynamic_fees() -> impl Strategy<Value = Value> {
    let keys = [
        "transfer",
        "secondSignature",
        "delegateRegistration",
        "delegateResignation",
        "burn",
        "vote",
        "ipfs",
        "multiSignature",
    ];
    (
        prop::bool::weighted(0.85),
        prop_oneof![3 => 1i64..=20_000, 1 => -10i64..=0, 1 => Just(6_173i64)],
        prop::collection::vec(
            (
                prop::sample::select(keys.to_vec()),
                prop_oneof![0i64..=2_000_000, Just(-5i64)],
            ),
            0..8,
        ),
    )
        .prop_map(|(enabled, min_fee, addons)| {
            let mut addon = serde_json::Map::new();
            for (k, v) in addons {
                addon.insert(k.to_owned(), json!(v));
            }
            json!({"enabled": enabled, "minFee": min_fee, "addonBytes": addon})
        })
}

/// A case of P1: the rule height and the input of `fee.minimum`.
pub fn fee_cases(ctx: Arc<TxContext>) -> BoxedStrategy<(u64, Value)> {
    let memo = prop_oneof![
        Just(None),
        Just(Some(String::new())),
        "[ -~]{1,255}".prop_map(Some)
    ];
    (
        memo_free_specs(&ctx),
        prop::option::weighted(0.7, dynamic_fees()),
        memo,
        1u64..=1_000_000,
        1u64..=1_000_000_000,
    )
        .prop_filter_map(
            "the builder refused the transaction",
            move |(spec, fees, memo, nonce, fee)| {
                let text = match &fees {
                    Some(fees) => ctx.milestones_with("dynamicFees", fees.clone()),
                    None => ctx.milestones_text.clone(),
                };
                let milestones = ctx.load(&text)?;
                let params = milestones.at(2);
                let sender = keys("l1 fee sender");
                let builder = spec
                    .builder()
                    .network(params.network())
                    .nonce(nonce)
                    .fee(fee);
                let builder = match memo {
                    Some(m) => {
                        builder.memo(heartwood_crypto::transactions::types::Memo::new(&m).ok()?)
                    }
                    None => builder,
                };
                let tx = builder.build(&sender, Aux::fixed(AUX), params).ok()?;
                let mut input = json!({"hex": hex::encode(tx.bytes())});
                if fees.is_some() {
                    input["milestones"] = json!(text);
                }
                Some((2, input))
            },
        )
        .boxed()
}

/// The milestone variants of P7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// The devnet's milestones.
    Devnet,
    /// The devnet's with fewer active delegates, so small fixtures can resign and vote for too
    /// many.
    FewActive(u64),
    /// The devnet's with dynamic fees off.
    FeesOff,
}

/// A case of P7.
#[derive(Debug, Clone)]
pub struct HandlerCase {
    /// The input of `state.applyTransaction`.
    pub input: Value,
    /// The rule height: the last block's height plus one.
    pub height: u64,
    /// The transaction's sender.
    pub sender: Address,
    /// Whether the sender has no public key in the fixture (the reference binds it even when it
    /// refuses the transaction).
    pub sender_unbound: bool,
    /// What the case aims at.
    pub aim: String,
}

#[derive(Debug, Clone, Copy)]
enum Nonce {
    Next,
    Skip,
    Same,
}

#[derive(Debug, Clone, Copy)]
enum Fee {
    Floor(i64),
    Big,
}

#[derive(Debug, Clone, Copy)]
enum Second {
    Proper,
    Missing,
    Wrong,
}

#[derive(Debug, Clone)]
struct Plan {
    variant: Variant,
    sender: usize,
    kind: u8,
    nonce: Nonce,
    fee: Fee,
    second: Second,
    amount_edge: i64,
    pick: usize,
    revoke_offset: i64,
    shares: Vec<u16>,
}

fn plan() -> impl Strategy<Value = Plan> {
    (
        prop_oneof![5 => Just(Variant::Devnet), 3 => (1u64..=3).prop_map(Variant::FewActive), 1 => Just(Variant::FeesOff)],
        0usize..14,
        0u8..6,
        prop_oneof![6 => Just(Nonce::Next), 1 => Just(Nonce::Skip), 1 => Just(Nonce::Same)],
        prop_oneof![5 => Just(Fee::Big), 3 => (-1i64..=1).prop_map(Fee::Floor)],
        prop_oneof![6 => Just(Second::Proper), 1 => Just(Second::Missing), 1 => Just(Second::Wrong)],
        prop_oneof![5 => Just(i64::MIN), 1 => -1i64..=1],
        any::<usize>(),
        prop_oneof![3 => Just(i64::MIN), 1 => -1i64..=1],
        (1usize..=4).prop_flat_map(super::split),
    )
        .prop_map(|(variant, sender, kind, nonce, fee, second, amount_edge, pick, revoke_offset, shares)| Plan {
            variant,
            sender,
            kind,
            nonce,
            fee,
            second,
            amount_edge,
            pick,
            revoke_offset,
            shares,
        })
}

/// Cases of P7: a fixture, and a transaction of a kept type from one of its wallets (or from a
/// wallet it does not hold), aimed at the handlers' checks: the nonce, the fee around the floor,
/// amounts around the balance, identical, empty, unknown and too many votes, second signatures,
/// taken usernames, every resignation transition, revoke delays around the limit and burns
/// around the minimum, at heights 2 to 10^6 and under milestone variants.
pub fn handler_cases(ctx: Arc<TxContext>) -> BoxedStrategy<HandlerCase> {
    prop_oneof![3 => 1u32..=2_000, 1 => 1u32..=999_999]
        .prop_flat_map(|last| (Just(last), fixtures_at(last), plan()))
        .prop_filter_map(
            "the builder refused the transaction",
            move |(last, fixture, plan)| build_handler_case(&ctx, last, fixture, &plan),
        )
        .boxed()
}

fn build_handler_case(
    ctx: &TxContext,
    last: u32,
    mut fixture: Fixture,
    plan: &Plan,
) -> Option<HandlerCase> {
    let text = match plan.variant {
        Variant::Devnet => ctx.milestones_text.clone(),
        Variant::FewActive(n) => ctx.milestones_with("activeDelegates", json!(n)),
        Variant::FeesOff => {
            let list: Value = serde_json::from_str(&ctx.milestones_text).ok()?;
            let mut fees = list[0]["dynamicFees"].clone();
            fees["enabled"] = json!(false);
            ctx.milestones_with("dynamicFees", fees)
        }
    };
    let milestones = ctx.load(&text)?;
    let height = last.checked_add(1)?;
    let params = milestones.at(height);
    let active = params.active_delegates();
    let n = fixture.wallets.len();
    let stranger = plan.sender >= n;
    let index = if stranger { n } else { plan.sender };
    let sender_keys = if stranger {
        keys("l1 stranger")
    } else {
        wallet_keys(index)
    };
    let sender = address_of(sender_keys.public_key());
    let wallet = fixture.wallets.get(index).cloned();
    let balance = wallet.as_ref().map_or(0, |w| w.balance);
    let wallet_nonce = wallet.as_ref().map_or(0, |w| w.nonce);
    let delegates: Vec<String> = fixture
        .wallets
        .iter()
        .filter_map(|w| w.delegate.as_ref().map(|d| d.username.clone()))
        .collect();
    let resigned: Vec<String> = fixture
        .wallets
        .iter()
        .filter_map(|w| {
            w.delegate
                .as_ref()
                .filter(|d| d.resigned.is_some())
                .map(|d| d.username.clone())
        })
        .collect();
    let pick = |list: &[String]| (!list.is_empty()).then(|| list[plan.pick % list.len()].clone());
    let burn_minimum = params.burn_tx_amount();
    let mut aim = String::new();
    let spec = match plan.kind {
        0 => {
            let recipient = match plan.pick % 3 {
                0 => ctx.recipients[plan.pick % ctx.recipients.len()],
                1 => fixture.wallets[plan.pick % n].address,
                _ => sender,
            };
            aim.push_str("transfer");
            TxSpec::Transfer(vec![(recipient, 1 + (plan.pick as u64 % 10_000))])
        }
        1 => {
            aim.push_str("second-signature registration");
            if plan.pick.is_multiple_of(4) {
                aim.push_str(" of the sender's own key");
                TxSpec::SecondSignature(format!("l1 wallet {index}"))
            } else {
                TxSpec::SecondSignature(format!("l1 new second {}", plan.pick % 1_000))
            }
        }
        2 => {
            aim.push_str("delegate registration");
            match pick(&delegates).filter(|_| plan.pick.is_multiple_of(3)) {
                Some(taken) => {
                    aim.push_str(" of a taken name");
                    TxSpec::DelegateRegistration(taken)
                }
                None => TxSpec::DelegateRegistration(format!("l1new{}", plan.pick % 100_000)),
            }
        }
        3 => {
            let kind = match plan.pick % 3 {
                0 => None,
                1 => Some(ResignationType::PermanentResign),
                _ => Some(ResignationType::NotResigned),
            };
            aim.push_str(match kind {
                None => "temporary resignation",
                Some(ResignationType::PermanentResign) => "permanent resignation",
                _ => "revoke",
            });
            if plan.revoke_offset != i64::MIN
                && let Some(d) = fixture
                    .wallets
                    .get_mut(index)
                    .and_then(|w| w.delegate.as_mut())
            {
                let back = i64::from(REVOKE_DELAY) + plan.revoke_offset;
                let at = i64::from(last) - back;
                if at >= 1 {
                    d.resigned_at = u32::try_from(at).ok();
                    aim.push_str(&format!(" {back} blocks after the resignation"));
                }
            }
            TxSpec::Resignation(kind)
        }
        4 => {
            let amount = if plan.amount_edge == i64::MIN {
                aim.push_str("burn");
                burn_minimum.saturating_add(plan.pick as u64 % 1_000_000)
            } else {
                aim.push_str("burn around the minimum");
                u64::try_from(i64::try_from(burn_minimum).ok()? + plan.amount_edge)
                    .ok()?
                    .max(1)
            };
            TxSpec::Burn(amount)
        }
        _ => {
            let current: Vec<(String, u16)> = wallet
                .as_ref()
                .map(|w| w.votes.iter().map(|(k, v)| (k.clone(), *v)).collect())
                .unwrap_or_default();
            let votes = match plan.pick % 7 {
                0 if !current.is_empty() => {
                    aim.push_str("identical vote");
                    current
                }
                1 => {
                    aim.push_str("empty vote");
                    Vec::new()
                }
                2 => {
                    aim.push_str("vote for an unknown name");
                    vec![("l1nobody".to_owned(), 10_000)]
                }
                3 => match pick(&resigned) {
                    Some(name) => {
                        aim.push_str("vote for a resigned delegate");
                        vec![(name, 10_000)]
                    }
                    None => vec![("l1nobody".to_owned(), 10_000)],
                },
                4 => {
                    aim.push_str("vote for more delegates than are active");
                    let k = usize::try_from(active).ok()?.saturating_add(1).min(53);
                    let shares = super::split_now(k);
                    (0..k).map(|i| (format!("l1d{i}"), shares[i])).collect()
                }
                _ => {
                    aim.push_str("vote");
                    if delegates.is_empty() {
                        vec![("l1nobody".to_owned(), 10_000)]
                    } else {
                        let k = plan.shares.len().min(delegates.len());
                        let mut shares = plan.shares.clone();
                        shares.truncate(k);
                        let sum: u32 = shares.iter().map(|s| u32::from(*s)).sum();
                        if let Some(l) = shares.last_mut() {
                            *l = u16::try_from(u32::from(*l) + 10_000 - sum).ok()?;
                        }
                        (0..k)
                            .map(|i| {
                                (
                                    delegates[(plan.pick + i) % delegates.len()].clone(),
                                    shares[i],
                                )
                            })
                            .collect()
                    }
                }
            };
            TxSpec::Vote(votes)
        }
    };
    let nonce = match plan.nonce {
        Nonce::Next => wallet_nonce.checked_add(1)?,
        Nonce::Skip => {
            aim.push_str(", nonce skipped");
            wallet_nonce.checked_add(2)?
        }
        Nonce::Same => {
            aim.push_str(", nonce not above the wallet's");
            wallet_nonce
        }
    };
    let has_second = wallet
        .as_ref()
        .is_some_and(|w| w.second_public_key.is_some());
    let second = match (has_second, plan.second) {
        (true, Second::Proper) => Some(second_keys(index)),
        (true, Second::Missing) => {
            aim.push_str(", second signature missing");
            None
        }
        (true, Second::Wrong) | (false, Second::Wrong) => {
            aim.push_str(if has_second {
                ", wrong second signature"
            } else {
                ", unexpected second signature"
            });
            Some(keys("l1 wrong second"))
        }
        (false, _) => None,
    };
    let probe = spec.build(&sender_keys, second.as_ref(), nonce, 1, params)?;
    let floor = aim_floor(&text, spec.key(), probe.bytes().len());
    let mut fee = match plan.fee {
        Fee::Big => floor.max(1).saturating_add(10_000_000),
        Fee::Floor(d) => {
            aim.push_str(&format!(", fee at the floor {d:+}"));
            u64::try_from(i64::try_from(floor).ok()? + d).ok()?.max(1)
        }
    };
    let mut spec = spec;
    if plan.amount_edge != i64::MIN && plan.kind == 0 {
        // A transfer whose item and fee spend the balance, give or take one.
        let spend = i64::try_from(balance).ok()? - i64::try_from(fee).ok()? + plan.amount_edge;
        if spend >= 1 {
            let recipient = match &spec {
                TxSpec::Transfer(items) => items[0].0,
                _ => sender,
            };
            spec = TxSpec::Transfer(vec![(recipient, u64::try_from(spend).ok()?)]);
            aim.push_str(&format!(", balance spent {:+}", plan.amount_edge));
        } else {
            fee = fee.max(1);
        }
    }
    let tx = spec.build(&sender_keys, second.as_ref(), nonce, fee, params)?;
    if fixture.total_balance().is_none_or(|t| t > MAX_SUPPLY) {
        return None;
    }
    let mut input = json!({
        "fixture": fixture.to_json(),
        "hex": hex::encode(tx.bytes()),
        "lastBlockHeight": last,
    });
    if plan.variant != Variant::Devnet {
        input["milestones"] = json!(text);
    }
    Some(HandlerCase {
        input,
        height: u64::from(height),
        sender,
        sender_unbound: wallet.as_ref().is_none_or(|w| w.public_key.is_none()),
        aim,
    })
}
