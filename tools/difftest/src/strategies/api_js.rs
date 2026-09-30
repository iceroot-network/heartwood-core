//! P22, the wallet search: random wallet sets in random address-index orders and random inputs
//! (short usernames, long address and key prefixes, mixed case), for `api.walletsLike`.
//!
//! A set holds 1 to 110 wallets: delegates in office, resigned delegates (temporary and
//! permanent), voters and plain wallets, so the comparator's three-way rule shows up with cycles.
//! The usernames come from a small alphabet, with digit runs, leading zeros, marks and integer-like
//! names (a vote's key order), so equal-primary pairs and shared prefixes are common.

use std::sync::{Arc, OnceLock};

use proptest::prelude::*;
use proptest::sample::Index;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;
use super::{address_of, wallet_keys};

/// The wallets a set is taken from: an address and a public key each, derived once.
fn pool() -> &'static [(String, String)] {
    static POOL: OnceLock<Vec<(String, String)>> = OnceLock::new();
    POOL.get_or_init(|| {
        (0..130)
            .map(|i| {
                let key = wallet_keys(1_000 + i);
                (
                    address_of(key.public_key()).to_base58(),
                    key.public_key().to_hex(),
                )
            })
            .collect()
    })
}

/// What a wallet of a set is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Delegate,
    Temporary,
    Permanent,
    Voter,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        3 => Just(Kind::Delegate),
        2 => Just(Kind::Temporary),
        2 => Just(Kind::Permanent),
        1 => Just(Kind::Plain),
        1 => Just(Kind::Voter),
    ]
}

fn username() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-c]{0,2}[0-9]{1,3}[_.]?",
        2 => "m0{0,2}[0-9]{1,2}",
        2 => "[_!.@&$][a-z0-9]{0,4}",
        1 => "[0-9]{1,3}",
        1 => "[a-z0-9!@$&_.]{1,20}",
        1 => "a{18,21}b?",
    ]
}

/// A member of a set: its pool index's slot, what it is, its name, a rank flag and a vote pick.
type Member = (Kind, String, bool, Index);

/// A prefix of `text` of 1 to `max` characters (at least one when there is one).
fn prefix(text: &str, cut: &Index, max: usize) -> String {
    let n = text.chars().count().min(max).max(1);
    text.chars().take(cut.index(n) + 1).collect()
}

/// The text in lower case, upper case or alternating.
fn recase(text: String, mode: u8) -> String {
    match mode {
        0 => text,
        1 => text.to_uppercase(),
        _ => text
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect(),
    }
}

/// The cases of P22.
pub fn wallet_search_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    let size = prop_oneof![5 => 1usize..=12, 2 => 13usize..=70, 1 => 71usize..=110];
    let set = size.prop_flat_map(move |n| {
        (
            prop::collection::vec((kind(), username(), prop::bool::ANY, any::<Index>()), n),
            prop::sample::subsequence((0..pool().len()).collect::<Vec<_>>(), n).prop_shuffle(),
            0u8..3,
        )
    });
    (set, any::<Index>(), any::<Index>(), 0u8..8, 0u8..3)
        .prop_map(|((members, slots, layout), pick, cut, input_kind, mode)| {
            let (fixture, ranks, names) = fixture(&members, slots, layout);
            let chosen = &fixture[pick.index(fixture.len())];
            let (address, key) = (
                chosen["a"].as_str().unwrap_or(""),
                chosen["pk"].as_str().unwrap_or(""),
            );
            let input = match input_kind {
                0 | 1 if !names.is_empty() => {
                    let name = &names[pick.index(names.len())];
                    prefix(name, &cut, 20)
                }
                2 => address.chars().take(21 + cut.index(14)).collect(),
                3 => key.chars().take(21 + cut.index(46)).collect(),
                4 => "a".repeat(20 + cut.index(2)),
                5 => String::new(),
                _ => prefix(["m", "a1", "_", "0", "zz", "d"][cut.index(6)], &cut, 20),
            };
            let mut args = json!({"fixture": fixture, "input": recase(input, mode)});
            if !ranks.is_empty() {
                args["ranks"] = Value::Array(ranks);
            }
            ("api.walletsLike", 2u64, args)
        })
        .boxed()
}

/// The fixture of a set (in `layout` 0 the wallets' order, in 1 ascending and in 2 descending by
/// address), the ranks of some delegates in office, and the delegates' names.
fn fixture(
    members: &[Member],
    slots: Vec<usize>,
    layout: u8,
) -> (Vec<Value>, Vec<Value>, Vec<String>) {
    let mut used = std::collections::BTreeSet::new();
    let mut wallets: Vec<Value> = Vec::new();
    let mut ranks = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for ((kind, name, ranked, vote), slot) in members.iter().zip(slots) {
        let (address, key) = &pool()[slot];
        let mut wallet =
            json!({"a": address, "pk": key, "bal": (1_000 + slot).to_string(), "n": "1"});
        let mut kind = *kind;
        if kind != Kind::Plain && kind != Kind::Voter && !used.insert(name.clone()) {
            kind = Kind::Plain;
        }
        match kind {
            Kind::Delegate | Kind::Temporary | Kind::Permanent => {
                wallet["u"] = json!(name);
                wallet["pb"] = json!(1);
                for field in ["ff", "fr", "bf", "dn"] {
                    wallet[field] = json!("5");
                }
                names.push(name.clone());
                if kind == Kind::Delegate && *ranked {
                    ranks.push(json!([address, ranks.len() + 1]));
                } else if kind != Kind::Delegate {
                    wallet["rs"] = json!(u8::from(kind == Kind::Permanent));
                }
            }
            Kind::Voter if !names.is_empty() => {
                wallet["v"] = json!([[names[vote.index(names.len())], 10_000]]);
            }
            _ => {}
        }
        wallets.push(wallet);
    }
    match layout {
        1 => wallets.sort_by(|a, b| a["a"].as_str().cmp(&b["a"].as_str())),
        2 => wallets.sort_by(|a, b| b["a"].as_str().cmp(&a["a"].as_str())),
        _ => {}
    }
    (wallets, ranks, names)
}
