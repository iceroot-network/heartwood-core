//! P21, the listings over the state: random wallet sets (attributes, balances with ties, resigned
//! delegates, usernames with digits and punctuation) in random index orders, random criteria,
//! `orderBy` chains and pages, for `api.delegatesPage` and `api.walletsPage`.
//!
//! A case is a fixture and a plan of seeds; the query is made from the two by plain functions, so
//! that a shrunk fixture keeps its query's shape. The criteria take their values from the fixture
//! (a name, an address, a key, a balance, a count) or from a few fixed ones, and a share of them
//! are refused on purpose (a `%` in a username, a range on a text value, a rank of zero).

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Map, Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;
use crate::fixture::Fixture;

/// The height the listings run at: past every resignation of the fixtures.
const HEIGHT: u64 = 1_000;

/// The versions a delegate is given.
const VERSIONS: [&str; 6] = [
    "4.3.0",
    "4.3.1",
    "4.10.0",
    "5.0.0-next.1",
    "1.0.0",
    "4.3.0-rc.2",
];

/// What a case is made of, besides its fixture.
#[derive(Debug, Clone)]
struct Plan {
    /// The route.
    route: u32,
    /// The seeds of the index orders: address, username, public key (0 keeps the fixture's).
    orders: [u32; 3],
    /// The seeds of each delegate's extras: rank, missed, productivity, version.
    extras: Vec<[u32; 4]>,
    /// The criteria: a kind and a seed.
    criteria: Vec<(u32, u32)>,
    /// The `orderBy` entries: a property and a direction.
    sort: Vec<(u32, bool)>,
    /// The paging: a limit, a page, an offset and which of them are named.
    page: [u32; 4],
}

fn plan() -> impl Strategy<Value = Plan> {
    (
        any::<u32>(),
        [any::<u32>(); 3],
        prop::collection::vec([any::<u32>(); 4], 12),
        prop::collection::vec((any::<u32>(), any::<u32>()), 0..=3),
        prop::collection::vec((any::<u32>(), any::<bool>()), 0..=3),
        [any::<u32>(); 4],
    )
        .prop_map(|(route, orders, extras, criteria, sort, page)| Plan {
            route,
            orders,
            extras,
            criteria,
            sort,
            page,
        })
}

/// One of `len` things (0 when there are none), by `seed`.
fn pick(seed: u32, len: usize) -> usize {
    if len == 0 { 0 } else { seed as usize % len }
}

/// A fixed shuffle of `0..n` by `seed`.
fn shuffled(n: usize, seed: u32) -> Vec<usize> {
    let mut list: Vec<usize> = (0..n).collect();
    let mut state = u64::from(seed) | 1;
    for i in (1..n).rev() {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let j = usize::try_from((state >> 33) % (i as u64 + 1)).unwrap_or(0);
        list.swap(i, j);
    }
    list
}

/// What the plan takes from the fixture.
struct Facts {
    usernames: Vec<String>,
    addresses: Vec<String>,
    keys: Vec<String>,
    balances: Vec<u64>,
    counts: Vec<u64>,
    seconds: Vec<String>,
}

impl Facts {
    fn of(fixture: &Fixture) -> Facts {
        let mut facts = Facts {
            usernames: Vec::new(),
            addresses: Vec::new(),
            keys: Vec::new(),
            balances: Vec::new(),
            counts: vec![0, 1, 2, 5, 10],
            seconds: Vec::new(),
        };
        for wallet in &fixture.wallets {
            facts.addresses.push(wallet.address.to_base58());
            facts.balances.push(wallet.balance);
            if let Some(key) = &wallet.public_key {
                facts.keys.push(key.to_hex());
            }
            if let Some(second) = &wallet.second_public_key {
                facts.seconds.push(second.to_hex());
            }
            if let Some(delegate) = &wallet.delegate {
                facts.usernames.push(delegate.username.clone());
                facts.counts.push(delegate.produced_blocks);
            }
        }
        facts
    }

    fn name(&self, seed: u32) -> String {
        match self.usernames.get(pick(seed, self.usernames.len() + 1)) {
            Some(name) => name.clone(),
            None => "nobody".to_owned(),
        }
    }

    fn address(&self, seed: u32) -> String {
        match self.addresses.get(pick(seed, self.addresses.len() + 1)) {
            Some(address) => address.clone(),
            None => "d".repeat(34),
        }
    }

    fn key(&self, seed: u32) -> String {
        match self.keys.get(pick(seed, self.keys.len() + 1)) {
            Some(key) => key.clone(),
            None => format!("02{}", "ab".repeat(32)),
        }
    }

    fn balance(&self, seed: u32) -> u64 {
        self.balances
            .get(pick(seed, self.balances.len() + 1))
            .copied()
            .unwrap_or(0)
    }

    fn count(&self, seed: u32) -> u64 {
        self.counts[pick(seed, self.counts.len())]
    }
}

/// `name` as a single value, a `from`, a `to` or both, by `seed`, over `values`.
fn ranged(name: &str, seed: u32, low: &str, high: &str) -> String {
    match seed % 5 {
        0 => format!("{name}={low}"),
        1 => format!("{name}.from={low}"),
        2 => format!("{name}.to={high}"),
        3 => format!("{name}.from={low}&{name}.to={high}"),
        _ => format!("{name}={low},{high}"),
    }
}

/// An ordered pair of two numbers from `seed`.
fn between(seed: u32, most: u64) -> (u64, u64) {
    let (a, b) = (
        u64::from(seed) % (most + 1),
        u64::from(seed >> 7) % (most + 1),
    );
    (a.min(b), a.max(b))
}

/// The criterion of `kind` over the delegates' properties.
fn delegate_criterion(kind: u32, seed: u32, facts: &Facts) -> String {
    let (low, high) = between(seed, 50);
    let (small, large) = between(seed, 100_000);
    let percent = |n: u64| format!("{}", n as f64 / 100.0);
    let (p_low, p_high) = between(seed, 10_000);
    match kind % 54 {
        0 => format!("username={}", facts.name(seed)),
        1 => format!("username={},{}", facts.name(seed), facts.name(seed >> 5)),
        2 => format!("address={}", facts.address(seed)),
        3 => format!("address={}%25", &facts.address(seed)[..3]),
        4 => format!("publicKey={}", facts.key(seed)),
        5 => format!("publicKey={}%25", &facts.key(seed)[..3]),
        6 => ranged("votesReceived.percent", seed, &percent(p_low), &percent(p_high)),
        7 => ranged(
            "votesReceived.votes",
            seed,
            &facts.balance(seed).to_string(),
            &facts.balance(seed >> 3).to_string(),
        ),
        8 => ranged("votesReceived.voters", seed, &low.to_string(), &high.to_string()),
        9 => ranged("rank", seed, &(low % 12 + 1).to_string(), &(high % 12 + 1).to_string()),
        10 => ["isResigned=true", "isResigned=false", "isResigned=true,false"][pick(seed, 3)]
            .to_owned(),
        11 => ["resignationType=permanent", "resignationType=temporary"][pick(seed, 2)].to_owned(),
        12 => ranged(
            "blocks.produced",
            seed,
            &facts.count(seed).to_string(),
            &facts.count(seed >> 3).to_string(),
        ),
        13 => ranged("blocks.missed", seed, &low.to_string(), &high.to_string()),
        14 => ranged("blocks.productivity", seed, &percent(p_low), &percent(p_high)),
        15 => ranged("forged.fees", seed, &small.to_string(), &large.to_string()),
        16 => ranged("forged.rewards", seed, &small.to_string(), &large.to_string()),
        17 => ranged("forged.burnedFees", seed, &small.to_string(), &large.to_string()),
        18 => ranged("forged.donations", seed, &small.to_string(), &large.to_string()),
        19 => ranged("forged.total", seed, &small.to_string(), &large.to_string()),
        20 => {
            let (a, b) = (VERSIONS[pick(seed, 6)], VERSIONS[pick(seed >> 4, 6)]);
            match seed % 3 {
                0 => format!("version={a}"),
                1 => format!("version={a},{b}"),
                _ => format!("version.from=4.3.0&version.to={b}"),
            }
        }
        // Refused, or an error, on purpose.
        21 => "username=%25a".to_owned(),
        22 => "username=a%25".to_owned(),
        23 => "rank=0".to_owned(),
        24 => "isResigned=maybe".to_owned(),
        25 => "blocks.last.id=".to_owned() + &"ab".repeat(32),
        26 => "blocks.last.height.from=1".to_owned(),
        27 => "votesReceived.percent=101".to_owned(),
        28 => "unknown=1".to_owned(),
        29 => "version=>=4.3.0".to_owned(),
        30 => "address=zz".to_owned(),
        31 => "forged.fees=-1".to_owned(),
        32 => format!("username={},nobody", facts.name(seed)),
        33 => "resignationType=neither".to_owned(),
        // The valid kinds again, so that most cases are answered with a page.
        other => delegate_criterion(other - 34, seed, facts),
    }
}

/// The criterion of `kind` over the wallets' properties.
fn wallet_criterion(kind: u32, seed: u32, facts: &Facts) -> String {
    let (small, large) = between(seed, 200);
    let (low, high) = (facts.balance(seed), facts.balance(seed >> 3));
    let (low, high) = (low.min(high), low.max(high));
    match kind % 34 {
        0 => format!("address={}", facts.address(seed)),
        1 => format!("address={},{}", facts.address(seed), facts.address(seed >> 5)),
        2 => format!("address={}%25", &facts.address(seed)[..3]),
        3 => format!("address=%25{}", &facts.address(seed)[28..]),
        4 => format!("publicKey={}", facts.key(seed)),
        5 => format!("publicKey={}%25", &facts.key(seed)[..3]),
        6 | 7 => ranged("balance", seed, &low.to_string(), &high.to_string()),
        8 => ranged("nonce", seed, &small.to_string(), &large.to_string()),
        9 => format!("attributes.delegate.username={}", facts.name(seed)),
        10 => format!("attributes.delegate.username=%25{}", &facts.name(seed)[..1]),
        11 => format!("attributes.delegate.producedBlocks={}", facts.count(seed)),
        12 => ranged(
            "attributes.delegate.producedBlocks",
            seed,
            &facts.count(seed).to_string(),
            &facts.count(seed >> 3).to_string(),
        ),
        13 => format!(
            "attributes.secondPublicKey={}",
            facts.seconds.get(pick(seed, facts.seconds.len().max(1))).map_or_else(
                || format!("03{}", "cd".repeat(32)),
                Clone::clone
            )
        ),
        14 => "attributes.secondPublicKey.from=1&attributes.secondPublicKey.to=2".to_owned(),
        15 => "attributes.delegate.username.from=a".to_owned(),
        16 => ranged(
            "attributes.delegate.voteBalance",
            seed,
            &low.to_string(),
            &high.to_string(),
        ),
        17 => "balance=-1".to_owned(),
        18 => "address=zz".to_owned(),
        19 => "unknown=1".to_owned(),
        20 => format!("attributes.delegate.rank={}", small % 12 + 1),
        21 => "nonce=x".to_owned(),
        // The valid kinds again, so that most cases are answered with a page.
        other => wallet_criterion(other - 22, seed, facts),
    }
}

const DELEGATE_PROPERTIES: [&str; 22] = [
    "username",
    "address",
    "publicKey",
    "votesReceived.percent",
    "votesReceived.votes",
    "votesReceived.voters",
    "rank",
    "isResigned",
    "resignationType",
    "blocks.produced",
    "blocks.missed",
    "blocks.productivity",
    "blocks.last.height",
    "blocks.last.id",
    "forged.fees",
    "forged.rewards",
    "forged.burnedFees",
    "forged.donations",
    "forged.total",
    "version",
    "blocks.last",
    "nosuch",
];

/// The wallets' sortable properties. `attributes` itself is not among them: it holds objects, on
/// which the reference's pagination throws for the pairs its tree compares, which only the
/// service itself reproduces (the vectors and the pagination's own branch of P21 cover it).
const WALLET_PROPERTIES: [&str; 13] = [
    "address",
    "publicKey",
    "balance",
    "nonce",
    "attributes.delegate.username",
    "attributes.delegate.producedBlocks",
    "attributes.delegate.voteBalance",
    "attributes.delegate.rank",
    "attributes.delegate.version",
    "attributes.delegate.lastBlock",
    "attributes.secondPublicKey",
    "attributes.nosuch",
    "nosuch",
];

/// The query text of a plan over a listing's properties.
fn query(plan: &Plan, facts: &Facts, delegates: bool) -> String {
    let mut parts: Vec<String> = plan
        .criteria
        .iter()
        .map(|(kind, seed)| {
            if delegates {
                delegate_criterion(*kind, *seed, facts)
            } else {
                wallet_criterion(*kind, *seed, facts)
            }
        })
        .collect();
    let properties: &[&str] = if delegates {
        &DELEGATE_PROPERTIES
    } else {
        &WALLET_PROPERTIES
    };
    if !plan.sort.is_empty() {
        let entries: Vec<String> = plan
            .sort
            .iter()
            .map(|(property, descending)| {
                let name = properties[pick(*property, properties.len())];
                match *property % 3 {
                    0 => name.to_owned(),
                    _ => format!("{name}:{}", if *descending { "desc" } else { "asc" }),
                }
            })
            .collect();
        parts.push(format!("orderBy={}", entries.join(",")));
    }
    let [limit, page, offset, named] = plan.page;
    if named % 3 != 0 {
        let limits = [1u32, 2, 3, 5, 10, 100, 4, 2, 1, 3, 0, 101];
        parts.push(format!("limit={}", limits[pick(limit, limits.len())]));
    }
    if named % 5 < 2 {
        parts.push(format!("page={}", page % 4 + u32::from(named % 7 != 0)));
    }
    if named % 11 == 0 {
        parts.push(format!("offset={}", offset % 8));
    }
    parts.join("&")
}

/// The index orders of a plan: permutations of the positions each index holds.
fn orders(plan: &Plan, fixture: &Fixture) -> Value {
    let holds = [
        (0..fixture.wallets.len()).collect::<Vec<_>>(),
        (0..fixture.wallets.len())
            .filter(|at| fixture.wallets[*at].delegate.is_some())
            .collect(),
        (0..fixture.wallets.len())
            .filter(|at| fixture.wallets[*at].public_key.is_some())
            .collect(),
    ];
    let mut made = Map::new();
    for ((name, seed), held) in ["address", "username", "publicKey"]
        .iter()
        .zip(plan.orders)
        .zip(holds)
    {
        if seed % 4 == 0 {
            continue;
        }
        let order: Vec<usize> = shuffled(held.len(), seed)
            .into_iter()
            .map(|at| held[at])
            .collect();
        made.insert((*name).to_owned(), json!(order));
    }
    Value::Object(made)
}

/// The delegates' extras of a plan: some ranks (distinct, from 1), missed blocks, productivity
/// and versions, each of them absent for a share of the delegates.
fn extras(plan: &Plan, fixture: &Fixture) -> Value {
    let mut made = Map::new();
    let named = fixture.wallets.iter().filter_map(|w| w.delegate.as_ref());
    let ranks = shuffled(12, plan.route);
    for (at, delegate) in named.enumerate() {
        let [rank, missed, productivity, version] = plan.extras[at % plan.extras.len()];
        let mut extra = Map::new();
        if rank % 4 != 0 && at < ranks.len() {
            extra.insert("rank".to_owned(), json!(ranks[at] + 1));
        }
        if missed % 3 != 0 {
            extra.insert("missed".to_owned(), json!(missed % 60));
        }
        if productivity % 3 != 0 {
            extra.insert("productivity".to_owned(), json!(productivity % 10_001));
        }
        if version % 3 != 0 {
            extra.insert("version".to_owned(), json!(VERSIONS[pick(version >> 2, 6)]));
        }
        made.insert(delegate.username.clone(), Value::Object(extra));
    }
    Value::Object(made)
}

/// The case of a fixture and a plan.
fn case(fixture: &Fixture, plan: &Plan) -> OpCase {
    let facts = Facts::of(fixture);
    let mut input = Map::new();
    input.insert("fixture".to_owned(), fixture.to_json());
    input.insert("orders".to_owned(), orders(plan, fixture));
    input.insert("extras".to_owned(), extras(plan, fixture));
    let route = plan.route % 20;
    let delegates = route < 12;
    let op = if delegates {
        "api.delegatesPage"
    } else {
        "api.walletsPage"
    };
    let path = match route {
        0..=6 => "/delegates".to_owned(),
        7 => format!("/delegates/{}", facts.name(plan.orders[0])),
        8 => format!("/delegates/{}", facts.address(plan.orders[0])),
        9 => format!("/delegates/{}", facts.key(plan.orders[0])),
        10 => format!("/delegates/{}/voters", facts.name(plan.orders[0])),
        11 => format!("/delegates/{}/voters", facts.address(plan.orders[1])),
        12..=17 => "/wallets".to_owned(),
        _ => "/wallets/top".to_owned(),
    };
    let voters = route == 10 || route == 11;
    let text = if (7..=9).contains(&route) {
        String::new()
    } else {
        query(plan, &facts, delegates && !voters)
    };
    input.insert("path".to_owned(), json!(path));
    input.insert("query".to_owned(), json!(text));
    (op, HEIGHT, Value::Object(input))
}

/// The cases of P21 over the delegate and wallet listings.
pub fn listing_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    (super::fixtures_at(HEIGHT as u32), plan())
        .prop_map(|(fixture, plan)| case(&fixture, &plan))
        .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, TestRunner};

    use super::*;

    fn cases(n: usize) -> Vec<OpCase> {
        let mut runner = TestRunner::new(Config::default());
        let strategy = (super::super::fixtures_at(HEIGHT as u32), plan());
        (0..n)
            .map(|_| {
                let (fixture, plan) = strategy.new_tree(&mut runner).unwrap().current();
                case(&fixture, &plan)
            })
            .collect()
    }

    #[test]
    fn the_cases_name_the_listing_routes_and_the_fixture() {
        for (op, height, input) in cases(200) {
            assert!(op == "api.delegatesPage" || op == "api.walletsPage");
            assert_eq!(height, HEIGHT);
            let path = input["path"].as_str().unwrap();
            assert!(path.starts_with("/delegates") || path.starts_with("/wallets"));
            assert!(input["fixture"].is_array());
            assert!(input["query"].is_string());
        }
    }

    #[test]
    fn the_cases_reach_every_route_and_the_refusals() {
        let all = cases(600);
        let paths: std::collections::BTreeSet<String> = all
            .iter()
            .map(|(_, _, input)| {
                let path = input["path"].as_str().unwrap();
                let parts: Vec<&str> = path.split('/').collect();
                match parts.as_slice() {
                    ["", "delegates"] => "/delegates".to_owned(),
                    ["", "delegates", _] => "/delegates/{id}".to_owned(),
                    ["", "delegates", _, _] => "/delegates/{id}/voters".to_owned(),
                    _ => path.to_owned(),
                }
            })
            .collect();
        assert_eq!(paths.len(), 5, "{paths:?}");
        let queries: Vec<&str> = all
            .iter()
            .map(|(_, _, input)| input["query"].as_str().unwrap())
            .collect();
        for wanted in ["orderBy=", "limit=", "page=", "%25", ".from=", "attributes."] {
            assert!(queries.iter().any(|q| q.contains(wanted)), "{wanted}");
        }
    }

    #[test]
    fn a_permutation_holds_each_position_once() {
        for seed in 0..50 {
            let mut list = shuffled(9, seed);
            list.sort_unstable();
            assert_eq!(list, (0..9).collect::<Vec<_>>());
        }
    }
}
