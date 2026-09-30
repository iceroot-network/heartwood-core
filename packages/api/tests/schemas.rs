//! The query grammar, the routes' validation and the list envelope, against facts of the reference
//! implementation's server (its answers on the committed devnet chain, and its query plugins and
//! pagination run on their own). The class V28 of the harness compares the grammar and the
//! pagination with the reference record by record.

use std::net::{IpAddr, Ipv4Addr};

use heartwood_api::pagination::{Pagination, envelope};
use heartwood_api::schemas::{
    PARSED, parse_query, validate_params, validate_payload, validate_query,
};
use heartwood_api::{ApiRequest, RouteId};
use serde_json::{Map, Value, json};

fn params(name: &str, value: &str) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert(name.to_owned(), Value::from(value));
    params
}

fn status(result: Result<Value, heartwood_api::ApiError>) -> u16 {
    result.map_or_else(|error| error.status, |_| 200)
}

fn query(route: RouteId, raw: &str) -> Result<Value, heartwood_api::ApiError> {
    validate_query(route, &parse_query(raw))
}

#[test]
fn queries_parse_as_the_reference_server_reads_them() {
    let cases = [
        ("", json!({})),
        ("a=1,2&b.c=3", json!({"a": ["1", "2"], "b": {"c": "3"}})),
        ("b=1&2=x&1=y", json!({"1": "y", "2": "x", "b": "1"})),
        ("a=1&a=2,3", json!({"a": ["1", "2,3"]})),
        ("a.b=2&a=1", json!({"a": "1"})),
        ("a=1&a.b=2", json!({"a": {"b": "2"}})),
        ("a=1,2&a.0=x", json!({"a": ["x", "2"]})),
        (
            "x=%FF&y=%E2%82%AC&z=%zz+1",
            json!({"x": "\u{fffd}", "y": "€", "z": "%zz 1"}),
        ),
        ("a%2Eb=1&c=%2C", json!({"a": {"b": "1"}, "c": ["", ""]})),
        ("__proto__=1&constructor.x=2&k", json!({"k": ""})),
        ("&&=x&", json!({"": "x"})),
        // A key set before another key's snapshot value changes that value, as in JavaScript.
        ("a.0.x=5&a=1,2", json!({"a": [{"x": "5"}, "2"]})),
        ("b.0=x&b=1,2", json!({"b": ["x", "2"]})),
        ("x.y=1&x=2&x.z=3", json!({"x": {"z": "3"}})),
        // A list's holes are `null`; a name that is not an index is a property JSON leaves out.
        (
            "a.b=1,2&a.b.3=x&a.b.1.c=y",
            json!({"a": {"b": ["1", {"c": "y"}, null, "x"]}}),
        ),
        ("a=1,2&a.4294967295=x&a.01=y", json!({"a": ["1", "2"]})),
        ("a=1,2&a.1.length=3", json!({"a": ["1", {"length": "3"}]})),
        // Node's loop checks the key's characters after its first escape as the value's: a
        // second escape in the key has the value decoded, where it may fail.
        (
            "%C3%A9=FALSE%zz\u{e9}",
            json!({"\u{e9}": "FALSE%zz\u{fffd}"}),
        ),
        ("a%41=FALSE%zz\u{e9}", json!({"aA": "FALSE%zz\u{e9}"})),
        ("%C3%A9=%u0041\u{1f600}", json!({"\u{e9}": "%u0041=\u{0}"})),
        ("a+%41%42=%zz\u{e9}", json!({"a AB": "%zz\u{fffd}"})),
        ("%41%4=%zz\u{e9}", json!({"A%4": "%zz\u{e9}"})),
        ("%41x%42%=%zz\u{e9}", json!({"AxB%": "%zz\u{fffd}"})),
        ("%41+%42=%zz\u{e9}", json!({"A B": "%zz\u{fffd}"})),
    ];
    for (raw, expected) in cases {
        assert_eq!(parse_query(raw), expected, "{raw}");
        let keys = |v: &Value| v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>());
        assert_eq!(
            keys(&parse_query(raw)),
            keys(&expected),
            "key order of {raw}"
        );
    }
}

#[test]
fn queries_on_which_the_reference_plugins_throw_are_500_everywhere() {
    let throws = [
        "a=1&a=,",
        "limit=1,2&limit.length=1",
        // Below a list's index far past its end, and below a list held by a list's named property.
        "a=1,2&a.5000=3,4&a.5000.length=1",
        "a=1,2&a.b=3,4&a.b.length=1",
    ];
    for raw in throws {
        assert_eq!(parse_query(raw), Value::Null, "{raw}");
        for route in RouteId::ALL {
            let answer = validate_query(route, &parse_query(raw));
            assert_eq!(status(answer), 500, "{route:?} {raw}");
        }
    }
}

#[test]
fn only_a_thousand_pairs_are_read() {
    let raw: Vec<String> = (0..1001).map(|i| format!("k{i}=1")).collect();
    let parsed = parse_query(&raw.join("&"));
    assert_eq!(parsed.as_object().map(Map::len), Some(1000));
    assert!(parsed.get("k1000").is_none());
}

/// Runs `work` on a thread with the stack of the API's workers (tokio's default, 2 MiB).
fn on_a_worker_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let thread = std::thread::Builder::new().stack_size(2 << 20);
    thread.spawn(work).unwrap().join().unwrap()
}

/// The values in `value` and its depth, counted without recursion.
fn shape(value: &Value) -> (usize, usize) {
    let (mut values, mut depth) = (0, 0);
    let mut stack = vec![(value, 1)];
    while let Some((value, level)) = stack.pop() {
        values += 1;
        depth = depth.max(level);
        match value {
            Value::Array(items) => stack.extend(items.iter().map(|x| (x, level + 1))),
            Value::Object(map) => stack.extend(map.values().map(|x| (x, level + 1))),
            _ => {}
        }
    }
    (values, depth)
}

#[test]
fn hostile_queries_are_read_with_bounded_work() {
    on_a_worker_stack(|| {
        use RouteId::{NodeStatus, TransactionsIndex};
        let dots = |key: &str| format!("{key}{}=1", ".a".repeat(32_000));
        let gaps: String = (1..=998).map(|i| format!("&a.{}=1", i * 1000)).collect();
        let many: String = (0..499).map(|i| format!("&k{i}=1,2&k{i}.1001=x")).collect();
        let long = format!("a={}", ",".repeat(16_000));
        let deep_throw = format!("{0}=1,2&{0}.length=1", format!("a{}", ".b".repeat(10_000)));
        // (query, its status on a list that refuses unknown keys, on a route without a schema)
        let cases = [
            (dots("a"), 422, 200),
            (dots("asset"), 500, 200),
            (format!("a=1,2{gaps}"), 422, 200),
            (format!("address=1,2{gaps}"), 422, 200),
            ("asset.x=1,2&asset.x.4294967294=3".to_owned(), 500, 200),
            (many.clone(), 422, 200),
            (long.clone(), 422, 200),
            (deep_throw, 500, 500),
        ];
        for (raw, listed, unlisted) in cases {
            let parsed = parse_query(&raw);
            let (values, depth) = shape(&parsed);
            let label = raw.get(..40).unwrap_or(&raw);
            assert!(depth <= 130, "{label}: {depth} levels");
            assert!(values <= raw.len() + 16_384 + 2, "{label}: {values} values");
            assert_eq!(status(query(TransactionsIndex, &raw)), listed, "{label}");
            let served = query(NodeStatus, &raw);
            assert_eq!(status(served.clone()), unlisted, "{label}");
            if let Ok(served) = served {
                assert!(served.get(".beyond").is_none(), "{label}");
            }
        }
        // Holes are written out, as the reference's JSON has them, up to 16,384 in all.
        let parsed = parse_query("a=1,2&a.1000=x");
        assert_eq!(parsed["a"].as_array().map(Vec::len), Some(1001));
        assert_eq!(parsed["a"][999], Value::Null);
        let parsed = parse_query(&format!("a=1,2{gaps}"));
        assert_eq!(parsed["a"].as_array().map(Vec::len), Some(2 + 2 * 998));
        // 499 lists of 1,002 items: 16 whole, then each run of holes as one, and the 483 names.
        let (values, _) = shape(&parse_query(&many));
        assert_eq!(values, 1 + 499 + 16 * 1002 + 483 * 4 + 1 + 483);
        // A comma list is written out whole, and 128 levels are written out whole.
        assert_eq!(
            parse_query(&long)["a"].as_array().map(Vec::len),
            Some(16_001)
        );
        let nested = format!("asset{}=1", ".a".repeat(127));
        assert_eq!(status(query(TransactionsIndex, &nested)), 200);
    });
}

#[test]
fn transaction_lists_get_defaults_after_the_given_keys() {
    let validated = query(RouteId::TransactionsIndex, "limit=05&amount.to=7").unwrap();
    let keys: Vec<&String> = validated.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["limit", "amount", "orderBy", "transform", "page", PARSED]
    );
    assert_eq!(validated["limit"], json!(5));
    assert_eq!(validated["amount"], json!({"to": 7}));
    assert_eq!(
        validated["orderBy"],
        json!(["timestamp:desc", "sequence:desc"])
    );
    assert_eq!(validated["transform"], json!(true));
    assert_eq!(
        validated[PARSED],
        json!({"limit": "05", "amount": {"to": "7"}})
    );
}

#[test]
fn the_reference_accepts_and_refuses_these_queries() {
    use RouteId::*;
    let cases = [
        (BlocksLast, "transform=TRUE", 200),
        (BlocksLast, "transform=1", 422),
        (BlocksLast, "transform=", 422),
        (BlocksLast, "page=1", 422),
        (
            TransactionsIndex,
            "orderBy=timestamp:asc&orderBy=sequence:asc",
            200,
        ),
        (TransactionsIndex, "orderBy=Timestamp:desc", 422),
        (TransactionsIndex, "orderBy=timestamp:DESC", 422),
        (TransactionsIndex, "orderBy=timestamp", 422),
        (TransactionsIndex, "sequence=0", 422),
        (TransactionsIndex, "amount=9007199254740993", 422),
        (TransactionsIndex, "amount.from=1&amount.to=", 422),
        (TransactionsIndex, "amount.foo=1", 422),
        (TransactionsIndex, "limit=1e1", 200),
        (TransactionsIndex, "limit= 5 ", 200),
        (TransactionsIndex, "memo=a,b", 200),
        (TransactionsIndex, "asset=x", 422),
        (TransactionsIndex, "__proto__=1&limit=1", 200),
        (PeersIndex, "orderBy=Amount:ASC", 200),
        (PeersIndex, "transform=false", 422),
        (PeersIndex, "ip=198.51.100.0/24", 200),
        (PeersIndex, "ip=notanip", 422),
        (PeersIndex, "version=4.3.1,4.3.0", 422),
        (TransactionsUnconfirmed, "transform=0", 422),
        (NodeStatus, "foo=bar&limit=0", 200),
        (WalletsShow, "foo=bar", 200),
    ];
    for (route, raw, expected) in cases {
        assert_eq!(status(query(route, raw)), expected, "{route:?} {raw}");
    }
}

#[test]
fn path_parameters_follow_each_route() {
    use RouteId::*;
    let cases = [
        (BlocksShow, "id", "1e2", true),
        (BlocksShow, "id", "007", true),
        (BlocksShow, "id", " 7", true),
        (BlocksShow, "id", "0", false),
        (BlocksShow, "id", "1.5", false),
        (BlocksShow, "id", "abc", false),
        (BlocksShow, "id", &"A".repeat(64), true),
        (BlocksTransactions, "id", "a", true),
        (TransactionsShow, "id", "xyz", false),
        (TransactionsShowUnconfirmed, "id", &"0".repeat(64), true),
        (PeersShow, "ip", "::1", true),
        (PeersShow, "ip", "notanip", false),
        (WalletsShow, "id", "UPPER", false),
        (WalletsShow, "id", "a.b", true),
        (WalletsShow, "id", &"a".repeat(35), false),
        (WalletsTransactions, "id", "genesis_1", true),
    ];
    for (route, name, value, accepted) in cases {
        let result = validate_params(route, &params(name, value));
        assert_eq!(result.is_ok(), accepted, "{route:?} {value:?}");
        if let Err(error) = result {
            assert_eq!(error.status, 422);
        }
    }
}

#[test]
fn submissions_are_one_to_forty_objects() {
    let route = RouteId::TransactionsStore;
    let refused = [
        json!({}),
        json!({"transactions": []}),
        json!({"transactions": [1]}),
        json!({"transactions": [{}], "x": 1}),
        json!({"transactions": vec![json!({}); 41]}),
        json!("hello"),
        Value::Null,
    ];
    for payload in refused {
        assert_eq!(status(validate_payload(route, &payload)), 422, "{payload}");
    }
    let accepted = json!({"transactions": vec![json!({}); 40]});
    assert_eq!(validate_payload(route, &accepted).unwrap(), accepted);
}

fn meta(route: RouteId, path: &str, raw: &str, total: u64) -> Value {
    let validated = query(route, raw).unwrap();
    let pagination = Pagination::of(&validated);
    let count = pagination
        .limit
        .min(total.saturating_sub(pagination.offset));
    let request = ApiRequest {
        route,
        params: Map::new(),
        path: path.to_owned(),
        query: validated,
        payload: None,
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    let data = vec![Value::Null; usize::try_from(count).unwrap()];
    envelope(&request, pagination, total, false, data)["meta"].clone()
}

#[test]
fn links_keep_the_given_spelling_and_order_last() {
    let meta = meta(
        RouteId::TransactionsIndex,
        "/api/transactions",
        "limit=1",
        149,
    );
    assert_eq!(
        meta,
        json!({
            "totalCountIsEstimate": false, "count": 1, "pageCount": 149, "totalCount": 149,
            "next": "/transactions?limit=1&transform=true&page=2", "previous": null,
            "self": "/transactions?limit=1&transform=true&page=1",
            "first": "/transactions?limit=1&transform=true&page=1",
            "last": "/transactions?limit=1&transform=true&page=149",
        })
    );
    let meta = self::meta(
        RouteId::TransactionsIndex,
        "/api/transactions/",
        "limit=05&page=02&transform=FALSE&orderBy=amount:asc",
        149,
    );
    assert_eq!(
        meta["next"],
        "/transactions?limit=05&page=3&transform=FALSE&orderBy=amount:asc"
    );
    let meta = self::meta(RouteId::TransactionsIndex, "/api/transactions", "", 0);
    assert_eq!(
        (&meta["pageCount"], &meta["next"], &meta["last"]),
        (
            &json!(1),
            &Value::Null,
            &json!("/transactions?transform=true&page=1&limit=100")
        )
    );
}

#[test]
fn asset_criteria_are_linked_as_the_reference_links_them() {
    let route = RouteId::TransactionsIndex;
    let path = "/api/transactions";
    let meta = meta(route, path, "asset.x=a,b&asset.x.5=c&limit=1", 3);
    assert_eq!(
        meta["next"],
        "/transactions?asset.x=a,b,,,,c&limit=1&transform=true&page=2"
    );
    let meta = self::meta(route, path, "asset.x=a,b&asset.x.3.y=c", 3);
    assert_eq!(
        meta["self"],
        "/transactions?asset.x=a,b,,[object Object]&transform=true&page=1&limit=100"
    );
    let meta = self::meta(route, path, "asset=a,b&asset.0.x=1&asset.1.y=2", 3);
    assert_eq!(
        meta["self"],
        "/transactions?asset=[object Object],[object Object]&transform=true&page=1&limit=100"
    );
    let key = format!("asset{}", ".a".repeat(127));
    let meta = self::meta(route, path, &format!("{key}=1"), 3);
    let expected = format!("/transactions?{key}=1&transform=true&page=1&limit=100");
    assert_eq!(meta["self"], Value::String(expected));
}

#[test]
fn offset_zero_is_ignored_and_another_offset_replaces_the_page() {
    let page = |raw: &str| Pagination::of(&query(RouteId::TransactionsIndex, raw).unwrap());
    assert_eq!(
        page("offset=0&page=3&limit=10"),
        Pagination {
            offset: 20,
            limit: 10
        }
    );
    assert_eq!(
        page("offset=5&page=3&limit=10"),
        Pagination {
            offset: 5,
            limit: 10
        }
    );
    assert_eq!(
        page(""),
        Pagination {
            offset: 0,
            limit: 100
        }
    );
    let meta = meta(
        RouteId::PeersIndex,
        "/api/peers",
        "offset=0&page=2&limit=1",
        3,
    );
    assert_eq!(meta["self"], "/peers?offset=0&page=2&limit=1");
    assert_eq!(meta["previous"], "/peers?offset=0&page=1&limit=1");
}

/// The keys of a block route's validated query in Joi's order: the given keys, then the defaults
/// in the schema's order, the default order first.
#[test]
fn a_block_route_adds_its_default_order_before_the_other_defaults() {
    let keys = |route: RouteId, raw: &str| -> Vec<String> {
        let validated = query(route, raw).unwrap();
        let keys = validated.as_object().unwrap().keys();
        keys.filter(|key| *key != PARSED).cloned().collect()
    };
    assert_eq!(
        keys(RouteId::BlocksIndex, ""),
        ["orderBy", "transform", "page", "limit"]
    );
    assert_eq!(
        keys(RouteId::BlocksIndex, "username=genesis_1&height.from=2"),
        [
            "username",
            "height",
            "orderBy",
            "transform",
            "page",
            "limit"
        ]
    );
    assert_eq!(
        keys(RouteId::BlocksMissed, ""),
        ["orderBy", "page", "limit"]
    );
    assert_eq!(
        keys(RouteId::DelegatesBlocks, "transform=false&totalFee.from=0"),
        ["transform", "totalFee", "orderBy", "page", "limit"]
    );
    assert_eq!(
        keys(RouteId::DelegatesMissedBlocks, "totalAmount=1"),
        ["totalAmount", "orderBy", "page", "limit"]
    );
    // A given order keeps its place.
    assert_eq!(
        keys(RouteId::BlocksIndex, "limit=3&orderBy=height:asc"),
        ["limit", "orderBy", "transform", "page"]
    );
}
