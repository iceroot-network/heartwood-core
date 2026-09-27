//! `GET /peers` and `GET /peers/{ip}`: the version filter, the orders and the page over six peers,
//! with the answers of the reference's peers controller for the same peers and queries, and the
//! two documented differences (a range outside the semver subset, the `ip` filter).

use std::net::IpAddr;

use heartwood_api::controllers::peers;
use heartwood_api::resources;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_p2p_wire::messages::peer::{Plugin, Plugins};
use serde_json::{Value, json};

mod snapshot;

fn peer(ip: &str, version: &str, height: Option<u32>, latency: u32) -> PeerInfo {
    PeerInfo {
        ip: ip.parse::<IpAddr>().unwrap(),
        port: 4002,
        ports: Default::default(),
        version: Some(version.to_owned()),
        height,
        latency: Some(latency),
        plugins: Plugins::new(),
    }
}

fn api_plugin(estimate: Option<bool>) -> Plugins {
    let mut plugins = Plugins::new();
    let plugin = Plugin {
        port: Some(4003),
        enabled: Some(true),
        estimate_total_count: estimate,
        ..Plugin::default()
    };
    plugins.insert("@solar-network/api".into(), Some(plugin));
    plugins
}

/// Six peers, in the repository's order.
fn six() -> Vec<PeerInfo> {
    let mut first = peer("198.51.100.1", "4.3.1", Some(265), 40);
    first.ports.insert("@solar-network/api".into(), 4003);
    first.plugins = api_plugin(None);
    let mut third = peer("198.51.100.3", "4.2.0", Some(300), 12);
    third.ports.insert("@solar-network/api".into(), -1);
    third.plugins = api_plugin(Some(false));
    vec![
        first,
        peer("198.51.100.2", "4.3.1-heartwood.0.1.0", Some(264), 12),
        third,
        peer("198.51.100.4", "3.9.9", None, 77),
        peer("198.51.100.5", "4.3.2-next.1", Some(10), 5),
        peer("2001:db8::7", "4.3.10", Some(265), 90),
    ]
}

/// The last group of each listed peer's address (`7` for the IPv6 one), the total, as the
/// reference's controller answered them.
fn listed(query: Value) -> Result<(Vec<String>, u64), u16> {
    let mut full = json!({ "page": 1, "limit": 100 });
    full.as_object_mut()
        .unwrap()
        .extend(query.as_object().unwrap().clone());
    match peers::list(six(), &full) {
        Ok((page, total, _)) => {
            let short = |peer: &PeerInfo| {
                let ip = peer.ip.to_string();
                ip.rsplit(['.', ':']).next().unwrap().to_owned()
            };
            Ok((page.iter().map(short).collect(), total))
        }
        Err(error) => Err(error.status),
    }
}

fn ok(order: &[&str], total: u64) -> Result<(Vec<String>, u64), u16> {
    Ok((order.iter().map(|s| (*s).to_owned()).collect(), total))
}

#[test]
fn the_version_filter_matches_the_reference() {
    let cases = [
        (json!({}), ok(&["5", "2", "3", "1", "4", "7"], 6)),
        (json!({ "version": ">=4.3.0" }), ok(&["1", "7"], 2)),
        (json!({ "version": "4.3.1-heartwood.0.1.0" }), ok(&["2"], 1)),
        (json!({ "version": "^4" }), ok(&["3", "1", "7"], 3)),
        (json!({ "version": "~4.2" }), ok(&["3"], 1)),
        (
            json!({ "version": "<4.3.0 || 4.3.1" }),
            ok(&["3", "1", "4"], 3),
        ),
        // The value is decoded once more, as the reference decodes it.
        (json!({ "version": "%3E%3D4.3.0" }), ok(&["1", "7"], 2)),
        (json!({ "version": ">=4.3.2-next.0" }), ok(&["5", "7"], 2)),
        (json!({ "version": "*" }), ok(&["3", "1", "4", "7"], 4)),
        (json!({ "version": "not a range" }), Err(404)),
        // A malformed escape is the reference's 500 (a URIError).
        (json!({ "version": "%" }), Err(500)),
    ];
    for (query, expected) in cases {
        assert_eq!(listed(query.clone()), expected, "{query}");
    }
}

#[test]
fn a_range_outside_the_semver_subset_is_an_invalid_range() {
    // The reference filters by a hyphen range (it lists 1 and 7 here); the version gate's subset
    // does not read one, so the route answers as for an invalid range.
    let refused = peers::list(six(), &json!({ "version": "4.3.1 - 4.4.0" })).unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (404, "Invalid version range provided")
    );
    let inside_a_word = peers::list(six(), &json!({ "version": ">=4.3.0*" })).unwrap_err();
    assert_eq!(inside_a_word.status, 404);
}

#[test]
fn the_orders_match_the_reference() {
    let cases = [
        ("version:asc", ["4", "3", "2", "1", "5", "7"]),
        ("version:desc", ["7", "5", "1", "2", "3", "4"]),
        ("VERSION:DESC", ["7", "5", "1", "2", "3", "4"]),
        // Unknown heights last in both directions; ties keep the repository's order.
        ("height:asc", ["5", "2", "1", "7", "3", "4"]),
        ("height:desc", ["3", "1", "7", "2", "5", "4"]),
        ("latency:desc", ["7", "4", "1", "2", "3", "5"]),
        ("latency:asc", ["5", "2", "3", "1", "4", "7"]),
        // Another property: by latency, ascending.
        ("ip:desc", ["5", "2", "3", "1", "4", "7"]),
    ];
    for (order, expected) in cases {
        assert_eq!(
            listed(json!({ "orderBy": order })),
            ok(&expected, 6),
            "{order}"
        );
    }
    // A list of orders breaks the reference's controller (a TypeError).
    assert_eq!(
        listed(json!({ "orderBy": ["latency:asc", "height:desc"] })),
        Err(500)
    );
}

#[test]
fn the_page_matches_the_reference() {
    let cases = [
        (json!({ "page": 2, "limit": 4 }), ok(&["4", "7"], 6)),
        (json!({ "offset": 2, "limit": 2 }), ok(&["3", "1"], 6)),
        (
            json!({ "offset": 0, "page": 2, "limit": 4 }),
            ok(&["4", "7"], 6),
        ),
        (json!({ "page": 3, "limit": 4 }), ok(&[], 6)),
    ];
    for (query, expected) in cases {
        assert_eq!(listed(query.clone()), expected, "{query}");
    }
    let (_, _, pagination) = peers::list(six(), &json!({ "page": 2, "limit": 4 })).unwrap();
    assert_eq!((pagination.offset, pagination.limit), (4, 4));
}

#[test]
fn the_ip_filter_keeps_that_address_only() {
    // The reference validates `ip` and ignores it; the route keeps the peers at that address.
    assert_eq!(listed(json!({ "ip": "198.51.100.3" })), ok(&["3"], 1));
    assert_eq!(listed(json!({ "ip": "203.0.113.9" })), ok(&[], 0));
}

#[test]
fn a_peer_is_shown_as_the_reference_shows_it() {
    let expected = [
        r#"{"ip":"198.51.100.1","port":4002,"ports":{"@solar-network/api":4003},"version":"4.3.1","height":265,"latency":40,"plugins":{"@solar-network/api":{"port":4003,"enabled":true}}}"#,
        r#"{"ip":"198.51.100.3","port":4002,"ports":{"@solar-network/api":-1},"version":"4.2.0","height":300,"latency":12,"plugins":{"@solar-network/api":{"port":4003,"enabled":true,"estimateTotalCount":false}}}"#,
        r#"{"ip":"198.51.100.4","port":4002,"ports":{},"version":"3.9.9","latency":77,"plugins":{}}"#,
        r#"{"ip":"2001:db8::7","port":4002,"ports":{},"version":"4.3.10","height":265,"latency":90,"plugins":{}}"#,
    ];
    let shown: Vec<String> = six()
        .iter()
        .map(|peer| serde_json::to_string(&resources::peer(peer)).unwrap())
        .collect();
    for text in expected {
        assert!(shown.iter().any(|shown| shown == text), "{text}");
    }
}

#[tokio::test]
#[ignore = "integration"]
async fn the_peer_routes_over_http() {
    let chain = snapshot::chain();
    let (address, server) = snapshot::serve_chain("peers", chain.pool(Vec::new()), six()).await;
    let list = snapshot::get(address, "/api/peers?version=%3E%3D4.3.0&limit=1").await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body["meta"]["totalCount"], 2);
    assert_eq!(list.body["data"][0]["ip"], "198.51.100.1");
    let one = snapshot::get(address, "/api/peers/198.51.100.3").await;
    assert_eq!(one.body["data"]["version"], "4.2.0");
    for (target, message) in [
        ("/api/peers/203.0.113.1", "Peer not found"),
        (
            "/api/peers?version=4.3.1%20-%204.4.0",
            "Invalid version range provided",
        ),
    ] {
        let answer = snapshot::get(address, target).await;
        assert_eq!(
            (answer.status, &answer.body["message"]),
            (404, &json!(message)),
            "{target}"
        );
    }
    server.stop().await;
}
