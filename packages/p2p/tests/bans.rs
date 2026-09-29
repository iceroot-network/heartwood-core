//! The ban hammer's rules with a paused clock: one socket per address, each ban reason, the
//! socket-close bans from 60 s after the first sync, the lifting of a ban after `banSeconds`, and
//! the exemption of the forger's addresses; then the same rules through the server.

#![cfg(all(feature = "node", feature = "difftest"))]

mod server_support;

use std::time::Duration;

use heartwood_p2p::server::ServerOptions;
use heartwood_p2p::server::ban_hammer::{BanHammer, SYNC_GRACE_MS, format_seconds};
use heartwood_p2p::server::seams::{ServerRefusal, SessionId};
use heartwood_p2p::server::session::SessionEnd;
use heartwood_p2p::testing::server::StubAnswer;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::Message;
use server_support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::sleep;

const SYNCED: i64 = START_MS - SYNC_GRACE_MS;

fn terminated(reason: &str) -> SessionEnd {
    SessionEnd::Terminated {
        reason: reason.to_owned(),
    }
}

#[test]
fn a_new_socket_names_the_others_of_its_address() {
    let bans = BanHammer::new(10, vec![FORGER]);
    assert_eq!(bans.opened(peer(2), SessionId(1)), []);
    assert_eq!(bans.opened(peer(3), SessionId(2)), []);
    assert_eq!(bans.opened(peer(2), SessionId(3)), [SessionId(1)]);
    assert_eq!(
        bans.opened(peer(2), SessionId(4)),
        [SessionId(1), SessionId(3)]
    );
    bans.closed(peer(2), SessionId(1), &SessionEnd::Closed, START_MS, None);
    assert_eq!(
        bans.opened(peer(2), SessionId(5)),
        [SessionId(3), SessionId(4)]
    );
}

#[test]
fn a_ban_lasts_its_seconds_and_keeps_its_first_reason() {
    let bans = BanHammer::new(10, vec![FORGER]);
    bans.ban(peer(2), "Rate limit exceeded", START_MS);
    assert!(bans.is_banned(peer(2), START_MS + 9_999));
    // A renewal restarts the time and keeps the first reason.
    bans.ban(peer(2), "Validation failed", START_MS + 5_000);
    assert!(bans.is_banned(peer(2), START_MS + 14_999));
    assert_eq!(bans.reason(peer(2)).as_deref(), Some("Rate limit exceeded"));
    // The ban is lifted at the next connection after its time.
    assert!(!bans.is_banned(peer(2), START_MS + 15_000));
    assert_eq!(bans.reason(peer(2)), None);
    assert!(!bans.is_banned(peer(3), START_MS));
}

#[test]
fn the_forger_and_a_ban_time_of_0_ban_nobody() {
    let bans = BanHammer::new(10, vec![FORGER]);
    bans.ban(FORGER, "Rate limit exceeded", START_MS);
    let end = terminated("Malicious ping frame received");
    bans.opened(FORGER, SessionId(1));
    bans.closed(FORGER, SessionId(1), &end, START_MS, Some(SYNCED));
    assert!(!bans.is_banned(FORGER, START_MS));
    let off = BanHammer::new(0, Vec::new());
    off.ban(peer(2), "Rate limit exceeded", START_MS);
    off.opened(peer(2), SessionId(1));
    off.closed(peer(2), SessionId(1), &end, START_MS, Some(SYNCED));
    assert!(!off.is_banned(peer(2), START_MS));
}

#[test]
fn a_socket_closes_into_a_ban_by_its_reason_or_after_the_sync_by_what_it_did() {
    let close = |end: &SessionEnd, established: bool, synced: Option<i64>| {
        let bans = BanHammer::new(10, vec![FORGER]);
        bans.opened(peer(2), SessionId(1));
        if established {
            bans.established(peer(2), SessionId(1));
        }
        bans.closed(peer(2), SessionId(1), end, START_MS, synced);
        bans.reason(peer(2))
    };
    let reason = terminated("Invalid payload received");
    for synced in [None, Some(START_MS), Some(SYNCED)] {
        let ban = close(&reason, true, synced);
        assert_eq!(ban.as_deref(), Some("Invalid payload received"));
    }
    let (closed, handshake) = (SessionEnd::Closed, SessionEnd::Handshake);
    // Until 60 s after the first sync, a close alone bans nobody.
    for synced in [None, Some(SYNCED + 1)] {
        assert_eq!(close(&closed, false, synced), None);
        assert_eq!(close(&handshake, false, synced), None);
    }
    let ban = close(&closed, false, Some(SYNCED));
    assert_eq!(ban.as_deref(), Some("Did not make an endpoint request"));
    assert_eq!(close(&closed, true, Some(SYNCED)), None);
    let ban = close(&handshake, false, Some(SYNCED));
    assert_eq!(
        ban.as_deref(),
        Some("Failed to complete websocket handshake")
    );
    // An address already banned keeps its ban and its reason.
    let bans = BanHammer::new(10, vec![FORGER]);
    bans.ban(peer(2), "Rate limit exceeded", START_MS);
    bans.opened(peer(2), SessionId(1));
    bans.closed(peer(2), SessionId(1), &reason, START_MS + 3_000, None);
    assert!(!bans.is_banned(peer(2), START_MS + 10_000));
}

#[test]
fn ban_times_read_as_the_reference_writes_them() {
    let cases = [
        (1, "1 second"),
        (10, "10 seconds"),
        (60, "1 minute"),
        (61, "1 minute and 1 second"),
        (3_600, "1 hour"),
        (3_662, "1 hour, 1 minute and 2 seconds"),
        (86_400, "1 day"),
        (90_061, "1 day, 1 hour, 1 minute and 1 second"),
        (172_800, "2 days"),
    ];
    for (seconds, text) in cases {
        assert_eq!(format_seconds(seconds), text);
    }
}

#[tokio::test(start_paused = true)]
async fn a_refusal_below_499_bans_the_caller_until_its_time_passes() {
    let bench = Bench::new().await;
    let mut raw = bench.raw(peer(2)).await.unwrap();
    raw.hello().await;
    raw.request(2, RouteId::GetSlotNumber, b"{}".to_vec()).await;
    assert_eq!(raw.next().await, Sent::Ended);
    let reason = bench.server.ban_reason(peer(2));
    assert_eq!(reason.as_deref(), Some("IP unauthorised on internal route"));
    assert!(
        bench.raw(peer(2)).await.is_none(),
        "a banned address's connection is dropped"
    );
    bench.fakes.clock.advance(10_000);
    assert!(bench.raw(peer(2)).await.is_some());
    assert_eq!(bench.server.ban_reason(peer(2)), None);
}

#[tokio::test(start_paused = true)]
async fn a_499_keeps_the_socket_and_bans_nobody() {
    let bench = Bench::new().await;
    bench.fakes.node.set_booted(false);
    let session = bench.session(peer(2)).await;
    let refused = session.request(
        RouteId::GetStatus,
        status_request(VERSION),
        Duration::from_secs(5),
        0,
    );
    let refused = refused.await.unwrap_err().to_string();
    assert!(
        refused.contains(r#"{"error":"Unknown","message":"null"}"#),
        "{refused}"
    );
    let forger = bench.session(FORGER).await;
    let refused = forger.request(
        RouteId::GetSlotNumber,
        b"{}".to_vec(),
        Duration::from_secs(5),
        0,
    );
    let refused = refused.await.unwrap_err().to_string();
    assert!(
        refused.contains(r#"{"error":"Unknown","message":"App is not ready"}"#),
        "{refused}"
    );
    bench.fakes.node.set_booted(true);
    let answer = session.request(
        RouteId::GetStatus,
        status_request(VERSION),
        Duration::from_secs(5),
        0,
    );
    assert_eq!(answer.await, Ok(Vec::new()));
    assert_eq!(bench.server.ban_reason(peer(2)), None);
}

#[tokio::test(start_paused = true)]
async fn a_500_ends_the_socket_and_bans_with_the_reference_text_when_it_closes() {
    let bench = Bench::new().await;
    let broken = ServerRefusal::new(500, "the store failed");
    bench
        .fakes
        .handlers
        .answer(RouteId::GetStatus, StubAnswer::Refuse(broken));
    let mut raw = bench.raw(peer(2)).await.unwrap();
    raw.hello().await;
    raw.request(2, RouteId::GetStatus, status_request(VERSION))
        .await;
    assert_eq!(raw.next().await, Sent::Ended);
    sleep(Duration::from_millis(1)).await;
    let reason = bench.server.ban_reason(peer(2));
    assert_eq!(reason.as_deref(), Some("An internal server error occurred"));
}

#[tokio::test(start_paused = true)]
async fn a_new_socket_from_an_address_destroys_the_older_one() {
    let bench = Bench::new().await;
    let mut older = bench.raw(peer(2)).await.unwrap();
    older.hello().await;
    let mut newer = bench.raw(peer(2)).await.unwrap();
    assert_eq!(older.next().await, Sent::Ended);
    newer.hello().await;
    sleep(Duration::from_millis(1)).await;
    let events = bench.fakes.events.events();
    let closed: Vec<_> = events.iter().filter(|(_, _, open)| !open).collect();
    assert_eq!(closed, [&(SessionId(1), peer(2), false)]);
    // Before the sync plus 60 s the older socket's close bans nobody.
    assert_eq!(bench.server.ban_reason(peer(2)), None);
}

#[tokio::test(start_paused = true)]
async fn after_the_sync_a_socket_that_made_no_request_or_no_handshake_is_banned() {
    let bench = Bench::new().await;
    bench.fakes.node.set_first_synced(Some(SYNCED));
    let mut idle = bench.raw(peer(2)).await.unwrap();
    idle.hello().await;
    drop(idle);
    let mut busy = bench.raw(peer(3)).await.unwrap();
    busy.hello().await;
    busy.request(2, RouteId::GetStatus, status_request(VERSION))
        .await;
    assert!(matches!(
        busy.next().await,
        Sent::Binary(Message { id: 2, .. })
    ));
    drop(busy);
    // A socket that never sends its request head times out after 2 s.
    let (_quiet, served) = tokio::io::duplex(1024);
    bench.server.accept(Box::new(served), peer(4));
    sleep(Duration::from_secs(3)).await;
    let reason = |last| bench.server.ban_reason(peer(last));
    assert_eq!(
        reason(2).as_deref(),
        Some("Did not make an endpoint request")
    );
    assert_eq!(reason(3), None);
    assert_eq!(
        reason(4).as_deref(),
        Some("Failed to complete websocket handshake")
    );
}

#[tokio::test(start_paused = true)]
async fn a_plain_http_request_is_answered_404_and_banned() {
    let bench = Bench::new().await;
    let (mut client, served) = tokio::io::duplex(1024);
    bench.server.accept(Box::new(served), peer(2));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    client.read_to_end(&mut answer).await.unwrap();
    assert!(answer.starts_with(b"HTTP/1.1 404 Not Found\r\n"));
    sleep(Duration::from_millis(1)).await;
    let reason = bench.server.ban_reason(peer(2));
    assert_eq!(reason.as_deref(), Some("Not a websocket connection"));
    // An upgrade to another path is dropped; the forger's address is never banned.
    let (mut client, served) = tokio::io::duplex(1024);
    bench.server.accept(Box::new(served), FORGER);
    let upgrade = "GET /x HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n";
    client.write_all(upgrade.as_bytes()).await.unwrap();
    assert_eq!(client.read(&mut [0; 16]).await.unwrap(), 0);
    let options = ServerOptions {
        ban_seconds: 0,
        ..options()
    };
    let quiet = Bench::with(options).await;
    let mut raw = quiet.raw(peer(2)).await.unwrap();
    raw.hello().await;
    raw.request(2, RouteId::GetSlotNumber, b"{}".to_vec()).await;
    assert_eq!(raw.next().await, Sent::Ended);
    assert_eq!(quiet.server.ban_reason(peer(2)), None);
    assert!(quiet.raw(peer(2)).await.is_some());
}

#[tokio::test(start_paused = true)]
async fn a_request_head_still_unfinished_after_60_s_is_answered_408_and_banned() {
    let bench = Bench::new().await;
    bench.fakes.node.set_first_synced(Some(SYNCED));
    let (mut client, served) = tokio::io::duplex(64 * 1024);
    bench.server.accept(Box::new(served), peer(2));
    let start = tokio::time::Instant::now();
    // One byte every 1.5 s keeps each read within the 2 s socket timeout, and never ends the
    // head.
    let dribble = async {
        for byte in b"GET / HTTP/1.1\r\nHost: x\r\nX-Slow: ".iter().cycle() {
            if client.write_all(&[*byte]).await.is_err() {
                break;
            }
            sleep(Duration::from_millis(1_500)).await;
        }
        start.elapsed()
    };
    let closed = tokio::time::timeout(Duration::from_secs(120), dribble).await;
    let closed = closed.expect("the server closes the connection");
    let window = Duration::from_secs(60)..=Duration::from_millis(61_500);
    assert!(window.contains(&closed), "{closed:?}");
    let mut answer = Vec::new();
    client.read_to_end(&mut answer).await.unwrap();
    assert_eq!(
        answer,
        b"HTTP/1.1 408 Request Timeout\r\nConnection: close\r\n\r\n"
    );
    let reason = bench.server.ban_reason(peer(2));
    assert_eq!(
        reason.as_deref(),
        Some("Failed to complete websocket handshake")
    );
}

#[tokio::test(start_paused = true)]
async fn an_upgrade_header_other_than_websocket_alone_is_refused_400() {
    let bench = Bench::new().await;
    for upgrade in ["websocket, foo", "foo, websocket", "websocket2"] {
        let (mut client, served) = tokio::io::duplex(1024);
        bench.server.accept(Box::new(served), peer(3));
        let request = format!(
            "GET / HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: {upgrade}\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        );
        client.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).await.unwrap();
        assert!(
            answer.starts_with(b"HTTP/1.1 400 Bad Request\r\n"),
            "{upgrade}"
        );
    }
    // A field sent twice is read as Node joins it, "websocket, websocket", "13, 13" or two keys
    // with ", " between them, which the library refuses as it would one such line.
    let fields = [
        ("Upgrade", "websocket"),
        ("Sec-WebSocket-Version", "13"),
        ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    for (twice, _) in fields {
        let (mut client, served) = tokio::io::duplex(1024);
        bench.server.accept(Box::new(served), peer(5));
        let mut request = String::from("GET / HTTP/1.1\r\nConnection: Upgrade\r\n");
        for (name, value) in fields {
            let times = if name == twice { 2 } else { 1 };
            for _ in 0..times {
                request.push_str(&format!("{name}: {value}\r\n"));
            }
        }
        request.push_str("\r\n");
        client.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).await.unwrap();
        assert!(
            answer.starts_with(b"HTTP/1.1 400 Bad Request\r\n"),
            "{twice} twice"
        );
    }
    // The comparison ignores case, as the library's does.
    let (mut client, served) = tokio::io::duplex(1024);
    bench.server.accept(Box::new(served), peer(4));
    let request = "GET / HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: WebSocket\r\n\
                   Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
    client.write_all(request.as_bytes()).await.unwrap();
    let mut answer = [0; 12];
    client.read_exact(&mut answer).await.unwrap();
    assert_eq!(&answer, b"HTTP/1.1 101");
}
