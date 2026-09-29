//! The relay's API read: the address rule, the request budget, the answers of the API (rate
//! limit included), and the parsers against the bodies the API writes.

mod doctor_support;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use heartwood_cli::HttpError;
use heartwood_cli::doctor::seams::{ChainFacts, RelayFacts};
use heartwood_cli::doctor::sources::api::{
    ApiContext, MAX_REQUESTS, PEERS_PAGE, PEERS_PAGES, api_address, fetch, parse_data, read_block,
    read_configuration, read_last_block, read_peers, read_status, read_syncing, read_wallet,
};
use heartwood_cli::doctor::view::{Budget, Reason, Source};
use heartwood_cli::http::Response;
use heartwood_kernel::clock::ManualClock;
use heartwood_kernel::log::Level;

use doctor_support::{FakeApi, NOW_MS};

const STATUS: &str = include_str!("fixtures/doctor/api/status.json");
const SYNCING: &str = include_str!("fixtures/doctor/api/syncing.json");
const CONFIGURATION: &str = include_str!("fixtures/doctor/api/configuration.json");
const PEERS: &str = include_str!("fixtures/doctor/api/peers-page-1.json");
const BLOCK: &str = include_str!("fixtures/doctor/api/block.json");
const WALLET: &str = include_str!("fixtures/doctor/api/wallet.json");

const KEY: &str = "02aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";

fn relay(api: Option<SocketAddr>) -> RelayFacts {
    RelayFacts {
        p2p: SocketAddr::from(([192, 0, 2, 1], 4001)),
        api,
        remote_access: Vec::new(),
        minimum_network_reach: 20,
        file_log_level: Level::Info,
        warnings: Vec::new(),
        env_names: Vec::new(),
        chain: ChainFacts {
            nethash: "7d1c0b3a5e9f2468a0c1e3d5b7f92468ace02468bdf13579acef13579bdf2468".to_owned(),
            epoch_ms: 1_788_220_800_000,
            block_time_s: 8,
            active_delegates: 53,
        },
        store_file: PathBuf::from("/nodes/node1/data/blockchain.db"),
    }
}

fn ok(body: &str) -> Result<Response, HttpError> {
    Ok(Response {
        status: 200,
        body: body.as_bytes().to_vec(),
    })
}

fn status(code: u16) -> Result<Response, HttpError> {
    Ok(Response {
        status: code,
        body: Vec::new(),
    })
}

/// A run: the fake API, the clock and the budget.
struct Run {
    api: FakeApi,
    clock: ManualClock,
    budget: Budget,
    address: SocketAddr,
}

impl Run {
    fn new() -> Run {
        Run {
            api: FakeApi::default(),
            clock: ManualClock::new(NOW_MS),
            budget: Budget::new(NOW_MS, Duration::from_secs(60)),
            address: SocketAddr::from(([127, 0, 0, 1], 4003)),
        }
    }

    fn answer(&mut self, path: &str, answer: Result<Response, HttpError>) {
        self.api.answers.insert(path.to_owned(), answer);
    }

    fn context(&self) -> ApiContext<'_> {
        ApiContext {
            api: &self.api,
            address: self.address,
            budget: &self.budget,
            clock: &self.clock,
        }
    }

    fn requests(&self) -> usize {
        self.api.requests.borrow().len()
    }
}

fn reason<T: std::fmt::Debug>(result: Result<T, heartwood_cli::doctor::view::Unknown>) -> Reason {
    let error = result.expect_err("an unknown");
    assert_eq!(error.source, Source::Api);
    error.reason
}

#[test]
fn a_429_is_unknown_as_rate_limited_and_is_never_retried() {
    let mut run = Run::new();
    run.answer("/api/node/status", status(429));
    assert_eq!(reason(read_status(&run.context())), Reason::RateLimited);
    assert_eq!(run.requests(), 1);
    assert_eq!(run.budget.requests_taken(), 1);
    // The error that carries the status gives the same answer.
    let mut run = Run::new();
    run.answer(
        "/api/node/status",
        Err(HttpError::Status {
            address: run.address,
            path: "/api/node/status".to_owned(),
            status: 429,
        }),
    );
    assert_eq!(reason(read_status(&run.context())), Reason::RateLimited);
    assert_eq!(run.requests(), 1);
}

#[test]
fn every_read_takes_one_request_and_none_is_repeated_after_a_rate_limit() {
    let mut run = Run::new();
    run.answer("/api/peers?page=1&limit=100", status(429));
    assert_eq!(reason(read_peers(&run.context())), Reason::RateLimited);
    assert_eq!(run.requests(), 1);
    for path in ["/api/blocks/last", "/api/node/syncing"] {
        run.answer(path, status(429));
    }
    assert_eq!(reason(read_last_block(&run.context())), Reason::RateLimited);
    assert_eq!(reason(read_syncing(&run.context())), Reason::RateLimited);
    assert_eq!(run.requests(), 3);
}

#[test]
fn a_run_sends_at_most_twelve_requests() {
    let mut run = Run::new();
    run.answer("/api/node/status", ok(STATUS));
    for _ in 0..MAX_REQUESTS {
        assert!(read_status(&run.context()).is_ok());
    }
    assert_eq!(run.requests(), MAX_REQUESTS as usize);
    for _ in 0..5 {
        assert_eq!(reason(read_status(&run.context())), Reason::ApiBudget);
        assert_eq!(
            reason(fetch(&run.context(), "/api/blocks/last")),
            Reason::ApiBudget
        );
    }
    // The refused ones were never sent.
    assert_eq!(run.requests(), 12);
    assert_eq!(run.budget.requests_taken(), 12);
}

#[test]
fn a_run_past_its_time_sends_no_request() {
    let run = Run::new();
    run.clock.set(NOW_MS + 61_000);
    assert_eq!(reason(read_status(&run.context())), Reason::RunBudget);
    assert_eq!(run.requests(), 0);
}

#[test]
fn peers_are_read_in_at_most_two_pages_of_a_hundred() {
    let mut run = Run::new();
    let page = |count: usize, total: usize| {
        let entries: Vec<String> = (0..count)
            .map(|i| {
                format!(
                    r#"{{"ip":"192.0.2.{}","port":4001,"version":"4.3.1","height":{}}}"#,
                    1 + i % 250,
                    1000 + i
                )
            })
            .collect();
        format!(
            r#"{{"meta":{{"totalCount":{total}}},"data":[{}]}}"#,
            entries.join(",")
        )
    };
    run.answer("/api/peers?page=1&limit=100", ok(&page(100, 450)));
    run.answer("/api/peers?page=2&limit=100", ok(&page(100, 450)));
    run.answer("/api/peers?page=3&limit=100", ok(&page(100, 450)));
    let peers = read_peers(&run.context()).expect("peers");
    assert_eq!(PEERS_PAGE, 100);
    assert_eq!(PEERS_PAGES, 2);
    assert_eq!(peers.peers.len(), 200);
    assert_eq!(peers.total_count, 450);
    let requests = run.api.requests.borrow();
    let paths: Vec<&str> = requests.iter().map(|(_, path)| path.as_str()).collect();
    assert_eq!(
        paths,
        ["/api/peers?page=1&limit=100", "/api/peers?page=2&limit=100"]
    );
}

#[test]
fn a_short_first_page_is_the_only_page() {
    let mut run = Run::new();
    run.answer("/api/peers?page=1&limit=100", ok(PEERS));
    let peers = read_peers(&run.context()).expect("peers");
    assert_eq!(run.requests(), 1);
    assert_eq!(peers.total_count, 3);
    // Two of the five entries have no usable address or port and are left out.
    let ips: Vec<IpAddr> = peers.peers.iter().map(|peer| peer.ip).collect();
    assert_eq!(
        ips,
        [
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 24)),
            IpAddr::V6("2001:db8::7".parse::<Ipv6Addr>().expect("an address")),
        ]
    );
    assert_eq!(peers.peers[0].height, Some(1301));
    assert_eq!(peers.peers[0].version.as_deref(), Some("4.3.1"));
    // A version is the peer's own text and is kept as it came, control characters included.
    assert_eq!(peers.peers[2].version.as_deref(), Some("4.3.1\nforged"));
}

#[test]
fn the_api_address_is_loopback_as_configured_or_the_loopback_of_a_wildcard() {
    let bound = |ip: IpAddr, port| relay(Some(SocketAddr::new(ip, port)));
    // A devnet node in mode B listens on its own 127.P.(10+k) address: kept as it is.
    let mode_b = IpAddr::V4(Ipv4Addr::new(127, 240, 3, 11));
    assert_eq!(
        api_address(&bound(mode_b, 4003)).expect("an address"),
        SocketAddr::new(mode_b, 4003)
    );
    let other = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    assert_eq!(
        api_address(&bound(other, 4103)).expect("an address"),
        SocketAddr::new(other, 4103)
    );
    let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
    assert_eq!(
        api_address(&bound(v6, 4003)).expect("an address"),
        SocketAddr::new(v6, 4003)
    );
    // A wildcard bind is reached on the loopback of its family, on the same port.
    let any4 = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    assert_eq!(
        api_address(&bound(any4, 4003)).expect("an address"),
        SocketAddr::new(other, 4003)
    );
    let any6 = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    assert_eq!(
        api_address(&bound(any6, 4005)).expect("an address"),
        SocketAddr::new(v6, 4005)
    );
}

#[test]
fn an_api_that_is_off_or_bound_to_an_address_that_is_not_loopback_is_not_read() {
    let off = api_address(&relay(None)).expect_err("off");
    assert_eq!((off.source, off.reason), (Source::Api, Reason::ApiOff));
    for address in [
        SocketAddr::from(([192, 0, 2, 5], 4003)),
        SocketAddr::from(([203, 0, 113, 9], 4003)),
        SocketAddr::new("2001:db8::1".parse().expect("an address"), 4003),
    ] {
        let refused = api_address(&relay(Some(address))).expect_err("not loopback");
        assert_eq!(refused.reason, Reason::ApiNotLoopback, "{address}");
    }
}

#[test]
fn the_fake_refuses_what_the_real_read_refuses_and_the_unknown_says_so() {
    let mut run = Run::new();
    run.address = SocketAddr::from(([192, 0, 2, 5], 4003));
    assert_eq!(reason(read_status(&run.context())), Reason::ApiNotLoopback);
}

#[test]
fn failures_of_the_request_are_told_by_their_kind() {
    let address = SocketAddr::from(([127, 0, 0, 1], 4003));
    let path = "/api/node/status".to_owned();
    let cases: Vec<(Result<Response, HttpError>, Reason)> = vec![
        (
            Err(HttpError::Timeout {
                address,
                path: path.clone(),
            }),
            Reason::TimedOut,
        ),
        (
            Err(HttpError::Connection {
                address,
                path: path.clone(),
                message: "refused".to_owned(),
            }),
            Reason::NotRunning,
        ),
        (
            Err(HttpError::Response {
                address,
                path: path.clone(),
                reason: "too long".to_owned(),
            }),
            Reason::Unparsed,
        ),
        (
            Err(HttpError::Status {
                address,
                path: path.clone(),
                status: 500,
            }),
            Reason::Unreadable,
        ),
        (status(500), Reason::Unreadable),
        (status(503), Reason::Unreadable),
        (status(404), Reason::Unreadable),
        (ok("not json"), Reason::Unparsed),
        (ok("{}"), Reason::Unparsed),
        (ok(r#"{"data":[]}"#), Reason::Unparsed),
        (ok(r#"{"data":{"synced":"yes"}}"#), Reason::Unparsed),
    ];
    for (answer, expected) in cases {
        let mut run = Run::new();
        run.answer(&path, answer.clone());
        assert_eq!(reason(read_status(&run.context())), expected, "{answer:?}");
        assert_eq!(run.requests(), 1);
    }
}

#[test]
fn the_bodies_the_api_writes_are_read_into_typed_answers() {
    let mut run = Run::new();
    run.answer("/api/node/status", ok(STATUS));
    run.answer("/api/node/syncing", ok(SYNCING));
    run.answer("/api/node/configuration", ok(CONFIGURATION));
    run.answer("/api/blocks/last", ok(BLOCK));
    run.answer("/api/blocks/1301", ok(BLOCK));
    run.answer(&format!("/api/wallets/{KEY}"), ok(WALLET));
    let context = run.context();
    let status = read_status(&context).expect("status");
    assert_eq!(
        (status.synced, status.now, status.blocks_count),
        (true, 1301, 0)
    );
    let syncing = read_syncing(&context).expect("syncing");
    assert_eq!(
        (syncing.syncing, syncing.blocks, syncing.height),
        (false, 0, 1301)
    );
    let configuration = read_configuration(&context).expect("configuration");
    // The node's version is `core.version`, not the network's address byte.
    assert_eq!(configuration.core_version.as_deref(), Some("4.3.1"));
    assert_eq!(configuration.block_time_s, Some(8));
    assert_eq!(configuration.active_delegates, Some(53));
    assert_eq!(configuration.nethash.as_deref().map(str::len), Some(64));
    let last = read_last_block(&context).expect("last block");
    assert_eq!((last.height, last.timestamp), (1301, Some(770_000)));
    assert_eq!(last.username.as_deref(), Some("alice"));
    assert_eq!(last.generator_public_key.as_deref(), Some(KEY));
    assert_eq!(read_block(&context, 1301).expect("block"), Some(last));
    let wallet = read_wallet(&context, KEY)
        .expect("wallet")
        .expect("a wallet");
    assert_eq!(wallet.username.as_deref(), Some("alice"));
    assert_eq!(wallet.public_key.as_deref(), Some(KEY));
}

#[test]
fn a_block_or_a_wallet_the_relay_does_not_have_is_none() {
    let run = Run::new();
    assert_eq!(read_block(&run.context(), 99_999).expect("read"), None);
    assert_eq!(read_wallet(&run.context(), KEY).expect("read"), None);
    assert_eq!(run.requests(), 2);
}

#[test]
fn a_body_that_lacks_what_a_read_needs_is_unparsed_and_never_a_panic() {
    let mut run = Run::new();
    for (path, body) in [
        ("/api/blocks/last", r#"{"data":{"id":"x"}}"#),
        ("/api/blocks/last", r#"{"data":{"id":7,"height":"9"}}"#),
        ("/api/node/syncing", r#"{"data":null}"#),
        ("/api/node/configuration", r#"{"data":[1,2]}"#),
    ] {
        run.answer(path, ok(body));
        let result = match path {
            "/api/node/syncing" => read_syncing(&run.context()).map(|_| ()),
            "/api/node/configuration" => read_configuration(&run.context()).map(|_| ()),
            _ => read_last_block(&run.context()).map(|_| ()),
        };
        assert_eq!(reason(result), Reason::Unparsed, "{path} {body}");
    }
    assert!(parse_data(b"").is_err());
    assert!(parse_data(b"\xff\xfe").is_err());
    assert!(parse_data(br#"{"data":{"a":1}}"#).is_ok());
}
