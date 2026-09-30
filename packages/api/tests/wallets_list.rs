//! The wallet listing over the committed devnet chain: every wallet against the reference's
//! checkpoint through the listing, the default order (balance, highest first; ties in the index
//! order), the criteria on the wallet's own keys and on `attributes.*`, and the order on nested
//! paths, through a stand-in for the pagination service.

mod listing;
mod snapshot;

use heartwood_api::controllers::wallets_list::wallets_page;
use heartwood_api::pagination::Pagination;
use heartwood_api::search::criteria::{of_query, sorting_of};
use heartwood_api::search::{Direction, plain};
use heartwood_api::{ApiError, DelegateProduction, Hundredths, ProductivityTable, RouteId};
use serde_json::Value;

use listing::{query, stand_in};
use snapshot::chain;

fn table() -> ProductivityTable {
    let mut table = ProductivityTable::default();
    let production = DelegateProduction {
        missed: Some(2),
        productivity: Some(Hundredths(9950)),
    };
    table.by_username.insert("genesis_1".into(), production);
    table
}

fn page_with(
    route: RouteId,
    raw: &str,
    table: &ProductivityTable,
) -> Result<(Vec<Value>, u64), ApiError> {
    let query = query(route, raw)?;
    let pagination = Pagination::of(&query);
    let sorting = sorting_of(&query, &[("balance", Direction::Desc)]);
    let chain = chain();
    let pool = chain.pool(Vec::new());
    let parts = (pagination, sorting, &[of_query(&query)][..]);
    let page = wallets_page(chain.state.wallets.iter(), &pool, table, parts, stand_in)?;
    Ok((
        page.results.into_iter().map(plain).collect(),
        page.total_count,
    ))
}

fn page(raw: &str) -> Result<(Vec<Value>, u64), ApiError> {
    page_with(RouteId::WalletsIndex, raw, &ProductivityTable::default())
}

fn addresses(results: &[Value]) -> Vec<&str> {
    results
        .iter()
        .map(|wallet| wallet["address"].as_str().unwrap())
        .collect()
}

#[test]
fn every_wallet_of_the_listing_equals_the_reference() {
    let (all, total) = page("limit=100&page=1").unwrap();
    assert_eq!((all.len(), total), (100, 350));
    let mut listed = Vec::new();
    for number in 1..=4 {
        listed.extend(page(&format!("limit=100&page={number}")).unwrap().0);
    }
    assert_eq!(listed.len(), 350);
    let expected = chain().checkpoint["wallets"].as_array().unwrap();
    for wallet in expected {
        let found = listed
            .iter()
            .find(|ours| ours["address"] == wallet["address"])
            .unwrap();
        assert_eq!(found, wallet);
    }
}

#[test]
fn the_default_order_is_balance_highest_first_and_ties_keep_the_index_order() {
    let (all, _) = page("limit=100").unwrap();
    let balance = |wallet: &Value| wallet["balance"].as_str().unwrap().parse::<u128>().unwrap();
    assert!(
        all.windows(2)
            .all(|pair| balance(&pair[0]) >= balance(&pair[1]))
    );
    let index: Vec<String> = chain()
        .state
        .wallets
        .iter()
        .map(|w| w.address().to_base58())
        .collect();
    let place = |wallet: &Value| {
        index
            .iter()
            .position(|a| a == wallet["address"].as_str().unwrap())
            .unwrap()
    };
    for pair in all.windows(2) {
        if balance(&pair[0]) == balance(&pair[1]) {
            assert!(place(&pair[0]) < place(&pair[1]));
        }
    }
    // Ascending is the same list backwards except within ties.
    let (asc, _) = page("orderBy=balance:asc&limit=100").unwrap();
    assert!(balance(&asc[0]) <= balance(&asc[99]));
}

#[test]
fn the_criteria_of_the_wallet_keys() {
    let count = |raw: &str| page(raw).map(|(_, total)| total);
    let address = "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY";
    let key = "02b85b15ff121c2408ea682a6f6bf1a1079bf42537cdeef9e039befa26671949df";
    assert_eq!(count(&format!("address={address}")), Ok(1));
    assert_eq!(
        count(&format!(
            "address={address},dNkDoLLMAzV8xUtxjjkmY9WG5qwA3Gxki2"
        )),
        Ok(1)
    );
    assert_eq!(count("address=dDneg%"), Ok(1));
    assert_eq!(count("address=%"), Ok(350));
    assert_eq!(count(&format!("publicKey={key}")), Ok(1));
    assert_eq!(count("publicKey=02b85b%"), Ok(1));
    assert_eq!(
        count("balance=936000000"),
        count("balance.from=936000000&balance.to=936000000")
    );
    assert_eq!(count("balance.from=1&balance.to=0"), Ok(0));
    assert_eq!(
        count("balance.to=0").unwrap() + count("balance.from=1").unwrap(),
        350
    );
    assert_eq!(
        count("nonce.from=1"),
        count("nonce.from=1&nonce.to=99999999999999999999")
    );
    assert_eq!(count("nonce=-1").map_err(|e| e.status), Err(422));
    assert_eq!(count("nothing=1").map_err(|e| e.status), Err(422));
}

#[test]
fn the_criteria_reach_into_the_attributes() {
    let count = |raw: &str| page(raw).map(|(_, total)| total);
    assert_eq!(count("attributes.delegate.username=genesis_1"), Ok(1));
    assert_eq!(
        count("attributes.delegate.username=genesis_1%"),
        count("attributes.delegate.username=genesis_1%,genesis_1%")
    );
    assert_eq!(
        count("attributes.delegate.rank.from=1&attributes.delegate.rank.to=10"),
        Ok(10)
    );
    assert_eq!(
        count("attributes.delegate.voteBalance.from=1"),
        count("attributes.delegate.voteBalance.from=1&attributes.delegate.voters.from=1")
    );
    assert_eq!(count("attributes.delegate.missedBlocks=2"), Ok(0));
    // `*` reads any of an object's properties.
    assert_eq!(count("attributes.*=genesis_1"), Ok(0));
    // A value of one type against a criterion of another is the reference's thrown error.
    assert_eq!(
        count("attributes.delegate.username.from=a").map_err(|e| e.status),
        Err(500)
    );
}

#[test]
fn the_delegates_missed_blocks_and_productivity_come_from_the_table() {
    let with = |raw: &str| page_with(RouteId::WalletsIndex, raw, &table());
    let (found, total) = with("attributes.delegate.username=genesis_1").unwrap();
    assert_eq!(total, 1);
    let delegate = &found[0]["attributes"]["delegate"];
    assert_eq!(
        (
            delegate["missedBlocks"].clone(),
            delegate["productivity"].clone()
        ),
        (2.into(), serde_json::json!(99.5))
    );
    let keys: Vec<_> = delegate.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys[keys.len() - 2..], ["missedBlocks", "productivity"]);
    assert_eq!(
        with("attributes.delegate.productivity.from=99.5")
            .unwrap()
            .1,
        1
    );
    assert_eq!(
        with("attributes.delegate.missedBlocks.from=0").unwrap().1,
        1
    );
}

#[test]
fn the_order_on_nested_and_attribute_paths() {
    let (by_rank, _) = page("orderBy=attributes.delegate.rank:asc&limit=3").unwrap();
    let ranks: Vec<_> = by_rank
        .iter()
        .map(|w| w["attributes"]["delegate"]["rank"].as_u64())
        .collect();
    assert_eq!(ranks, [Some(1), Some(2), Some(3)]);
    // Wallets without the value are last, in both directions.
    let (desc, _) = page("orderBy=attributes.delegate.rank:desc&limit=100").unwrap();
    assert_eq!(desc[0]["attributes"]["delegate"]["rank"], 54);
    let (last, _) = page("orderBy=attributes.delegate.rank:desc&limit=100&page=4").unwrap();
    assert!(
        last.iter()
            .all(|wallet| wallet["attributes"].get("delegate").is_none())
    );
    let (by_name, _) = page("orderBy=attributes.delegate.username:asc&limit=2").unwrap();
    assert_eq!(
        by_name[0]["attributes"]["delegate"]["username"],
        "genesis_1"
    );
    let (by_votes, _) = page("orderBy=attributes.delegate.voteBalance:desc&limit=1").unwrap();
    assert_eq!(
        by_votes[0]["attributes"]["delegate"]["voteBalance"],
        "5826564306300"
    );
    // A path that is not one of the resource's is refused, and so is a direction.
    assert_eq!(page("orderBy=nothing").map_err(|e| e.status), Err(422));
    assert_eq!(page("orderBy=balance:up").map_err(|e| e.status), Err(422));
    assert_eq!(page("orderBy=").map(|(_, total)| total), Ok(350));
    // Mixed types under one path are the reference's thrown error.
    assert_eq!(
        page("orderBy=attributes.delegate&limit=1").map_err(|e| e.status),
        Err(500)
    );
    assert_eq!(
        addresses(&page("orderBy=address:asc&limit=1").unwrap().0).len(),
        1
    );
}
