//! The history routes' queries and parameters, as the reference's schemas check them: every
//! route alone and with each of its criteria, the sort keys of each, and the parameters. They run
//! on the query text as the server reads it, without a store.

use heartwood_api::RouteId;
use heartwood_api::schemas::{parse_query, validate_params, validate_query};
use serde_json::{Map, Value, json};

fn query(route: RouteId, raw: &str) -> Result<Value, heartwood_api::ApiError> {
    validate_query(route, &parse_query(raw))
}

fn status(route: RouteId, raw: &str) -> u16 {
    query(route, raw).map_or_else(|error| error.status, |_| 200)
}

fn params(id: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("id".to_owned(), json!(id));
    map
}

/// A criterion and a value it takes, for each key of a block.
const BLOCK_VALUES: [(&str, &str); 15] = [
    (
        "id",
        "a3bd6a9d1c0d6f4f9c2b8e0a57f26a1d3c6b3d0d9e3a1c4b5d6e7f8091a2b3c4",
    ),
    ("version", "0"),
    ("timestamp", "1000"),
    (
        "previousBlock",
        "b3bd6a9d1c0d6f4f9c2b8e0a57f26a1d3c6b3d0d9e3a1c4b5d6e7f8091a2b3c4",
    ),
    ("height", "7"),
    ("numberOfTransactions", "3"),
    ("totalAmount", "0"),
    ("totalFee", "5"),
    ("burnedFee", "1"),
    ("reward", "0"),
    ("payloadLength", "96"),
    ("payloadHash", "abc123"),
    (
        "generatorPublicKey",
        "02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
    ("username", "genesis_1"),
    ("blockSignature", "aa"),
];

#[test]
fn a_block_list_alone_has_the_defaults() {
    let got = query(RouteId::BlocksIndex, "").unwrap();
    assert_eq!(got["orderBy"], json!("height:desc"));
    assert_eq!(got["transform"], json!(true));
    assert_eq!(got["page"], json!(1));
    assert_eq!(got["limit"], json!(100));
}

#[test]
fn every_block_criterion_is_accepted_alone() {
    for (name, value) in BLOCK_VALUES {
        let raw = format!("{name}={value}");
        let value = query(RouteId::BlocksIndex, &raw);
        assert!(value.is_ok() || name == "blockSignature", "{name}");
    }
    // A signature has 128 hexadecimal characters.
    assert_eq!(status(RouteId::BlocksIndex, "blockSignature=aa"), 422);
    let signature = "a".repeat(128);
    assert_eq!(
        status(RouteId::BlocksIndex, &format!("blockSignature={signature}")),
        200
    );
}

#[test]
fn the_block_criteria_take_ranges_and_lists() {
    let got = query(RouteId::BlocksIndex, "height.from=2&height.to=9").unwrap();
    assert_eq!(got["height"], json!({"from": 2, "to": 9}));
    let got = query(RouteId::BlocksIndex, "height=1,3").unwrap();
    assert_eq!(got["height"], json!([1, 3]));
    assert_eq!(status(RouteId::BlocksIndex, "height=0"), 422);
    assert_eq!(status(RouteId::BlocksIndex, "height.from=0"), 422);
    assert_eq!(status(RouteId::BlocksIndex, "timestamp=-1"), 422);
    assert_eq!(status(RouteId::BlocksIndex, "username=_x"), 422);
    assert_eq!(status(RouteId::BlocksIndex, "username=12345"), 422);
    assert_eq!(status(RouteId::BlocksIndex, "username=a"), 200);
    assert_eq!(status(RouteId::BlocksIndex, "unknown=1"), 422);
}

#[test]
fn the_block_sort_keys_are_the_criteria() {
    for (name, _) in BLOCK_VALUES {
        for direction in ["asc", "desc"] {
            let raw = format!("orderBy={name}:{direction}");
            assert_eq!(status(RouteId::BlocksIndex, &raw), 200, "{raw}");
        }
    }
    assert_eq!(status(RouteId::BlocksIndex, "orderBy=donations:asc"), 422);
    assert_eq!(status(RouteId::BlocksIndex, "orderBy=height:up"), 422);
    let got = query(RouteId::BlocksIndex, "orderBy=timestamp:asc,id:desc").unwrap();
    assert_eq!(got["orderBy"], json!(["timestamp:asc", "id:desc"]));
}

#[test]
fn the_missed_blocks_take_three_keys_and_no_transform() {
    let got = query(RouteId::BlocksMissed, "").unwrap();
    assert_eq!(got["orderBy"], json!("timestamp:desc"));
    assert!(got.get("transform").is_none());
    for raw in ["timestamp=5", "height=5", "username=genesis_1"] {
        assert_eq!(status(RouteId::BlocksMissed, raw), 200, "{raw}");
    }
    for name in ["timestamp", "height", "username"] {
        let raw = format!("orderBy={name}:asc");
        assert_eq!(status(RouteId::BlocksMissed, &raw), 200, "{raw}");
    }
    // Any other key of a block is unknown here, as a criterion and as a sort key.
    for (name, value) in BLOCK_VALUES {
        if ["timestamp", "height", "username"].contains(&name) {
            continue;
        }
        assert_eq!(
            status(RouteId::BlocksMissed, &format!("{name}={value}")),
            422,
            "{name}"
        );
        assert_eq!(
            status(RouteId::BlocksMissed, &format!("orderBy={name}:asc")),
            422,
            "{name}"
        );
    }
    assert_eq!(status(RouteId::BlocksMissed, "transform=false"), 422);
}

#[test]
fn the_first_block_takes_only_transform() {
    assert_eq!(
        query(RouteId::BlocksFirst, "").unwrap(),
        json!({"transform": true})
    );
    assert_eq!(
        query(RouteId::BlocksFirst, "transform=false").unwrap(),
        json!({"transform": false})
    );
    assert_eq!(status(RouteId::BlocksFirst, "height=1"), 422);
    assert_eq!(status(RouteId::BlocksFirst, "page=1"), 422);
}

#[test]
fn a_delegates_blocks_have_the_thirteen_criteria() {
    let got = query(RouteId::DelegatesBlocks, "").unwrap();
    assert_eq!(got["orderBy"], json!("height:desc"));
    for (name, value) in BLOCK_VALUES {
        let raw = format!("{name}={value}");
        let expected = if ["username", "generatorPublicKey", "blockSignature"].contains(&name) {
            422
        } else {
            200
        };
        assert_eq!(status(RouteId::DelegatesBlocks, &raw), expected, "{name}");
    }
    assert_eq!(
        status(
            RouteId::DelegatesBlocks,
            &format!("blockSignature={}", "a".repeat(128))
        ),
        200
    );
    assert_eq!(
        status(RouteId::DelegatesBlocks, "orderBy=username:asc"),
        422
    );
    assert_eq!(status(RouteId::DelegatesBlocks, "orderBy=reward:asc"), 200);
    assert_eq!(status(RouteId::DelegatesBlocks, "transform=false"), 200);
}

#[test]
fn a_delegates_missed_blocks_sort_by_the_three_keys_and_take_the_block_keys() {
    let got = query(RouteId::DelegatesMissedBlocks, "").unwrap();
    assert_eq!(got["orderBy"], json!("timestamp:desc"));
    assert!(got.get("transform").is_none());
    assert_eq!(
        status(RouteId::DelegatesMissedBlocks, "height=4&reward=0"),
        200
    );
    for name in ["timestamp", "height", "username"] {
        let raw = format!("orderBy={name}:desc");
        assert_eq!(status(RouteId::DelegatesMissedBlocks, &raw), 200, "{raw}");
    }
    assert_eq!(
        status(RouteId::DelegatesMissedBlocks, "orderBy=reward:asc"),
        422
    );
    assert_eq!(status(RouteId::DelegatesMissedBlocks, "username=x"), 422);
}

#[test]
fn a_round_takes_any_query() {
    assert_eq!(
        query(RouteId::RoundsDelegates, "x=1").unwrap(),
        json!({"x": "1"})
    );
}

#[test]
fn the_votes_take_the_transaction_criteria() {
    let got = query(RouteId::VotesIndex, "").unwrap();
    assert_eq!(got["orderBy"], json!(["timestamp:desc", "sequence:desc"]));
    for raw in [
        "senderPublicKey=02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "blockHeight=3",
        "fee.from=1",
        "typeGroup=2&type=2",
        "asset=x",
    ] {
        let expected = if raw.starts_with("asset") { 422 } else { 200 };
        assert_eq!(status(RouteId::VotesIndex, raw), expected, "{raw}");
    }
    assert_eq!(status(RouteId::VotesIndex, "nonsense=1"), 422);
    assert_eq!(status(RouteId::VotesIndex, "limit=101"), 422);
}

#[test]
fn a_vote_takes_only_transform() {
    assert_eq!(
        query(RouteId::VotesShow, "").unwrap(),
        json!({"transform": true})
    );
    assert_eq!(
        query(RouteId::VotesShow, "transform=false").unwrap(),
        json!({"transform": false})
    );
    assert_eq!(status(RouteId::VotesShow, "limit=1"), 422);
}

#[test]
fn a_wallets_history_takes_the_transaction_criteria() {
    for route in [
        RouteId::WalletsTransactionsSent,
        RouteId::WalletsTransactionsReceived,
        RouteId::WalletsVotes,
    ] {
        let got = query(route, "").unwrap();
        assert_eq!(got["orderBy"], json!(["timestamp:desc", "sequence:desc"]));
        let address = "D".to_owned() + &"a".repeat(33);
        assert_eq!(status(route, &format!("senderId={address}")), 200);
        assert_eq!(status(route, &format!("recipientId={address}")), 200);
        assert_eq!(status(route, "senderId=genesis_1"), 422);
        assert_eq!(status(route, "typeGroup=1&type=3"), 200);
        assert_eq!(status(route, "transform=false"), 200);
        assert_eq!(status(route, "orderBy=amount:asc"), 200);
        assert_eq!(status(route, "bogus=1"), 422);
    }
}

#[test]
fn the_parameters_are_checked_by_route() {
    let check = |route, id: &str| validate_params(route, &params(id)).is_ok();
    assert!(check(RouteId::RoundsDelegates, "12"));
    assert!(!check(RouteId::RoundsDelegates, "0"));
    assert!(!check(RouteId::RoundsDelegates, "-1"));
    assert!(!check(RouteId::RoundsDelegates, "1.5"));
    assert!(check(RouteId::VotesShow, &"ab".repeat(32)));
    assert!(!check(RouteId::VotesShow, &"ab".repeat(31)));
    assert!(!check(RouteId::VotesShow, &"zz".repeat(32)));
    for route in [
        RouteId::WalletsTransactionsSent,
        RouteId::WalletsTransactionsReceived,
        RouteId::WalletsVotes,
        RouteId::DelegatesBlocks,
        RouteId::DelegatesMissedBlocks,
    ] {
        assert!(check(route, "genesis_1"), "{route:?}");
        assert!(check(route, &format!("D{}", "a".repeat(33))), "{route:?}");
        assert!(check(route, &format!("02{}", "a".repeat(64))), "{route:?}");
        assert!(!check(route, "Bad"), "{route:?}");
        assert!(!check(route, ""), "{route:?}");
    }
}
