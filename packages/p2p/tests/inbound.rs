//! The inbound limits and the plugin chain: the inbound rate limiter's table and the forger's
//! exemption, the caps per /24 and /48, AwaitBlock with a stalled queue, the plugins' order with a
//! stub controller, and class V31's records (the plugin chain's verdicts and the inbound limiter
//! against the oracle).
//!
//! V31 is read from `tests/vectors` of the crate, which must hold it (`make vectors-install`), or
//! from the directory in `P2P_VECTORS` (a relative path is taken from the workspace root), which
//! may lack it: the test then says so and passes.

#![cfg(all(feature = "node", feature = "difftest"))]

mod server_support;

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::Milestones;
use heartwood_p2p::node_services::{AwaitEnd, QueueState};
use heartwood_p2p::server::ServerOptions;
use heartwood_p2p::server::await_block::await_block;
use heartwood_p2p::server::ban_hammer::BanHammer;
use heartwood_p2p::server::caps::InboundCaps;
use heartwood_p2p::server::plugins::Plugins;
use heartwood_p2p::server::rate_limit::InboundLimiter;
use heartwood_p2p::server::seams::{ServerRefusal, SessionId, SocketClosed};
use heartwood_p2p::testing::server::{PeerCall, StubAnswer, inbound_rate_limit, serve_request};
use heartwood_p2p_wire::RouteId;
use serde_json::{Value, json};
use server_support::*;
use tokio::sync::watch;
use tokio::time::{Instant, sleep};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn the_server_options_keep_the_exact_forger_addresses_normalised() {
    let p2p = heartwood_p2p::P2pOptions {
        remote_access: ["127.0.0.1", "::ffff:127.0.0.1", "::1", "10.0.0.0/8"]
            .map(str::to_owned)
            .to_vec(),
        max_transactions_per_request: 7,
        ..heartwood_p2p::P2pOptions::default()
    };
    let options = ServerOptions::from_p2p(&p2p);
    let expected: Vec<IpAddr> = ["127.0.0.1", "127.0.0.1", "::1"]
        .iter()
        .map(|ip| ip.parse().unwrap())
        .collect();
    assert_eq!(options.remote_access, expected);
    assert_eq!((options.port, options.ban_seconds), (4002, 10));
    assert_eq!(options.max_transactions_per_request, 7);
    assert_eq!(options.await_block_bound, Duration::from_secs(8));
}

#[test]
fn the_inbound_table_limits_each_route_per_address() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    for (route, points, window) in [
        (RouteId::PostBlock, 2, 2_000),
        (RouteId::GetBlocks, 2, 1_000),
        (RouteId::GetPeers, 2, 1_000),
        (RouteId::GetUnconfirmedTransactions, 2, 1_000),
        (RouteId::GetStatus, 9, 1_000),
        (RouteId::GetCommonBlocks, 9, 1_000),
        (RouteId::PostTransactions, 25, 1_000),
    ] {
        let limiter = InboundLimiter::new(100, 25);
        for _ in 0..points {
            assert!(limiter.consume(peer(2), route, at(0)), "{route:?}");
        }
        assert!(
            !limiter.consume(peer(2), route, at(window - 1)),
            "{route:?}"
        );
        assert!(limiter.consume(peer(3), route, at(window - 1)), "{route:?}");
        assert!(limiter.consume(peer(2), route, at(window)), "{route:?}");
    }
    // Every route counts toward the global limit, the internal ones only toward it.
    let limiter = InboundLimiter::new(3, 0);
    assert!(limiter.consume(peer(2), RouteId::GetSlotNumber, at(0)));
    assert!(limiter.consume(peer(2), RouteId::GetSlotNumber, at(0)));
    assert!(limiter.consume(peer(2), RouteId::GetStatus, at(0)));
    assert!(!limiter.consume(peer(2), RouteId::GetStatus, at(999)));
    let posts = InboundLimiter::new(100, 0);
    let taken = (0..26).filter(|_| posts.consume(peer(2), RouteId::PostTransactions, at(0)));
    assert_eq!(taken.count(), 25);
}

fn plugins(fakes: &Fakes) -> (Plugins, Arc<BanHammer>) {
    let bans = Arc::new(BanHammer::new(10, vec![FORGER]));
    (fakes.plugins(options(), Arc::clone(&bans)), bans)
}

#[tokio::test(start_paused = true)]
async fn the_forger_is_exempt_from_the_limiter_and_others_are_refused_and_banned() {
    let fakes = Fakes::new();
    let (plugins, bans) = plugins(&fakes);
    let request = status_request(VERSION);
    for _ in 0..30 {
        let answer = plugins.handle(
            Some(RouteId::GetStatus),
            request.clone(),
            FORGER,
            SessionId(1),
            SocketClosed::never(),
        );
        assert_eq!(answer.await.result, Ok(Vec::new()));
    }
    for n in 0..10 {
        let answer = plugins.handle(
            Some(RouteId::GetStatus),
            request.clone(),
            peer(2),
            SessionId(2),
            SocketClosed::never(),
        );
        let answer = answer.await;
        assert_eq!(answer.result.is_ok(), n < 9);
        assert_eq!(answer.close, n == 9);
    }
    assert_eq!(bans.reason(peer(2)).as_deref(), Some("Rate limit exceeded"));
    assert_eq!(bans.reason(FORGER), None);
    // A refusal of the limiter bans through the hammer and is never a violation.
    assert_eq!(fakes.peers.violations(), []);
}

#[test]
fn the_caps_count_addresses_per_24_and_per_48() {
    let caps = InboundCaps::new(2);
    let v4 = |last: u8| IpAddr::from([198, 51, 100, last]);
    assert!(caps.admit(v4(1)) && caps.admit(v4(2)));
    assert!(!caps.admit(v4(3)), "a third address of the /24");
    assert!(
        caps.admit(v4(1)),
        "an address that holds a socket opens another"
    );
    assert!(caps.admit(IpAddr::from([198, 51, 101, 3])), "another /24");
    let mapped: IpAddr = "::ffff:198.51.100.4".parse().unwrap();
    assert!(
        !caps.admit(mapped),
        "an IPv4-mapped address counts in its /24"
    );
    caps.release(v4(2));
    assert!(caps.admit(v4(3)));
    let v6 = |text: &str| text.parse::<IpAddr>().unwrap();
    assert!(caps.admit(v6("2001:db8:1:1::1")) && caps.admit(v6("2001:db8:1:ffff::2")));
    assert!(
        !caps.admit(v6("2001:db8:1::3")),
        "a third address of the /48"
    );
    assert!(caps.admit(v6("2001:db8:2::3")), "another /48");
}

#[tokio::test(start_paused = true)]
async fn the_server_drops_a_connection_over_its_subnet_cap() {
    let bench = Bench::with(ServerOptions {
        max_same_subnet_peers: 1,
        ..options()
    })
    .await;
    let first = bench.raw(peer(2)).await;
    assert!(first.is_some());
    assert!(
        bench.raw(peer(3)).await.is_none(),
        "127.11.0.3 shares the /24"
    );
    assert!(bench.raw(IpAddr::from([127, 12, 0, 3])).await.is_some());
    drop(first);
    sleep(ms(10)).await;
    assert!(bench.raw(peer(3)).await.is_some());
}

#[tokio::test(start_paused = true)]
async fn await_block_waits_for_the_drain_at_most_its_bound() {
    let stalled = QueueState {
        new_block: true,
        running: true,
    };
    let (queue, reader) = watch::channel(stalled);
    let start = Instant::now();
    let never = SocketClosed::never;
    let outcome = await_block(reader.clone(), ms(8_000), never()).await;
    assert_eq!((outcome.end, outcome.waited), (AwaitEnd::Bound, ms(8_000)));
    assert_eq!(Instant::now() - start, ms(8_000));
    let waiting = tokio::spawn(await_block(reader.clone(), ms(8_000), never()));
    sleep(ms(300)).await;
    queue.send_replace(QueueState {
        new_block: true,
        running: false,
    });
    let outcome = waiting.await.unwrap();
    assert_eq!((outcome.end, outcome.waited), (AwaitEnd::Drained, ms(300)));
    assert_eq!(Instant::now() - start, ms(8_300));
    // The queue running outside newBlock, or newBlock with an idle queue: no wait.
    for state in [
        QueueState {
            new_block: false,
            running: true,
        },
        QueueState {
            new_block: true,
            running: false,
        },
    ] {
        queue.send_replace(state);
        let outcome = await_block(reader.clone(), ms(8_000), never()).await;
        assert_eq!((outcome.end, outcome.waited), (AwaitEnd::NoWait, ms(0)));
    }
    assert_eq!(Instant::now() - start, ms(8_300));
}

#[tokio::test(start_paused = true)]
async fn a_stalled_queue_holds_public_routes_for_8_s_and_not_the_internal_ones() {
    let fakes = Fakes::new();
    fakes.node.set_queue(QueueState {
        new_block: true,
        running: true,
    });
    let (plugins, _) = plugins(&fakes);
    let start = Instant::now();
    let answer = plugins.handle(
        Some(RouteId::GetSlotNumber),
        b"{}".to_vec(),
        FORGER,
        SessionId(1),
        SocketClosed::never(),
    );
    assert_eq!(answer.await.result, Ok(Vec::new()));
    assert_eq!(Instant::now() - start, Duration::ZERO);
    let answer = plugins.handle(
        Some(RouteId::GetStatus),
        status_request(VERSION),
        peer(2),
        SessionId(2),
        SocketClosed::never(),
    );
    assert_eq!(answer.await.result, Ok(Vec::new()));
    assert_eq!(Instant::now() - start, ms(8_000));
}

#[tokio::test(start_paused = true)]
async fn the_plugins_run_in_the_reference_order() {
    let refusal = |status: u16, message: &str| Err(ServerRefusal::new(status, message));
    let run = |fakes: Fakes, route, payload: Vec<u8>, ip| async move {
        let (plugins, bans) = plugins(&fakes);
        let answer =
            (plugins.handle(route, payload, ip, SessionId(1), SocketClosed::never())).await;
        (answer.result, bans.reason(ip), fakes)
    };
    // The allowlist comes before the codec.
    let (result, ban, _) = run(
        Fakes::new(),
        Some(RouteId::GetSlotNumber),
        b"{".to_vec(),
        peer(2),
    )
    .await;
    assert_eq!(result, refusal(403, "IP unauthorised on internal route"));
    assert_eq!(ban.as_deref(), Some("IP unauthorised on internal route"));
    // AwaitBlock comes before the payload limit, and the limit before the codec.
    let fakes = Fakes::new();
    fakes.node.set_queue(QueueState {
        new_block: true,
        running: true,
    });
    let start = Instant::now();
    let (result, _, _) = run(fakes, Some(RouteId::GetStatus), vec![0xff; 1025], peer(2)).await;
    assert_eq!(
        result,
        refusal(
            413,
            "Payload content length greater than maximum allowed: 1024"
        )
    );
    assert_eq!(Instant::now() - start, ms(8_000));
    // The codec before the version gate, the gate before the schema.
    let (result, _, fakes) = run(
        Fakes::new(),
        Some(RouteId::GetStatus),
        vec![0x0a, 0x05],
        peer(2),
    )
    .await;
    assert_eq!(
        result,
        refusal(
            400,
            "Payload deserialising failed: RangeError: index out of range"
        )
    );
    assert_eq!(
        fakes.peers.calls(),
        [PeerCall::Violation(
            peer(2),
            heartwood_p2p::scoring::Violation::RequestDecode
        )]
    );
    let old = heartwood_p2p_wire::messages::peer::GetCommonBlocksRequest {
        headers: Some(heartwood_p2p_wire::messages::shared::Headers {
            version: Some("1.0.0".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    use heartwood_p2p_wire::messages::Proto;
    let (result, _, _) = run(
        Fakes::new(),
        Some(RouteId::GetCommonBlocks),
        old.encode(),
        peer(2),
    )
    .await;
    assert_eq!(result, refusal(400, "Validation failed (invalid version)"));
    // A known peer's version is recorded before readiness, which comes before the acceptance and
    // the handler; a 499 bans nobody.
    let fakes = Fakes::new();
    fakes.node.set_booted(false);
    fakes.peers.know(peer(2));
    let (result, ban, fakes) = run(
        fakes,
        Some(RouteId::GetStatus),
        status_request(VERSION),
        peer(2),
    )
    .await;
    assert_eq!((result, ban), (refusal(499, "App is not ready"), None));
    assert_eq!(
        fakes.peers.calls(),
        [PeerCall::Version(peer(2), VERSION.to_owned())]
    );
    assert!(fakes.handlers.requests().is_empty());
    // The acceptance before the handler, at the headers' port or the node's; the handler's
    // refusal below 499 bans with its message.
    let fakes = Fakes::new();
    fakes.handlers.answer(
        RouteId::GetStatus,
        StubAnswer::Refuse(ServerRefusal::new(400, "no")),
    );
    let (result, ban, fakes) = run(
        fakes,
        Some(RouteId::GetStatus),
        status_request(VERSION),
        peer(2),
    )
    .await;
    assert_eq!((result, ban.as_deref()), (refusal(400, "no"), Some("no")));
    assert_eq!(fakes.peers.calls(), [PeerCall::Accept(peer(2), 4002)]);
    assert_eq!(fakes.handlers.requests().len(), 1);
    // A path that names no route: 404, banned as "Not a websocket connection".
    let (result, ban, _) = run(Fakes::new(), None, Vec::new(), peer(2)).await;
    assert_eq!(
        (result, ban.as_deref()),
        (
            refusal(404, "Not Found"),
            Some("Not a websocket connection")
        )
    );
}

/// V31's records without its meta record, or `None` when the directory `P2P_VECTORS` names lacks
/// the class. The crate's own directory must hold it.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("P2P_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join("V31-server.jsonl");
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        // A directory named in the environment may lack the class; the crate's own must hold it.
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!("V31-server"))
    );
    Some(lines.collect())
}

#[tokio::test]
async fn v31_the_plugin_chain_and_the_inbound_limiter_agree_with_the_oracle() {
    let Some(records) = records() else {
        eprintln!("V31-server is not in the directory P2P_VECTORS names");
        return;
    };
    let network = devnet().1;
    let (mut served, mut limited) = (0, 0);
    for record in &records {
        let input = &record["input"];
        let ours = match record["op"].as_str() {
            Some("p2p.serveRequest") => {
                served += 1;
                let text = input["milestones"]
                    .as_str()
                    .expect("the record's milestones");
                let milestones = Milestones::load(text, &network).unwrap();
                let height = u32::try_from(record["height"].as_u64().unwrap()).unwrap();
                serve_request(input, height, milestones, network.clone()).await
            }
            Some("ratelimit.run") if input["limiter"]["inbound"] == json!(true) => {
                limited += 1;
                inbound_rate_limit(input)
            }
            _ => continue,
        };
        let mut expected = record["output"].clone();
        if let Some(output) = expected.as_object_mut() {
            output.shift_remove("notes");
        }
        assert_eq!(ours.as_ref(), Ok(&expected), "{}", record["name"]);
    }
    assert!(
        served > 100 && limited > 10,
        "{served} and {limited} records"
    );
}
