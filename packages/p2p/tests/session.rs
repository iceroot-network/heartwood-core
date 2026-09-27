//! The nes client session against the mock peer: hello, heartbeat, pings, out-of-order responses,
//! the connection-wide timeout and receive limit, the protocol errors and the reconnect.
//!
//! Every test runs on tokio's paused clock over in-memory connections (the paused clock cannot
//! drive real sockets), so the reference's delays (the 20 s heartbeat, the 1 s ping limit, the
//! 1 s reconnect) are exact. Some tests use a peer that writes and reads the WebSocket bytes
//! itself. The frames the session writes are compared with the reference's client frames of the
//! wire vectors, and `devnet_smoke` runs against a reference node when `HEARTWOOD_SMOKE_PEER`
//! names one.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_p2p::errors::{ProtocolError, SessionError};
use heartwood_p2p::nes::{ConnectOptions, DEFAULT_MAX_PAYLOAD, ErrorHook, Session};
use heartwood_p2p::testing::{MockFrame, MockPeer, Reply, Script};
use heartwood_p2p::transport::{Dial, Stream};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::{Heartbeat, Kind, Message};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

const KIB: usize = 1024;
const LONG: Duration = Duration::from_secs(60);
const HEARTBEAT: Heartbeat = Heartbeat {
    interval: 15_000,
    timeout: 5_000,
};

type Errors = Arc<Mutex<Vec<SessionError>>>;
type Answer = JoinHandle<(Result<Vec<u8>, SessionError>, Instant)>;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn secs(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

fn recorder() -> (ErrorHook, Errors) {
    let errors = Errors::default();
    let sink = errors.clone();
    (
        Arc::new(move |error| sink.lock().unwrap().push(error)),
        errors,
    )
}

fn seen(errors: &Errors) -> Vec<SessionError> {
    errors.lock().unwrap().clone()
}

fn protocol(error: ProtocolError) -> SessionError {
    SessionError::Protocol(error)
}

fn after(delay: Duration, reply: Reply) -> Reply {
    Reply::After(delay, Box::new(reply))
}

fn big(size: usize) -> Reply {
    Reply::Payload(vec![0x5a; size])
}

fn address() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4002)
}

fn start(
    dial: Dial,
) -> (
    Session,
    Errors,
    impl Future<Output = Result<(), SessionError>>,
) {
    let (hook, errors) = recorder();
    let (session, connected) = Session::connect(address(), ConnectOptions::default(), hook, dial);
    (session, errors, connected)
}

/// A scripted mock peer and a session connected to it.
async fn scripted(script: Script) -> (MockPeer, Session, Errors) {
    let peer = MockPeer::scripted(address().ip(), script).await.unwrap();
    let (session, errors, connected) = start(MockPeer::dial(&[&peer]));
    connected.await.unwrap();
    (peer, session, errors)
}

/// getStatus with a one-byte payload, in a task; the answer and when it came.
fn ask(session: &Session, timeout: Duration, max_payload: usize) -> Answer {
    let session = session.clone();
    tokio::spawn(async move {
        let answer = session
            .request(RouteId::GetStatus, vec![1], timeout, max_payload)
            .await;
        (answer, Instant::now())
    })
}

async fn answer(answer: Answer) -> Result<Vec<u8>, SessionError> {
    answer.await.unwrap().0
}

/// A peer on one in-memory connection that writes and reads the WebSocket bytes itself.
struct Wire {
    stream: DuplexStream,
    buffer: Vec<u8>,
}

/// `n` wire peers and the dial that connects to each in turn, once; later dials are refused.
fn wires(n: usize) -> (Vec<Wire>, Dial) {
    let (clients, peers): (Vec<_>, Vec<_>) = (0..n)
        .map(|_| {
            let (client, server) = tokio::io::duplex(64 * KIB);
            let peer = Wire {
                stream: server,
                buffer: Vec::new(),
            };
            (client, peer)
        })
        .unzip();
    let clients = Mutex::new(clients.into_iter());
    let dial: Dial = Arc::new(move |_| {
        let stream = clients.lock().unwrap().next();
        Box::pin(async move {
            stream
                .map(|stream| Box::new(stream) as Box<dyn Stream>)
                .ok_or_else(|| io::ErrorKind::ConnectionRefused.into())
        })
    });
    (peers, dial)
}

fn wire() -> (Wire, Dial) {
    let (mut peers, dial) = wires(1);
    (peers.remove(0), dial)
}

/// A server frame (unmasked) with `opcode` and `payload`.
fn frame(first: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![first];
    match payload.len() {
        n if n < 126 => frame.push(n as u8),
        n if n < 65_536 => {
            frame.push(126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

impl Wire {
    async fn fill(&mut self, n: usize) -> bool {
        while self.buffer.len() < n {
            if self.stream.read_buf(&mut self.buffer).await.unwrap() == 0 {
                return false;
            }
        }
        true
    }

    fn take(&mut self, n: usize) -> Vec<u8> {
        self.buffer.drain(..n).collect()
    }

    /// Read the upgrade request and accept it.
    async fn upgrade(&mut self) -> String {
        while !self.buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            assert!(self.stream.read_buf(&mut self.buffer).await.unwrap() > 0);
        }
        let end = self
            .buffer
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        let request = String::from_utf8(self.take(end)).unwrap();
        let key = request
            .lines()
            .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
            .unwrap();
        let accept = derive_accept_key(key.as_bytes());
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        self.write(response.as_bytes()).await;
        request
    }

    async fn write(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    async fn send(&mut self, message: &Message) {
        self.write(&frame(0x82, &message.encode().unwrap())).await;
    }

    /// The next client frame: its first byte and its unmasked payload; `None` at the end.
    async fn frame(&mut self) -> Option<(u8, Vec<u8>)> {
        if !self.fill(2).await {
            return None;
        }
        let (first, second) = (self.buffer[0], self.buffer[1]);
        assert_eq!(second & 0x80, 0x80, "a client frame is masked");
        let (extra, length) = match second & 0x7f {
            126 => (2, None),
            127 => (8, None),
            n => (0, Some(n as usize)),
        };
        assert!(self.fill(2 + extra + 4).await);
        let header = self.take(2 + extra);
        let length = length.unwrap_or_else(|| {
            header[2..]
                .iter()
                .fold(0usize, |total, byte| total << 8 | usize::from(*byte))
        });
        let mask = self.take(4);
        assert!(self.fill(length).await);
        let payload = (self.take(length).iter().enumerate())
            .map(|(i, byte)| byte ^ mask[i % 4])
            .collect();
        Some((first, payload))
    }

    /// The next client frame as a nes message, with its bytes.
    async fn message(&mut self) -> (Message, Vec<u8>) {
        let (first, payload) = self.frame().await.expect("a frame");
        assert_eq!(first, 0x82, "a final binary frame");
        (Message::parse(&payload).unwrap(), payload)
    }

    /// Answer the hello.
    async fn hello(&mut self, heartbeat: Heartbeat) -> Vec<u8> {
        self.upgrade().await;
        let (hello, bytes) = self.message().await;
        assert_eq!(hello.kind, Kind::Hello);
        self.send(&Message::server_hello(hello.id, heartbeat, "wire".into()))
            .await;
        bytes
    }
}

async fn wire_session() -> (Wire, Session, Errors) {
    let (mut peer, dial) = wire();
    let (session, errors, connected) = start(dial);
    peer.hello(HEARTBEAT).await;
    connected.await.unwrap();
    (peer, session, errors)
}

#[tokio::test(start_paused = true)]
async fn the_upgrade_asks_for_path_slash_without_compression() {
    let (mut peer, dial) = wire();
    let (_session, _, _connected) = start(dial);
    let request = peer.upgrade().await;
    assert!(request.starts_with("GET / HTTP/1.1\r\n"), "{request}");
    assert!(
        request.contains("\r\nHost: 127.0.0.1:4002\r\n"),
        "{request}"
    );
    assert!(
        request.contains("\r\nSec-WebSocket-Version: 13\r\n"),
        "{request}"
    );
    assert!(!request.contains("Extensions"), "{request}");
}

#[tokio::test(start_paused = true)]
async fn the_hello_is_type_0_version_2_with_the_first_id() {
    let (mut peer, dial) = wire();
    let (_session, _, connected) = start(dial);
    peer.upgrade().await;
    let (hello, _) = peer.message().await;
    assert_eq!(
        (hello.kind, hello.version, hello.id, hello.status_code),
        (Kind::Hello, 2, 1, 200)
    );
    assert!(hello.path.is_empty() && hello.socket.is_empty() && hello.payload.is_empty());
    peer.send(&Message::server_hello(1, HEARTBEAT, "wire".into()))
        .await;
    connected.await.unwrap();
}

/// The client frames of the wire vectors (`sent: ...` records of V13): the hello, the requests of
/// the twelve routes with ids 2 to 13, and the ping reply with id 14.
fn client_frames() -> Vec<(String, Value)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::var_os("WIRE_VECTORS").map_or_else(
        || manifest.join("../p2p-wire/tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let text = std::fs::read_to_string(dir.join("V13-p2p-wire.jsonl")).unwrap();
    text.lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["op"] == "nes.stringify")
        .filter_map(|record| {
            let name = record["name"].as_str()?.strip_prefix("sent: ")?.to_owned();
            let client = name == "client hello"
                || name == "client ping reply"
                || name.starts_with("request p2p.");
            client.then_some((name, record))
        })
        .collect()
}

fn unhex(text: &Value) -> Vec<u8> {
    let text = text.as_str().unwrap();
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn the_frames_the_session_writes_are_the_reference_clients() {
    let records = client_frames();
    assert_eq!(records.len(), 14, "V13's client frames");
    let (mut peer, dial) = wire();
    let (session, _, connected) = start(dial);
    let mut compared = 0;
    for (name, record) in records {
        let expected = unhex(&record["output"]["hex"]);
        let message = &record["input"]["message"];
        let written = match name.as_str() {
            "client hello" => peer.hello(HEARTBEAT).await,
            "client ping reply" => {
                peer.send(&Message::ping(0)).await;
                peer.message().await.1
            }
            _ => {
                let route = RouteId::from_id(message["path"].as_str().unwrap()).unwrap();
                let payload = unhex(&message["payload"]);
                let session = session.clone();
                tokio::spawn(async move { session.request(route, payload, LONG, 0).await });
                peer.message().await.1
            }
        };
        assert_eq!(written, expected, "{name}");
        compared += 1;
        if compared == 1 {
            // The requests follow the hello once it is answered.
            sleep(ms(1)).await;
        }
    }
    connected.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn requests_are_answered_by_id_in_any_order() {
    let script = Script::new()
        .on(RouteId::GetStatus, after(secs(2), Reply::Payload(vec![1])))
        .on(RouteId::GetStatus, Reply::Payload(vec![2]));
    let (peer, session, errors) = scripted(script).await;
    let first = ask(&session, LONG, 0);
    sleep(ms(10)).await;
    let second = ask(&session, LONG, 0);
    let (second, second_at) = second.await.unwrap();
    let (first, first_at) = first.await.unwrap();
    assert_eq!((first, second), (Ok(vec![1]), Ok(vec![2])));
    assert!(second_at < first_at);
    assert_eq!(peer.requests(), vec![(RouteId::GetStatus, vec![1]); 2]);
    assert_eq!(peer.connections(), 1);
    assert!(seen(&errors).is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_heartbeat_expires_20_s_after_the_last_message() {
    let (_peer, session, errors) = scripted(Script::new()).await;
    let start = Instant::now();
    let pending = ask(&session, LONG, 0);
    sleep(ms(19_999)).await;
    assert!(seen(&errors).is_empty());
    let (result, at) = pending.await.unwrap();
    assert_eq!(
        (result, at - start),
        (Err(SessionError::Disconnect), secs(20))
    );
    assert_eq!(
        seen(&errors),
        vec![protocol(ProtocolError::HeartbeatTimeout)]
    );
}

#[tokio::test(start_paused = true)]
async fn every_message_restarts_the_heartbeat() {
    let script = Script {
        pings: vec![secs(15), secs(15)],
        ..Script::new()
    };
    let (_peer, _session, errors) = scripted(script).await;
    sleep(ms(49_999)).await;
    assert!(seen(&errors).is_empty());
    sleep(ms(2)).await;
    assert_eq!(
        seen(&errors),
        vec![protocol(ProtocolError::HeartbeatTimeout)]
    );
}

#[tokio::test(start_paused = true)]
async fn the_heartbeat_is_the_hellos_interval_plus_timeout() {
    let heartbeat = Heartbeat {
        interval: 3_000,
        timeout: 2_000,
    };
    let (_peer, _session, errors) = scripted(Script {
        heartbeat,
        ..Script::new()
    })
    .await;
    sleep(ms(4_999)).await;
    assert!(seen(&errors).is_empty());
    sleep(ms(2)).await;
    assert_eq!(
        seen(&errors),
        vec![protocol(ProtocolError::HeartbeatTimeout)]
    );

    // No heartbeat at all never expires.
    let heartbeat = Heartbeat::default();
    let (_peer, _session, errors) = scripted(Script {
        heartbeat,
        ..Script::new()
    })
    .await;
    sleep(secs(3_600)).await;
    assert!(seen(&errors).is_empty());
}

#[tokio::test(start_paused = true)]
async fn server_pings_are_answered_with_the_next_id_at_most_once_a_second() {
    let (mut peer, session, errors) = wire_session().await;
    peer.send(&Message::ping(0)).await;
    let (reply, _) = peer.message().await;
    assert_eq!((reply.kind, reply.id), (Kind::Ping, 2));
    sleep(secs(1)).await;
    peer.send(&Message::ping(0)).await;
    assert_eq!(peer.message().await.0.id, 3);
    // A ping less than 1 s after the last one is a flood, and gets no reply.
    sleep(ms(999)).await;
    peer.send(&Message::ping(0)).await;
    sleep(ms(1)).await;
    assert_eq!(seen(&errors), vec![protocol(ProtocolError::PingFlood)]);
    // The flood counts as the last ping.
    sleep(ms(999)).await;
    peer.send(&Message::ping(0)).await;
    assert_eq!(peer.message().await.0.id, 4);
    // Requests take the next id; a reply's id is not tracked.
    let _pending = ask(&session, LONG, 0);
    assert_eq!(peer.message().await.0.id, 5);
    peer.send(&Message::response(4, Vec::new())).await;
    sleep(ms(1)).await;
    let expected = [ProtocolError::PingFlood, ProtocolError::UnknownRequest];
    assert_eq!(seen(&errors), expected.map(protocol));
}

#[tokio::test(start_paused = true)]
async fn each_request_sets_the_connection_wide_timeout_and_keeps_its_own() {
    let (_peer, session, _) = scripted(Script::new()).await;
    let start = Instant::now();
    let a = ask(&session, secs(5), 0);
    sleep(secs(1)).await;
    let b = ask(&session, secs(1), 0);
    sleep(ms(10)).await;
    // A zero keeps the setting of the last request: 1 s.
    let c = ask(&session, Duration::ZERO, 0);
    let mut ends = Vec::new();
    for pending in [a, b, c] {
        let (result, at) = pending.await.unwrap();
        assert_eq!(result, Err(SessionError::Timeout));
        ends.push(at - start);
    }
    assert_eq!(ends, [secs(5), secs(2), ms(2_010)]);
}

#[tokio::test(start_paused = true)]
async fn the_first_requests_timeout_is_10_s() {
    let (_peer, session, _) = scripted(Script::new()).await;
    let start = Instant::now();
    let (result, at) = ask(&session, Duration::ZERO, 0).await.unwrap();
    assert_eq!((result, at - start), (Err(SessionError::Timeout), secs(10)));
}

#[tokio::test(start_paused = true)]
async fn a_response_after_its_timeout_is_for_an_unknown_request() {
    let script = Script::new().on(RouteId::GetStatus, after(secs(3), Reply::Payload(vec![])));
    let (_peer, session, errors) = scripted(script).await;
    assert_eq!(
        answer(ask(&session, secs(2), 0)).await,
        Err(SessionError::Timeout)
    );
    sleep(ms(1_001)).await;
    assert_eq!(seen(&errors), vec![protocol(ProtocolError::UnknownRequest)]);
}

#[tokio::test(start_paused = true)]
async fn a_request_raises_the_receive_limit_and_every_message_resets_it() {
    let script = Script::new()
        .on(RouteId::GetStatus, big(150 * KIB))
        .on(RouteId::GetStatus, big(DEFAULT_MAX_PAYLOAD))
        .on(RouteId::GetStatus, Reply::Payload(vec![1]))
        .on(RouteId::GetStatus, after(secs(1), big(150 * KIB)));
    let (peer, session, errors) = scripted(script).await;
    // Raised: 150 KiB is read under a limit of 200 KiB.
    let raised = answer(ask(&session, LONG, 200 * KIB)).await;
    assert_eq!(raised.map(|payload| payload.len()), Ok(150 * KIB));
    // Reset: a message of 100 KiB of payload and its nes header is above 100 KiB.
    assert_eq!(
        answer(ask(&session, LONG, 0)).await,
        Err(SessionError::Disconnect)
    );
    sleep(ms(1_001)).await;
    assert_eq!(peer.connections(), 2, "the one reconnect");
    // Both requests raise the limit; the first answer resets it, so the second is too long.
    let first = ask(&session, LONG, 200 * KIB);
    let second = ask(&session, LONG, 200 * KIB);
    assert_eq!(answer(first).await, Ok(vec![1]));
    assert_eq!(answer(second).await, Err(SessionError::Disconnect));
    // No reconnect is left: the session reports the failure and stays down.
    sleep(secs(10)).await;
    assert_eq!(peer.connections(), 2);
    assert_eq!(seen(&errors), vec![SessionError::Ws("Socket error".into())]);
    assert_eq!(
        answer(ask(&session, LONG, 0)).await,
        Err(SessionError::Disconnect)
    );
}

#[tokio::test(start_paused = true)]
async fn a_server_ping_does_not_reset_the_receive_limit() {
    let script = Script {
        pings: vec![ms(500)],
        ..Script::new().on(RouteId::GetStatus, after(secs(1), big(150 * KIB)))
    };
    let (_peer, session, errors) = scripted(script).await;
    let raised = answer(ask(&session, LONG, 200 * KIB)).await;
    assert_eq!(raised.map(|payload| payload.len()), Ok(150 * KIB));
    assert!(seen(&errors).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_frame_above_the_limit_closes_with_1009_before_its_payload_is_read() {
    let (mut peer, session, _) = wire_session().await;
    let pending = ask(&session, LONG, 0);
    peer.message().await;
    // Only the header of a frame that declares 100 KiB + 1 bytes.
    let mut header = vec![0x82, 127];
    header.extend_from_slice(&(DEFAULT_MAX_PAYLOAD as u64 + 1).to_be_bytes());
    peer.write(&header).await;
    assert_eq!(answer(pending).await, Err(SessionError::Disconnect));
    let (first, payload) = peer.frame().await.unwrap();
    assert_eq!((first, payload), (0x88, 1009u16.to_be_bytes().to_vec()));
    // The frames of a message count together.
    let (mut peer, session, _) = wire_session().await;
    let pending = ask(&session, LONG, 0);
    peer.message().await;
    peer.write(&frame(0x02, &[0; 60 * KIB])).await;
    peer.write(&frame(0x80, &[0; 50 * KIB])[..4]).await;
    assert_eq!(answer(pending).await, Err(SessionError::Disconnect));
    assert_eq!(peer.frame().await.unwrap().1, 1009u16.to_be_bytes());
}

#[tokio::test(start_paused = true)]
async fn a_frame_that_breaks_the_protocol_closes_with_1002_and_bad_text_with_1007() {
    for (bytes, code) in [
        (frame(0xc2, b"x"), 1002u16),       // compressed
        (frame(0x80, b"x"), 1002),          // a continuation of nothing
        (frame(0x09, b""), 1002),           // a ping that is not final
        (frame(0x83, b"x"), 1002),          // a reserved opcode
        (frame(0x81, &[0xff]), 1007),       // text that is not UTF-8
        (frame(0x88, &[0x03]), 1002),       // a close frame of one byte
        (frame(0x88, &[0x03, 0xec]), 1002), // close code 1004
    ] {
        let (mut peer, session, _) = wire_session().await;
        let pending = ask(&session, LONG, 0);
        peer.message().await;
        peer.write(&bytes).await;
        assert_eq!(
            answer(pending).await,
            Err(SessionError::Disconnect),
            "{bytes:02x?}"
        );
        assert_eq!(
            peer.frame().await.unwrap().1,
            code.to_be_bytes(),
            "{bytes:02x?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_close_frame_is_echoed_and_the_session_reconnects_once() {
    let script = Script::new()
        .on(RouteId::GetStatus, Reply::Close)
        .on(RouteId::GetStatus, Reply::Payload(vec![2]))
        .on(RouteId::GetStatus, Reply::Close);
    let (peer, session, errors) = scripted(script).await;
    let start = Instant::now();
    assert_eq!(
        answer(ask(&session, LONG, 0)).await,
        Err(SessionError::Disconnect)
    );
    // While it waits to reconnect, a request fails at once.
    let (result, at) = ask(&session, LONG, 0).await.unwrap();
    assert_eq!(
        (result, at - start),
        (Err(SessionError::Disconnect), Duration::ZERO)
    );
    sleep(ms(999)).await;
    assert_eq!(peer.connections(), 1);
    sleep(ms(2)).await;
    assert_eq!(peer.connections(), 2);
    assert_eq!(answer(ask(&session, LONG, 0)).await, Ok(vec![2]));
    // The second close is final: no reconnect and no error report.
    assert_eq!(
        answer(ask(&session, LONG, 0)).await,
        Err(SessionError::Disconnect)
    );
    sleep(secs(60)).await;
    assert_eq!(peer.connections(), 2);
    assert!(seen(&errors).is_empty());
    assert_eq!(
        answer(ask(&session, LONG, 0)).await,
        Err(SessionError::Disconnect)
    );
}

#[tokio::test(start_paused = true)]
async fn the_reconnect_sends_a_fresh_hello_with_the_next_id() {
    let (mut peers, dial) = wires(2);
    let (session, errors, connected) = start(dial);
    peers[0].hello(HEARTBEAT).await;
    connected.await.unwrap();
    let pending = ask(&session, LONG, 0);
    assert_eq!(peers[0].message().await.0.id, 2);
    peers[0].write(&frame(0x88, &1000u16.to_be_bytes())).await;
    assert_eq!(answer(pending).await, Err(SessionError::Disconnect));
    assert_eq!(
        peers[0].frame().await.unwrap(),
        (0x88, 1000u16.to_be_bytes().to_vec())
    );
    let closed = Instant::now();
    peers[1].upgrade().await;
    assert_eq!(closed.elapsed(), secs(1));
    let (hello, _) = peers[1].message().await;
    assert_eq!((hello.kind, hello.version, hello.id), (Kind::Hello, 2, 3));
    peers[1]
        .send(&Message::server_hello(3, HEARTBEAT, "wire".into()))
        .await;
    let pending = ask(&session, LONG, 0);
    let (request, _) = peers[1].message().await;
    peers[1].send(&Message::response(request.id, vec![5])).await;
    assert_eq!((request.id, answer(pending).await), (4, Ok(vec![5])));
    assert!(seen(&errors).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_status_of_400_or_more_fails_the_request_with_the_first_512_bytes() {
    let long = [b"x".repeat(600)].concat();
    let script = Script::new()
        .on(
            RouteId::GetStatus,
            Reply::Error {
                status: 499,
                payload: br#"{"error":"graceful"}"#.to_vec(),
            },
        )
        .on(
            RouteId::GetStatus,
            Reply::Error {
                status: 400,
                payload: long,
            },
        )
        .on(RouteId::GetStatus, Reply::Payload(vec![3]));
    let (_peer, session, errors) = scripted(script).await;
    let graceful = SessionError::Server {
        status: 499,
        payload: r#"{"error":"graceful"}"#.into(),
    };
    assert_eq!(answer(ask(&session, LONG, 0)).await, Err(graceful));
    let cut = SessionError::Server {
        status: 400,
        payload: "x".repeat(512),
    };
    assert_eq!(answer(ask(&session, LONG, 0)).await, Err(cut));
    // The connection stays open.
    assert_eq!(answer(ask(&session, LONG, 0)).await, Ok(vec![3]));
    assert!(seen(&errors).is_empty());
}

#[tokio::test(start_paused = true)]
async fn protocol_errors_are_reported_and_leave_the_session_open() {
    let unknown = Message::response(99, vec![]);
    let error_type = Message {
        kind: Kind::Error,
        ..Message::response(0, vec![])
    };
    let script = Script::new()
        .on(
            RouteId::GetStatus,
            Reply::Frames(vec![MockFrame::Text("{}".into())]),
        )
        .on(
            RouteId::GetStatus,
            Reply::Frames(vec![MockFrame::Binary(vec![0; 13])]),
        )
        .on(
            RouteId::GetStatus,
            Reply::Frames(vec![MockFrame::Binary(unknown.encode().unwrap())]),
        )
        .on(RouteId::GetStatus, Reply::Payload(vec![4]));
    let (_peer, session, errors) = scripted(script).await;
    for _ in 0..3 {
        let _ignored = ask(&session, secs(1), 0);
        sleep(ms(10)).await;
    }
    assert_eq!(answer(ask(&session, LONG, 0)).await, Ok(vec![4]));
    let reported: Vec<_> = seen(&errors)
        .into_iter()
        .map(|error| match error {
            SessionError::Protocol(ProtocolError::Frame(_)) => "frame",
            SessionError::Protocol(ProtocolError::TextFrame) => "text",
            SessionError::Protocol(ProtocolError::UnknownRequest) => "unknown",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(reported, ["text", "frame", "unknown"]);

    // A message of the error type fails its request and is reported.
    let (mut peer, session, errors) = wire_session().await;
    let pending = ask(&session, LONG, 0);
    let (request, _) = peer.message().await;
    peer.send(&Message {
        id: request.id,
        ..error_type
    })
    .await;
    let failed = Err(protocol(ProtocolError::UnknownType));
    assert_eq!(answer(pending).await, failed);
    assert_eq!(seen(&errors), vec![protocol(ProtocolError::UnknownType)]);
}

#[tokio::test(start_paused = true)]
async fn a_websocket_ping_or_pong_ends_the_connection_for_good() {
    for control in [MockFrame::Ping, MockFrame::Pong] {
        let script = Script::new().on(RouteId::GetStatus, Reply::Frames(vec![control]));
        let (peer, session, errors) = scripted(script).await;
        assert_eq!(
            answer(ask(&session, LONG, 0)).await,
            Err(SessionError::Disconnect)
        );
        sleep(secs(10)).await;
        assert_eq!(peer.connections(), 1, "no reconnect");
        assert!(seen(&errors).is_empty());
    }
    // A ping is answered with a pong before the connection ends; the client never pings.
    let (mut peer, session, _) = wire_session().await;
    let pending = ask(&session, LONG, 0);
    peer.message().await;
    peer.write(&frame(0x89, b"hi")).await;
    assert_eq!(answer(pending).await, Err(SessionError::Disconnect));
    assert_eq!(peer.frame().await, Some((0x8a, b"hi".to_vec())));
    assert_eq!(peer.frame().await, None);
}

#[tokio::test(start_paused = true)]
async fn a_failed_connection_reconnects_once_then_reports() {
    let (_session, errors, connected) = start(MockPeer::dial(&[]));
    let terminated = "Connection terminated while waiting to connect";
    assert_eq!(connected.await, Err(SessionError::Ws(terminated.into())));
    sleep(ms(1_001)).await;
    assert_eq!(seen(&errors), vec![SessionError::Ws("Socket error".into())]);
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_does_not_open_times_out_after_5_s_and_reconnects_once() {
    let dial: Dial = Arc::new(|_| Box::pin(std::future::pending()));
    let start_at = Instant::now();
    let (_session, errors, connected) = start(dial);
    assert_eq!(connected.await, Err(SessionError::Timeout));
    assert_eq!(start_at.elapsed(), secs(5));
    sleep(ms(5_999)).await;
    assert!(seen(&errors).is_empty());
    sleep(ms(2)).await;
    assert_eq!(seen(&errors), vec![SessionError::Timeout]);
}

#[tokio::test(start_paused = true)]
async fn a_hello_answered_with_an_error_fails_the_connection_for_good() {
    let (mut peer, dial) = wire();
    let (session, errors, connected) = start(dial);
    peer.upgrade().await;
    let (hello, _) = peer.message().await;
    let refused = Message {
        status_code: 400,
        kind: Kind::Hello,
        ..Message::response(hello.id, b"no".to_vec())
    };
    peer.send(&refused).await;
    let error = SessionError::Server {
        status: 400,
        payload: "no".into(),
    };
    assert_eq!(connected.await, Err(error));
    // The client closes the connection and does not reconnect.
    assert_eq!(peer.frame().await.map(|frame| frame.0), Some(0x88));
    sleep(secs(10)).await;
    assert!(seen(&errors).is_empty());
    assert_eq!(
        answer(ask(&session, LONG, 0)).await,
        Err(SessionError::Disconnect)
    );
}

#[tokio::test(start_paused = true)]
async fn terminate_fails_the_pending_requests_and_ends_the_session() {
    let (peer, session, errors) = scripted(Script::new()).await;
    let pending = ask(&session, LONG, 0);
    sleep(ms(1)).await;
    session.terminate();
    assert_eq!(answer(pending).await, Err(SessionError::Disconnect));
    sleep(secs(10)).await;
    assert_eq!(peer.connections(), 1);
    assert!(seen(&errors).is_empty());
    assert_eq!(
        answer(ask(&session, LONG, 0)).await,
        Err(SessionError::Disconnect)
    );
}
