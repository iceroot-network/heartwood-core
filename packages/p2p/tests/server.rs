//! The server against the client's session and connector: the hello, the heartbeats both ways, the
//! server's pings, requests multiplexed on one socket and answered as they finish, 20 MiB
//! messages, and the reasons a socket ends with. The violation record takes the transport faults
//! and never a request refusal.
//!
//! The tests run on tokio's paused clock over in-memory connections.

#![cfg(all(feature = "node", feature = "difftest"))]

mod server_support;

use std::time::Duration;

use heartwood_p2p::connector::Connector;
use heartwood_p2p::scoring::Violation;
use heartwood_p2p::server::seams::{ServerRefusal, SessionId};
use heartwood_p2p::server::session::SessionEnd;
use heartwood_p2p::testing::server::{PeerCall, StubAnswer};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::{Kind, Message};
use server_support::*;
use std::net::SocketAddr;
use tokio::time::{Instant, sleep};

const MIB_20: usize = 20 * 1024 * 1024;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[tokio::test(start_paused = true)]
async fn a_client_session_says_hello_and_its_request_reaches_the_handler() {
    let bench = Bench::new().await;
    let fakes = &bench.fakes;
    fakes
        .handlers
        .answer(RouteId::GetStatus, StubAnswer::Payload(vec![1, 2, 3]));
    let session = bench.session(peer(2)).await;
    assert!(session.is_ready());
    let answer = session.request(RouteId::GetStatus, status_request(VERSION), ms(5_000), 0);
    assert_eq!(answer.await, Ok(vec![1, 2, 3]));
    let requests = fakes.handlers.requests();
    assert_eq!(requests.len(), 1);
    let (route, request) = &requests[0];
    assert_eq!(
        (*route, request.ip, request.from_forger),
        (RouteId::GetStatus, peer(2), false)
    );
    assert_eq!(request.payload, status_request(VERSION));
    // The pre-handler step offered the caller as a peer at its headers' port.
    assert!(
        fakes
            .peers
            .calls()
            .contains(&PeerCall::Accept(peer(2), 4002))
    );
    assert_eq!(fakes.events.events(), [(SessionId(1), peer(2), true)]);
}

#[tokio::test(start_paused = true)]
async fn heartbeats_keep_an_answering_client_and_close_a_silent_one() {
    let bench = Bench::new().await;
    let session = bench.session(peer(2)).await;
    let mut silent = bench.raw(peer(3)).await.unwrap();
    silent.hello().await;
    // The server pings every 15 s from its start; the client answers each ping.
    let start = Instant::now();
    let ping = silent.next().await;
    assert!(matches!(
        ping,
        Sent::Binary(Message {
            kind: Kind::Ping,
            ..
        })
    ));
    assert_eq!(Instant::now() - start, ms(15_000));
    assert!(matches!(
        silent.next().await,
        Sent::Binary(Message {
            kind: Kind::Ping,
            ..
        })
    ));
    // The silent client was active until the check at 20 s, and not until the one at 35 s.
    assert_eq!(silent.next().await, Sent::Close(Vec::new()));
    assert_eq!(Instant::now() - start, ms(35_000));
    sleep(ms(120_000)).await;
    assert!(session.is_ready());
    let answer = session.request(RouteId::GetPeers, status_request(VERSION), ms(5_000), 0);
    assert_eq!(answer.await, Ok(Vec::new()));
}

#[tokio::test(start_paused = true)]
async fn a_request_in_progress_keeps_a_silent_socket_open() {
    let bench = Bench::new().await;
    let long = StubAnswer::After(ms(50_000), vec![9]);
    bench.fakes.handlers.answer(RouteId::GetStatus, long);
    let mut raw = bench.raw(peer(2)).await.unwrap();
    raw.hello().await;
    raw.request(2, RouteId::GetStatus, status_request(VERSION))
        .await;
    for _ in 0..3 {
        assert!(matches!(
            raw.next().await,
            Sent::Binary(Message {
                kind: Kind::Ping,
                ..
            })
        ));
    }
    let answer = raw.next().await;
    assert!(matches!(answer, Sent::Binary(Message { id: 2, ref payload, .. }) if *payload == [9]));
}

#[tokio::test(start_paused = true)]
async fn a_response_still_being_written_keeps_a_silent_socket_open() {
    let bench = Bench::new().await;
    let body = vec![7; 1024 * 1024];
    let handlers = &bench.fakes.handlers;
    handlers.answer(RouteId::GetStatus, StubAnswer::Payload(body.clone()));
    let (client, served) = tokio::io::duplex(64 * 1024);
    bench.server.accept(Box::new(served), peer(2));
    let start = Instant::now();
    let mut raw = Raw::open(client).await.unwrap();
    raw.hello().await;
    raw.request(2, RouteId::GetStatus, status_request(VERSION))
        .await;
    // The client reads 16 KiB a second through a 64 KiB pipe: the response takes until about
    // 60 s to write.
    while raw.buffered() < body.len() {
        assert!(raw.read_some(16 * 1024).await);
        sleep(ms(1_000)).await;
    }
    let answer = raw.next().await;
    assert!(matches!(answer, Sent::Binary(Message { id: 2, ref payload, .. }) if *payload == body));
    // The checks at 35 s and 50 s find the response being written, and the one at 65 s finds it
    // written since the check before, as the reference's `_flush` marks it once written; the one
    // at 80 s closes the silent socket.
    let mut pings = 0;
    loop {
        match raw.next().await {
            Sent::Binary(Message {
                kind: Kind::Ping, ..
            }) => pings += 1,
            Sent::Close(payload) => {
                assert!(payload.is_empty());
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!((pings, Instant::now() - start), (5, ms(80_000)));
}

/// A raw client from `ip` over a pipe of 64 KiB, whose getStatus the server answers with `size`
/// bytes, its hello done.
async fn reader_of_answers(bench: &Bench, ip: std::net::IpAddr, size: usize) -> Raw {
    let body = StubAnswer::Payload(vec![7; size]);
    bench.fakes.handlers.answer(RouteId::GetStatus, body);
    let (client, served) = tokio::io::duplex(64 * 1024);
    bench.server.accept(Box::new(served), ip);
    let mut raw = Raw::open(client).await.unwrap();
    raw.hello().await;
    raw
}

/// Whether the server closed the socket of `ip`.
fn closed(bench: &Bench, ip: std::net::IpAddr) -> bool {
    let events = bench.fakes.events.events();
    events.iter().any(|(_, from, open)| *from == ip && !*open)
}

/// The answers `raw` reads before its stream ends.
async fn answers_until_the_end(raw: &mut Raw) -> usize {
    let mut answers = 0;
    loop {
        match raw.next().await {
            Sent::Binary(Message { id: 2.., .. }) => answers += 1,
            Sent::Ended => return answers,
            _ => {}
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_asks_and_never_reads_is_dropped_once_its_backlog_passes_the_cap() {
    let bench = Bench::new().await;
    let mut raw = reader_of_answers(&bench, peer(2), 4 * 1024 * 1024).await;
    // Five getStatus a second, within the inbound limit, each answered with 4 MiB that the peer
    // never reads: the eleventh answer takes the unwritten bytes past 40 MiB.
    let mut asked = 0;
    while asked < 20 && !closed(&bench, peer(2)) {
        raw.request(asked + 2, RouteId::GetStatus, status_request(VERSION))
            .await;
        asked += 1;
        sleep(ms(200)).await;
    }
    assert_eq!(asked, 11, "the socket is still open");
    assert_eq!(bench.server.ban_reason(peer(2)), None);
    assert!(answers_until_the_end(&mut raw).await < 11);
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_stops_reading_is_dropped_by_the_heartbeat_and_its_writes_end() {
    let bench = Bench::new().await;
    let mut raw = reader_of_answers(&bench, peer(2), 1024 * 1024).await;
    for id in 2..5 {
        raw.request(id, RouteId::GetStatus, status_request(VERSION))
            .await;
    }
    // The check at 20 s finds the first bytes written; the one at 35 s finds that none was
    // written since, while bytes waited, and drops the socket at once, without a close frame.
    sleep(ms(34_000)).await;
    assert!(!closed(&bench, peer(2)));
    sleep(ms(2_000)).await;
    assert!(closed(&bench, peer(2)), "the socket is still open");
    assert_eq!(bench.server.ban_reason(peer(2)), None);
    assert_eq!(answers_until_the_end(&mut raw).await, 0);
}

#[tokio::test(start_paused = true)]
async fn a_pinging_peer_that_never_reads_is_dropped_once_its_writes_stall() {
    let bench = Bench::new().await;
    let mut raw = reader_of_answers(&bench, peer(2), 4 * 1024 * 1024).await;
    // Nine answers of 4 MiB, under the cap, which the peer never reads; a nes ping every 10 s
    // keeps it active by messages, but the check at 35 s finds that no byte moved since 20 s.
    for id in 2..11 {
        raw.request(id, RouteId::GetStatus, status_request(VERSION))
            .await;
        sleep(ms(200)).await;
    }
    for id in 11..14 {
        sleep(ms(10_000)).await;
        raw.send(&Message::ping(id)).await;
    }
    sleep(ms(4_000)).await;
    assert!(closed(&bench, peer(2)), "the socket is still open");
    assert_eq!(bench.server.ban_reason(peer(2)), None);
    assert_eq!(answers_until_the_end(&mut raw).await, 0);
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_closes_with_answers_unread_is_dropped_at_once() {
    let bench = Bench::new().await;
    // By a close frame, or by the end of its stream (a half-close, its reading side open).
    for (ip, half) in [(peer(2), false), (peer(3), true)] {
        let mut raw = reader_of_answers(&bench, ip, 1024 * 1024).await;
        for id in 2..5 {
            raw.request(id, RouteId::GetStatus, status_request(VERSION))
                .await;
        }
        sleep(ms(100)).await;
        match half {
            false => raw.frame(0x88, &1000_u16.to_be_bytes()).await,
            true => raw.half_close().await,
        }
        sleep(ms(100)).await;
        assert!(closed(&bench, ip));
        // Within the close timeout, the peer reads what the pipe holds and no answer.
        assert!(raw.bytes_until_the_end().await <= 64 * 1024);
    }
}

/// A raw client from `ip` over a pipe of 64 KiB, whose one answer of 96 KiB waits in part.
async fn one_answer_waiting(bench: &Bench, ip: std::net::IpAddr) -> Raw {
    let mut raw = reader_of_answers(bench, ip, 96 * 1024).await;
    raw.request(2, RouteId::GetStatus, status_request(VERSION))
        .await;
    sleep(ms(100)).await;
    raw.frame(0x88, &1000_u16.to_be_bytes()).await;
    sleep(ms(100)).await;
    assert!(closed(bench, ip));
    raw
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_closes_with_little_unread_still_gets_it_and_the_close_echo() {
    let bench = Bench::new().await;
    let mut raw = one_answer_waiting(&bench, peer(2)).await;
    let answer = raw.next().await;
    assert!(
        matches!(answer, Sent::Binary(Message { id: 2, ref payload, .. }) if payload.len() == 96 * 1024)
    );
    assert_eq!(
        raw.next().await,
        Sent::Close(1000_u16.to_be_bytes().to_vec())
    );
    assert_eq!(raw.next().await, Sent::Ended);
}

#[tokio::test(start_paused = true)]
async fn a_new_socket_from_the_address_ends_the_writes_of_its_closed_one() {
    let bench = Bench::new().await;
    let mut old = one_answer_waiting(&bench, peer(2)).await;
    // As the reference destroys every other socket of the address, the closing one included.
    let mut new = bench.raw(peer(2)).await.unwrap();
    new.hello().await;
    sleep(ms(100)).await;
    assert_eq!(old.next().await, Sent::Ended);
    // Another address's closed socket keeps its writes.
    let mut other = one_answer_waiting(&bench, peer(3)).await;
    let _ = bench.raw(peer(4)).await.unwrap();
    sleep(ms(100)).await;
    assert!(matches!(
        other.next().await,
        Sent::Binary(Message { id: 2, .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn close_and_reconnect_cycles_from_one_address_hold_less_than_two_backlogs() {
    let bench = Bench::new().await;
    let mut ended = Vec::new();
    for cycle in 0..4 {
        let mut raw = reader_of_answers(&bench, peer(2), 4 * 1024 * 1024).await;
        // Nine answers of 4 MiB, under the cap, which the peer never reads; then it closes, by a
        // close frame or a half-close, and connects again.
        for id in 2..11 {
            raw.request(id, RouteId::GetStatus, status_request(VERSION))
                .await;
            sleep(ms(200)).await;
        }
        match cycle % 2 {
            0 => raw.frame(0x88, &1000_u16.to_be_bytes()).await,
            _ => raw.half_close().await,
        }
        sleep(ms(100)).await;
        ended.push(raw);
    }
    // Everything the closed sockets still get within the close timeout: the relay holds it.
    let mut held = 0;
    for raw in &mut ended {
        held += raw.bytes_until_the_end().await;
    }
    let max_backlog = heartwood_p2p::server::listener::MAX_BACKLOG as usize;
    assert!(held < 2 * max_backlog, "{held} bytes held");
    assert!(held <= 4 * 64 * 1024, "{held} bytes held");
}

#[tokio::test(start_paused = true)]
async fn a_dropped_socket_leaves_no_task_behind() {
    let bench = Bench::new().await;
    sleep(ms(100)).await;
    let metrics = tokio::runtime::Handle::current().metrics();
    let before = metrics.num_alive_tasks();
    for n in 2..6 {
        let mut raw = bench.raw(peer(n)).await.unwrap();
        raw.hello().await;
        // A text message ends the socket at once, with a reason.
        raw.frame(0x81, b"text").await;
        assert_eq!(raw.next().await, Sent::Ended);
    }
    sleep(ms(100)).await;
    assert_eq!(metrics.num_alive_tasks(), before);
}

#[tokio::test(start_paused = true)]
async fn requests_on_one_socket_are_answered_as_they_finish() {
    let bench = Bench::new().await;
    let handlers = &bench.fakes.handlers;
    handlers.answer(RouteId::GetStatus, StubAnswer::After(ms(10_000), vec![1]));
    handlers.answer(RouteId::GetPeers, StubAnswer::Payload(vec![2]));
    let session = bench.session(peer(2)).await;
    let start = Instant::now();
    let slow = {
        let session = session.clone();
        tokio::spawn(async move {
            let answer =
                session.request(RouteId::GetStatus, status_request(VERSION), ms(20_000), 0);
            (answer.await, Instant::now())
        })
    };
    sleep(ms(1)).await;
    let quick = session.request(RouteId::GetPeers, status_request(VERSION), ms(20_000), 0);
    assert_eq!(quick.await, Ok(vec![2]));
    assert!(Instant::now() - start < ms(10));
    let (slow, at) = slow.await.unwrap();
    assert_eq!((slow, at - start), (Ok(vec![1]), ms(10_000)));
}

#[tokio::test(start_paused = true)]
async fn the_connector_shares_one_session_for_concurrent_requests() {
    let bench = Bench::new().await;
    let handlers = &bench.fakes.handlers;
    handlers.answer(RouteId::GetStatus, StubAnswer::After(ms(100), vec![4]));
    let connector = Connector::with_dial(bench.dial(peer(5)));
    let address = SocketAddr::new(peer(1), 4002);
    let emit = || {
        let connector = connector.clone();
        tokio::spawn(async move {
            let payload = status_request(VERSION);
            connector
                .emit(address, RouteId::GetStatus, payload, ms(5_000), 0)
                .await
        })
    };
    assert_eq!(emit().await.unwrap(), Ok(vec![4]));
    let pending: Vec<_> = (0..5).map(|_| emit()).collect();
    for answer in pending {
        assert_eq!(answer.await.unwrap(), Ok(vec![4]));
    }
    let opens = bench
        .fakes
        .events
        .events()
        .into_iter()
        .filter(|(_, _, open)| *open);
    assert_eq!(opens.count(), 1);
}

#[tokio::test(start_paused = true)]
async fn messages_of_20_mib_pass_and_larger_ones_end_the_socket() {
    let bench = Bench::new().await;
    // A forger's session takes a response of 20 MiB.
    let body = vec![7; MIB_20 - 14];
    let handlers = &bench.fakes.handlers;
    handlers.answer(RouteId::GetSlotNumber, StubAnswer::Payload(body.clone()));
    let forger = bench.session(FORGER).await;
    let answer = forger.request(RouteId::GetSlotNumber, b"{}".to_vec(), ms(5_000), MIB_20);
    assert_eq!(answer.await.map(|payload| payload.len()), Ok(body.len()));
    // A message of 20 MiB is read; one byte more ends the socket before its payload is read.
    let fakes = Fakes::new();
    let path = "p2p.peer.unknown";
    let exact = Message::request(2, RouteId::GetPeers, vec![0; MIB_20 - 14 - path.len()]);
    let exact = Message {
        path: path.to_owned(),
        ..exact
    };
    assert_eq!(exact.encode().unwrap().len(), MIB_20);
    let hello = binary(&Message::client_hello(1));
    let end = session_end(&fakes, peer(2), &[hello.clone(), binary(&exact)]).await;
    assert_eq!(end, terminated("Not Found"));
    let mut over = vec![0x82, 0x80 | 127];
    over.extend_from_slice(&(MIB_20 as u64 + 1).to_be_bytes());
    let end = session_end(&fakes, peer(3), &[hello, over]).await;
    assert_eq!(end, terminated("Max payload size exceeded"));
    assert_eq!(fakes.peers.violations(), [(peer(3), Violation::Protocol)]);
}

fn terminated(reason: &str) -> SessionEnd {
    SessionEnd::Terminated {
        reason: reason.to_owned(),
    }
}

#[tokio::test(start_paused = true)]
async fn sockets_end_with_the_reference_reasons() {
    let hello = binary(&Message::client_hello(1));
    let request = |id, path: &str| {
        let message = Message::request(id, RouteId::GetPeers, status_request(VERSION));
        binary(&Message {
            path: path.to_owned(),
            ..message
        })
    };
    let error = binary(&Message {
        kind: Kind::Error,
        ..Message::ping(3)
    });
    let with_version = |version| {
        binary(&Message {
            version,
            ..Message::client_hello(1)
        })
    };
    let subscribing = |payload: &[u8]| {
        binary(&Message {
            payload: payload.to_vec(),
            ..Message::client_hello(1)
        })
    };
    // The writer puts 1 for an id 0, so the id's bytes are cleared by hand.
    let mut ping = Message::ping(1).encode().unwrap();
    ping[2..6].fill(0);
    let id_zero = frame(0x82, &ping);
    let cases: Vec<(Vec<Vec<u8>>, &str)> = vec![
        (vec![frame(0x81, b"hi")], "Invalid message received"),
        (vec![frame(0x89, b"")], "Malicious ping frame received"),
        (vec![frame(0x8a, b"")], "Malicious pong frame received"),
        (vec![frame(0x82, b"short")], "Invalid payload received"),
        (
            vec![vec![0x82, 0x00]],
            "Invalid websocket frame - MASK must be set",
        ),
        (
            vec![vec![0xc2, 0x80, 0, 0, 0, 0]],
            "Invalid websocket frame - RSV1 must be clear",
        ),
        (
            vec![vec![0xa2, 0x80, 0, 0, 0, 0]],
            "Invalid websocket frame - RSV2 and RSV3 must be clear",
        ),
        (
            vec![frame(0x80, b"x")],
            "Invalid websocket frame - invalid opcode 0",
        ),
        (
            vec![frame(0x83, b"x")],
            "Invalid websocket frame - invalid opcode 3",
        ),
        (
            vec![frame(0x09, b"")],
            "Invalid websocket frame - FIN must be set",
        ),
        (
            vec![vec![0x89, 0x80 | 126, 0, 200]],
            "Invalid websocket frame - invalid payload length 126",
        ),
        (
            vec![frame(0x88, &[3])],
            "Invalid websocket frame - invalid payload length 1",
        ),
        (
            vec![frame(0x88, &[3, 236])],
            "Invalid websocket frame - invalid status code 1004",
        ),
        (
            vec![frame(0x02, b"a"), frame(0x82, b"b")],
            "Invalid websocket frame - invalid opcode 2",
        ),
        (
            vec![with_version(3)],
            "Incorrect protocol version (expected 2 but received 3)",
        ),
        (
            vec![hello.clone(), hello.clone()],
            "Connection already initialised",
        ),
        (
            vec![subscribing(b"[\"/a\"]")],
            "Invalid subscription payload",
        ),
        (vec![subscribing(b"{")], "Invalid subscription payload"),
        (
            vec![request(2, "p2p.peer.getPeers")],
            "Connection is not initialised",
        ),
        (vec![id_zero], "Message missing id"),
        (vec![hello.clone(), error], "Unknown message type"),
        (
            vec![binary(&Message::ping(1)), binary(&Message::ping(2))],
            "Exceeded ping limit",
        ),
        (vec![hello.clone(), request(2, "")], "Message missing path"),
        (
            vec![hello.clone(), request(2, "p2p.peer.nothing")],
            "Not Found",
        ),
        (
            vec![hello.clone(), request(2, "P2P.peer.getPeers")],
            "Not Found",
        ),
    ];
    for (frames, reason) in cases {
        let end = session_end(&Fakes::new(), peer(2), &frames).await;
        assert_eq!(end, terminated(reason), "{reason}");
    }
    // A long frame's 64-bit length beyond 2^53 - 1.
    let mut long = vec![0x82, 0x80 | 127];
    long.extend_from_slice(&(1u64 << 53).to_be_bytes());
    let end = session_end(&Fakes::new(), peer(2), &[long]).await;
    assert_eq!(
        end,
        terminated("Unsupported websocket frame - payload length > 2^53 - 1")
    );
    // A close frame, an empty subscription list and a text message that is not UTF-8 end the
    // socket without a reason.
    for frames in [
        vec![hello.clone(), frame(0x88, &[3, 232])],
        vec![subscribing(b" [ ] "), frame(0x88, b"")],
        vec![frame(0x81, &[0xff])],
    ] {
        assert_eq!(
            session_end(&Fakes::new(), peer(2), &frames).await,
            SessionEnd::Closed
        );
    }
}

#[tokio::test(start_paused = true)]
async fn the_violation_record_takes_transport_faults_and_never_a_refusal() {
    let hello = binary(&Message::client_hello(1));
    let on = |id, route, payload: Vec<u8>| binary(&Message::request(id, route, payload));
    let faults = [
        vec![frame(0x81, b"hi")],
        vec![frame(0x89, b"")],
        vec![frame(0x8a, b"")],
        vec![frame(0x82, b"short")],
        vec![vec![0x82, 0x00]],
        vec![binary(&Message {
            version: 1,
            ..Message::client_hello(1)
        })],
        vec![hello.clone(), hello.clone()],
        vec![frame(0x81, &[0xff])],
    ];
    for frames in faults {
        let fakes = Fakes::new();
        session_end(&fakes, peer(2), &frames).await;
        assert_eq!(
            fakes.peers.violations(),
            [(peer(2), Violation::Protocol)],
            "{frames:?}"
        );
    }
    // A body the route's codec cannot decode.
    let fakes = Fakes::new();
    let garbage = on(2, RouteId::GetStatus, vec![0x0a, 0x05]);
    session_end(&fakes, peer(2), &[hello.clone(), garbage]).await;
    assert_eq!(
        fakes.peers.violations(),
        [(peer(2), Violation::RequestDecode)]
    );
    // The refusals an honest client can cause, and every fault from the forger's address.
    let refused = ServerRefusal::new(400, "a handler's refusal");
    let refusals = [
        on(2, RouteId::GetSlotNumber, b"{}".to_vec()),
        on(2, RouteId::GetStatus, status_request("4.2.0")),
        on(2, RouteId::GetStatus, status_request("")),
        on(2, RouteId::GetStatus, vec![0x10, 0x01]),
        on(2, RouteId::PostTransactions, status_request(VERSION)),
    ];
    for request in refusals {
        let fakes = Fakes::new();
        fakes.handlers.answer(
            RouteId::PostTransactions,
            StubAnswer::Refuse(refused.clone()),
        );
        let end = session_end(&fakes, peer(2), &[hello.clone(), request]).await;
        assert!(matches!(end, SessionEnd::Terminated { .. }));
        assert_eq!(fakes.peers.violations(), []);
    }
    let fakes = Fakes::new();
    fakes.node.set_booted(false);
    session_end(
        &fakes,
        peer(2),
        &[
            hello.clone(),
            on(2, RouteId::GetStatus, status_request(VERSION)),
        ],
    )
    .await;
    let fakes2 = Fakes::new();
    session_end(&fakes2, FORGER, &[frame(0x89, b"")]).await;
    session_end(
        &fakes2,
        FORGER,
        &[hello, on(2, RouteId::GetStatus, vec![0x0a, 0x05])],
    )
    .await;
    assert_eq!(
        (fakes.peers.violations(), fakes2.peers.violations()),
        (vec![], vec![])
    );
}
