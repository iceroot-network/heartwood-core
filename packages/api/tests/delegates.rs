//! The delegate resource and listing over the committed devnet chain: every delegate against the
//! reference's checkpoint, the resource's keys, the vote percent, the criteria and the order of
//! `GET /delegates` and `GET /delegates/{id}/voters` through a stand-in for the pagination
//! service, and the lookups of `GET /delegates/{id}`.

mod listing;
mod snapshot;

use std::sync::Arc;

use heartwood_api::controllers::{delegate_history, delegates, wallets_list};
use heartwood_api::pagination::Pagination;
use heartwood_api::resources::delegate::{delegate, supply};
use heartwood_api::schemas::validate_query;
use heartwood_api::search::criteria::{of_query, sorting_of};
use heartwood_api::search::{Criteria, Direction, plain};
use heartwood_api::{ApiContext, ApiError, RouteId};
use heartwood_api::{DelegateProduction, Hundredths, ProductivityTable};
use heartwood_state::{Resignation, StateView, Wallet};
use serde_json::{Value, json};
use tokio::sync::watch;

use listing::{query, request, stand_in};
use snapshot::chain;

fn delegate_wallets() -> Vec<Wallet> {
    chain().state.wallets.delegates().cloned().collect()
}

fn table() -> ProductivityTable {
    let mut table = ProductivityTable::default();
    let production = |missed, points| DelegateProduction {
        missed: Some(missed),
        productivity: Some(Hundredths(points)),
    };
    table
        .by_username
        .insert("genesis_1".into(), production(2, 9950));
    table
        .by_username
        .insert("genesis_2".into(), production(0, 10000));
    table.by_username.insert(
        "genesis_3".into(),
        DelegateProduction {
            missed: Some(1),
            productivity: None,
        },
    );
    table
}

/// The delegates page of `raw` over `wallets`, as the route computes it, with the stand-in pager.
fn page(wallets: &[Wallet], raw: &str) -> Result<(Vec<Value>, u64), ApiError> {
    let query = query(RouteId::DelegatesIndex, raw)?;
    let pagination = Pagination::of(&query);
    let sorting = sorting_of(&query, &[]);
    let chain = chain();
    let pool = chain.pool(Vec::new());
    let supply = supply(&chain.state.wallets);
    let parts = (pagination, sorting, &[of_query(&query)][..]);
    let page = delegates::delegates_page(wallets.iter(), supply, &pool, &table(), parts, stand_in)?;
    Ok((
        page.results.into_iter().map(plain).collect(),
        page.total_count,
    ))
}

fn names(results: &[Value]) -> Vec<&str> {
    results
        .iter()
        .map(|resource| resource["username"].as_str().unwrap())
        .collect()
}

fn resource_of(wallet: &Wallet, supply: u128) -> Result<Value, ApiError> {
    let pool = chain().pool(Vec::new());
    delegate(wallet, supply, &pool, &table()).map(plain)
}

#[test]
fn every_delegate_equals_the_checkpoint() {
    let chain = chain();
    let supply = supply(&chain.state.wallets);
    let expected = chain.checkpoint["delegates"].as_array().unwrap();
    assert_eq!(expected.len(), 56);
    for reference in expected {
        let name = reference["username"].as_str().unwrap();
        let wallet = chain.state.wallets.find_by_username(name).unwrap();
        let ours = resource_of(wallet, supply).unwrap();
        assert_eq!(ours["address"], reference["address"], "{name}");
        assert_eq!(ours["publicKey"], reference["publicKey"], "{name}");
        assert_eq!(ours["rank"], reference["rank"], "{name}");
        assert_eq!(ours["votesReceived"]["votes"], reference["votes"], "{name}");
        assert_eq!(
            ours["votesReceived"]["voters"], reference["voters"],
            "{name}"
        );
        assert_eq!(ours["isResigned"], reference["isResigned"], "{name}");
        assert_eq!(ours["blocks"]["produced"], reference["produced"], "{name}");
        assert_eq!(ours["blocks"]["last"], reference["lastBlock"], "{name}");
        assert_eq!(ours["forged"], reference["forged"], "{name}");
        assert_eq!(ours["version"], reference["version"], "{name}");
    }
}

#[test]
fn the_resource_keeps_the_reference_keys_and_leaves_out_what_is_undefined() {
    let chain = chain();
    let supply = supply(&chain.state.wallets);
    let wallet = chain.state.wallets.find_by_username("genesis_1").unwrap();
    let ours = resource_of(wallet, supply).unwrap();
    let keys = |value: &Value| {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    let all = [
        "username",
        "address",
        "publicKey",
        "votesReceived",
        "rank",
        "isResigned",
        "blocks",
        "forged",
        "version",
    ];
    assert_eq!(keys(&ours), all);
    assert_eq!(keys(&ours["votesReceived"]), ["percent", "votes", "voters"]);
    assert_eq!(
        keys(&ours["blocks"]),
        ["produced", "missed", "productivity", "last"]
    );
    assert_eq!(
        keys(&ours["forged"]),
        ["fees", "burnedFees", "rewards", "donations", "total"]
    );
    assert_eq!(ours["blocks"]["missed"], 2);
    assert_eq!(ours["blocks"]["productivity"], 99.5);
    let mut bare = wallet.clone();
    let record = bare.delegate_mut().unwrap();
    (record.rank, record.last_block, record.resigned) = (None, None, Some(Resignation::Permanent));
    let pool = heartwood_pool::PoolView {
        online: Vec::new(),
        ..chain.pool(Vec::new())
    };
    let ours = plain(delegate(&bare, supply, &pool, &ProductivityTable::default()).unwrap());
    assert_eq!(
        keys(&ours),
        [
            "username",
            "address",
            "publicKey",
            "votesReceived",
            "isResigned",
            "resignationType",
            "blocks",
            "forged"
        ]
    );
    assert_eq!(keys(&ours["blocks"]), ["produced"]);
    assert_eq!(ours["resignationType"], "permanent");
    // A whole number is written as one, as JavaScript writes it.
    assert!(!ours["votesReceived"]["percent"].to_string().contains('.'));
}

#[test]
fn the_vote_percent_is_the_reference_s_arithmetic() {
    let chain = chain();
    let mut wallet = chain
        .state
        .wallets
        .find_by_username("genesis_1")
        .unwrap()
        .clone();
    let cases: [(u64, u128, Result<f64, ()>); 9] = [
        (9, 800, Ok(1.13)),
        (5, 800, Ok(0.63)),
        (1, 3, Ok(33.33)),
        (3, 7, Ok(42.85)),
        (0, 1000, Ok(0.0)),
        (100, 1000, Ok(10.0)),
        (1, 0, Err(())),
        (1_000_000, 1, Err(())),
        (
            1,
            100_000_000_000_000_000_000_000_000_000_000_000_000,
            Ok(0.0),
        ),
    ];
    for (votes, supply, expected) in cases {
        wallet.delegate_mut().unwrap().vote_balance = votes;
        let made = resource_of(&wallet, supply)
            .map(|resource| resource["votesReceived"]["percent"].as_f64().unwrap());
        assert_eq!(
            made.map_err(|error| error.status == 500).map_err(|_| ()),
            expected,
            "{votes} of {supply}"
        );
    }
}

#[test]
fn the_forged_total_may_be_negative_and_is_exact() {
    let chain = chain();
    let mut wallet = chain
        .state
        .wallets
        .find_by_username("genesis_1")
        .unwrap()
        .clone();
    let record = wallet.delegate_mut().unwrap();
    (
        record.forged_fees,
        record.burned_fees,
        record.forged_rewards,
        record.donations,
    ) = (10, 5, 3, 20);
    let ours = resource_of(&wallet, supply(&chain.state.wallets)).unwrap();
    assert_eq!(ours["forged"]["total"], "-12");
    let record = wallet.delegate_mut().unwrap();
    (
        record.forged_fees,
        record.burned_fees,
        record.forged_rewards,
        record.donations,
    ) = (u64::MAX, 0, u64::MAX, 0);
    let ours = resource_of(&wallet, supply(&chain.state.wallets)).unwrap();
    assert_eq!(ours["forged"]["total"], "36893488147419103230");
}

#[test]
fn the_default_order_is_rank_then_name_and_the_page_is_counted() {
    let wallets = delegate_wallets();
    let (all, total) = page(&wallets, "limit=100").unwrap();
    assert_eq!((all.len(), total), (56, 56));
    // The two delegates without a rank are last, by name.
    let ranks: Vec<Option<u64>> = all.iter().map(|r| r["rank"].as_u64()).collect();
    assert!(
        ranks[..54].windows(2).all(|pair| pair[0] < pair[1]),
        "{ranks:?}"
    );
    assert_eq!(ranks[54..], [None, None]);
    assert!(all[54]["username"].as_str() < all[55]["username"].as_str());
    let (second, total) = page(&wallets, "limit=10&page=2").unwrap();
    assert_eq!((names(&second), total), (names(&all[10..20]), 56));
    let (offset, _) = page(&wallets, "limit=5&offset=7&page=9").unwrap();
    assert_eq!(names(&offset), names(&all[7..12]));
    let (beyond, total) = page(&wallets, "limit=10&page=99").unwrap();
    assert_eq!((beyond.len(), total), (0, 56));
}

#[test]
fn ties_follow_the_given_order_then_rank() {
    let wallets = delegate_wallets();
    let (voters, _) = page(&wallets, "orderBy=votesReceived.voters:desc&limit=100").unwrap();
    let key = |r: &Value| {
        (
            r["votesReceived"]["voters"].as_u64().unwrap(),
            r["rank"].as_u64(),
        )
    };
    let keys: Vec<_> = voters.iter().map(key).collect();
    // Within one count the ranks rise, and a delegate without a rank comes after those with one.
    let rises = |a: Option<u64>, b: Option<u64>| a.is_some() && (b.is_none() || a < b);
    assert!(
        keys.windows(2).all(|p| p[0].0 > p[1].0
            || (p[0].0 == p[1].0 && (rises(p[0].1, p[1].1) || p[0].1 == p[1].1))),
        "{keys:?}"
    );
    let (nested, _) = page(
        &wallets,
        "orderBy=forged.total:asc,blocks.produced:desc&limit=100",
    )
    .unwrap();
    assert_eq!(nested.len(), 56);
    let (by_name, _) = page(&wallets, "orderBy=username:desc&limit=3").unwrap();
    assert_eq!(names(&by_name), ["tx1n3233", "tx1n2290", "tx1n1160"]);
    // A boolean order: the false ones, then the true ones.
    let (resigned, _) = page(&wallets, "orderBy=isResigned:desc&limit=100").unwrap();
    assert_eq!(resigned[0]["isResigned"], true);
    assert_eq!(resigned[2]["isResigned"], false);
}

#[test]
fn criteria_narrow_the_listing_as_the_reference_reads_them() {
    let wallets = delegate_wallets();
    let count = |raw: &str| page(&wallets, raw).map(|(_, total)| total);
    assert_eq!(count("username=genesis_1"), Ok(1));
    assert_eq!(count("username=genesis_1,genesis_2,nobody"), Ok(2));
    assert_eq!(count("isResigned=false"), Ok(54));
    assert_eq!(count("isResigned=true"), Ok(2));
    assert_eq!(count("rank=1"), Ok(1));
    assert_eq!(count("rank.from=10&rank.to=19"), Ok(10));
    assert_eq!(count("rank.from=55"), Ok(0));
    assert_eq!(
        count("blocks.produced.from=6"),
        count("blocks.produced.from=6&isResigned=false")
    );
    assert_eq!(
        count("votesReceived.votes.from=1&votesReceived.voters.from=2"),
        count("votesReceived.voters.from=2&votesReceived.votes.from=1")
    );
    assert_eq!(count("version=4.3.1"), Ok(53));
    assert_eq!(count("version.from=4.4.0"), Ok(0));
    assert_eq!(count("version.to=4.3.1"), Ok(53));
    assert_eq!(count("blocks.missed=0"), Ok(1));
    assert_eq!(
        count("blocks.productivity.from=99.5&blocks.productivity.to=99.5"),
        Ok(1)
    );
    assert_eq!(count("resignationType=permanent").map(|n| n <= 2), Ok(true));
    // A delegate with no missed blocks value never matches, whatever the criterion.
    assert_eq!(count("blocks.missed.from=0"), Ok(3));
    // A criterion that is an object on a value that is a string is the reference's thrown error.
    assert_eq!(
        count("blocks.last.height=1").map_err(|e| e.status),
        Err(500)
    );
    assert_eq!(count("username=%25").map_err(|e| e.status), Err(422));
    assert_eq!(count("rank=0").map_err(|e| e.status), Err(422));
    assert_eq!(count("nothing=1").map_err(|e| e.status), Err(422));
    assert_eq!(count("orderBy=version.x").map_err(|e| e.status), Err(422));
    assert_eq!(count("username=%").map_err(|e| e.status), Err(422));
}

#[test]
fn a_criterion_on_a_missing_value_never_matches_and_never_throws() {
    let mut wallets = delegate_wallets();
    for wallet in &mut wallets {
        wallet.delegate_mut().unwrap().last_block = None;
    }
    let count = |raw: &str| page(&wallets, raw).map(|(_, total)| total);
    assert_eq!(count("blocks.last.height=1"), Ok(0));
    let ranks = |raw: &str| page(&wallets, raw).map(|(r, _)| names(&r).len());
    assert_eq!(ranks("orderBy=blocks.last.id:asc&limit=100"), Ok(56));
}

#[test]
fn resigned_delegates_show_their_kind() {
    let mut wallets = delegate_wallets();
    for wallet in &mut wallets {
        wallet.delegate_mut().unwrap().resigned = None;
    }
    wallets[0].delegate_mut().unwrap().resigned = Some(Resignation::Temporary);
    wallets[1].delegate_mut().unwrap().resigned = Some(Resignation::Permanent);
    let (found, total) = page(&wallets, "isResigned=true&orderBy=resignationType:asc").unwrap();
    assert_eq!(total, 2);
    let kinds: Vec<_> = found.iter().map(|r| r["resignationType"].clone()).collect();
    assert_eq!(kinds, [json!("permanent"), json!("temporary")]);
    assert_eq!(page(&wallets, "resignationType=temporary").unwrap().1, 1);
}

#[test]
fn the_voters_of_a_delegate_are_the_wallets_voting_for_the_name() {
    let chain = chain();
    let name = "genesis_34";
    let raw = "orderBy=balance:asc&limit=100";
    let query = query(RouteId::DelegatesVoters, raw).unwrap();
    let pagination = Pagination::of(&query);
    let sorting = sorting_of(&query, &[("balance", Direction::Desc)]);
    let mut voting = serde_json::Map::new();
    voting.insert("votingFor".into(), json!([{ name: "*" }]));
    let criteria = [of_query(&query), Criteria(voting)];
    let wallets = chain
        .state
        .wallets
        .iter()
        .filter(|wallet| wallet.public_key().is_some());
    let pool = chain.pool(Vec::new());
    let parts = (pagination, sorting, &criteria[..]);
    let page = wallets_list::wallets_page(wallets, &pool, &table(), parts, stand_in).unwrap();
    assert_eq!(page.total_count, 1);
    assert_eq!(
        page.results[0]["address"],
        "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY"
    );
    // Only wallets with a public key are searched: the same voters, by that index.
    let voting: Vec<_> = chain.checkpoint["wallets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|wallet| wallet["votingFor"].get(name).is_some())
        .collect();
    assert_eq!(voting.len(), 1);
}

async fn ask(route: RouteId, id: &str) -> Result<Value, ApiError> {
    let chain = chain();
    let context = snapshot::context(
        "delegates",
        chain.pool(Vec::new()),
        Vec::new(),
        snapshot::local_config(),
    );
    let request = request(route, Some(id), json!({}));
    let ctx = Arc::new(context);
    match route {
        RouteId::DelegatesVoters => delegates::voters(ctx, request).await,
        _ => delegates::show(ctx, request).await,
    }
    .map(|reply| reply.body)
}

#[tokio::test]
async fn a_delegate_is_found_by_name_address_and_key_and_nothing_else_is() {
    let chain = chain();
    let address = "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY";
    let key = "02b85b15ff121c2408ea682a6f6bf1a1079bf42537cdeef9e039befa26671949df";
    let by_name = ask(RouteId::DelegatesShow, "genesis_34").await.unwrap();
    assert_eq!(by_name["data"]["address"], address);
    assert_eq!(by_name["data"]["blocks"]["produced"], 5);
    assert_eq!(by_name["data"]["votesReceived"]["votes"], "936000000");
    assert_eq!(ask(RouteId::DelegatesShow, address).await.unwrap(), by_name);
    assert_eq!(ask(RouteId::DelegatesShow, key).await.unwrap(), by_name);
    let wallet = chain.state.wallets.find_by_username("genesis_34").unwrap();
    let expected = resource_of(wallet, supply(&chain.state.wallets)).unwrap();
    assert_eq!(by_name["data"]["forged"], expected["forged"]);
    // A wallet that is no delegate, an unknown valid address and an unknown name.
    let voter = chain.checkpoint["wallets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|wallet| {
            wallet["attributes"].get("delegate").is_none() && wallet["publicKey"].is_string()
        })
        .unwrap()["address"]
        .as_str()
        .unwrap()
        .to_owned();
    for id in [
        voter.as_str(),
        "dNkDoLLMAzV8xUtxjjkmY9WG5qwA3Gxki2",
        "nobody",
        "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjZ",
    ] {
        let missing = ask(RouteId::DelegatesShow, id).await.unwrap_err();
        assert_eq!(
            (missing.status, missing.message.as_str()),
            (404, "Delegate not found"),
            "{id}"
        );
        let missing = ask(RouteId::DelegatesVoters, id).await.unwrap_err();
        assert_eq!(missing.status, 404, "{id}");
    }
}

/// The chain's state with every balance spent, so that no supply is behind any vote percent, and
/// `genesis_34` without votes: the reference's vote percent then divides by zero.
fn without_supply() -> StateView {
    let mut state = chain().state.clone();
    let funded: Vec<_> = (state.wallets.iter())
        .map(|wallet| (*wallet.address(), wallet.balance()))
        .collect();
    for (address, balance) in funded {
        let wallet = state.wallets.wallet_mut(&address).unwrap();
        wallet.debit(balance).unwrap();
    }
    let delegate = state.wallets.find_by_username("genesis_34").unwrap();
    let address = *delegate.address();
    let wallet = state.wallets.wallet_mut(&address).unwrap();
    wallet.delegate_mut().unwrap().vote_balance = 0;
    state
}

/// The answer of `route` for `id` over `ctx`, with the route's default query.
async fn ask_over(ctx: &Arc<ApiContext>, route: RouteId, id: &str) -> Result<Value, ApiError> {
    let request = request(route, Some(id), query(route, "").unwrap());
    let ctx = Arc::clone(ctx);
    match route {
        RouteId::DelegatesShow => delegates::show(ctx, request).await,
        RouteId::DelegatesVoters => delegates::voters(ctx, request).await,
        RouteId::DelegatesBlocks => delegate_history::blocks(ctx, request).await,
        _ => delegate_history::missed_blocks(ctx, request).await,
    }
    .map(|reply| reply.body)
}

#[tokio::test]
async fn every_route_of_one_delegate_answers_500_where_its_resource_cannot_be_made() {
    // The reference builds a delegate's resource before any route of one delegate answers, the
    // voters and the blocks included, which use only the name: with no supply behind the vote
    // percent that resource throws, and each route answers 500. A wallet that is no delegate is
    // still not found first.
    let chain = chain();
    let context = |name: &str, state: StateView| {
        let mut context = snapshot::context(
            name,
            chain.pool(Vec::new()),
            Vec::new(),
            snapshot::local_config(),
        );
        context.state = watch::channel(Arc::new(state)).1;
        Arc::new(context)
    };
    let state = without_supply();
    assert_eq!(supply(&state.wallets), 0);
    let empty = context("delegates-without-supply", state);
    let funded = context("delegates-with-supply", chain.state.clone());
    let routes = [
        RouteId::DelegatesShow,
        RouteId::DelegatesVoters,
        RouteId::DelegatesBlocks,
        RouteId::DelegatesMissedBlocks,
    ];
    let named = [
        "genesis_34",
        "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY",
        "02b85b15ff121c2408ea682a6f6bf1a1079bf42537cdeef9e039befa26671949df",
        "genesis_1",
    ];
    for route in routes {
        for id in named {
            let refused = ask_over(&empty, route, id).await.unwrap_err();
            assert_eq!(refused.status, 500, "{route:?} {id}");
            assert!(ask_over(&funded, route, id).await.is_ok(), "{route:?} {id}");
        }
        for id in ["nobody", "dNkDoLLMAzV8xUtxjjkmY9WG5qwA3Gxki2"] {
            let missing = ask_over(&empty, route, id).await.unwrap_err();
            assert_eq!(
                (missing.status, missing.message.as_str()),
                (404, "Delegate not found"),
                "{route:?} {id}"
            );
        }
    }
}

#[test]
fn the_routes_validate_the_delegate_id() {
    let refused = |id: &str| {
        let mut params = serde_json::Map::new();
        params.insert("id".into(), Value::from(id));
        heartwood_api::schemas::validate_params(RouteId::DelegatesShow, &params)
            .map_err(|e| e.status)
    };
    assert_eq!(refused("genesis_1"), Ok(()));
    assert_eq!(refused("dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY"), Ok(()));
    assert_eq!(refused("GENESIS"), Err(422));
    assert_eq!(refused("a%"), Err(422));
    assert_eq!(
        validate_query(RouteId::DelegatesIndex, &json!({})).map(|q| q["orderBy"].clone()),
        Ok(json!([]))
    );
}
