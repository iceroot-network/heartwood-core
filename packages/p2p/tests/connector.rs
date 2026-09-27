//! The connector against mock peers: one session per IP, the pause between sessions to one IP,
//! concurrent requests sharing a session, and the disconnects.
//!
//! The tests run on tokio's paused clock over the mock peers' in-memory connections.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use heartwood_p2p::connector::{Connector, RECONNECT_PAUSE};
use heartwood_p2p::errors::{ProtocolError, SessionError};
use heartwood_p2p::seams::PeerTransport;
use heartwood_p2p::testing::{MockFrame, MockPeer, Reply, Script};
use heartwood_p2p_wire::RouteId;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

const LONG: Duration = Duration::from_secs(60);

type Answer = JoinHandle<(Result<Vec<u8>, SessionError>, Instant)>;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(127, 0, 0, last))
}

/// A peer on 127.0.0.`last` that answers getStatus `answers` times with the answer's number.
async fn peer(last: u8, answers: u8) -> MockPeer {
    let script = (1..=answers).fold(Script::new(), |script, n| {
        script.on(RouteId::GetStatus, Reply::Payload(vec![n]))
    });
    MockPeer::scripted(ip(last), script).await.unwrap()
}

/// getStatus to `address` through the connector, in a task; the answer and when it came.
fn emit(connector: &Connector, address: SocketAddr) -> Answer {
    let connector = connector.clone();
    tokio::spawn(async move {
        let answer = connector
            .emit(address, RouteId::GetStatus, vec![1], LONG, 0)
            .await;
        (answer, Instant::now())
    })
}

async fn answer(answer: Answer) -> Result<Vec<u8>, SessionError> {
    answer.await.unwrap().0
}

#[tokio::test(start_paused = true)]
async fn one_session_per_ip_whatever_the_port() {
    let (a, b) = (peer(1, 2).await, peer(2, 1).await);
    let connector = Connector::with_dial(MockPeer::dial(&[&a, &b]));
    assert_eq!(answer(emit(&connector, a.address())).await, Ok(vec![1]));
    let other_port = SocketAddr::new(a.address().ip(), a.address().port().wrapping_add(1));
    assert_eq!(answer(emit(&connector, other_port)).await, Ok(vec![2]));
    assert_eq!(answer(emit(&connector, b.address())).await, Ok(vec![1]));
    assert_eq!((a.connections(), b.connections()), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn concurrent_requests_to_one_ip_share_its_session() {
    let a = peer(1, 6).await;
    let connector = Connector::with_dial(MockPeer::dial(&[&a]));
    assert_eq!(answer(emit(&connector, a.address())).await, Ok(vec![1]));
    let pending: Vec<_> = (0..5).map(|_| emit(&connector, a.address())).collect();
    let mut answers = Vec::new();
    for request in pending {
        answers.push(answer(request).await.unwrap()[0]);
    }
    answers.sort_unstable();
    assert_eq!(answers, [2, 3, 4, 5, 6]);
    assert_eq!(a.connections(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_request_while_the_session_is_created_fails_at_once() {
    // As in the reference, the second request finds the session that the first one is still
    // connecting, and sending on it fails.
    let a = peer(1, 2).await;
    let connector = Connector::with_dial(MockPeer::dial(&[&a]));
    let first = emit(&connector, a.address());
    let second = emit(&connector, a.address());
    assert_eq!(answer(first).await, Ok(vec![1]));
    assert_eq!(answer(second).await, Err(SessionError::Disconnect));
    assert_eq!(a.connections(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_new_session_to_an_ip_waits_10_s_after_the_last_one() {
    let a = peer(1, 3).await;
    let connector = Connector::with_dial(MockPeer::dial(&[&a]));
    let start = Instant::now();
    assert_eq!(answer(emit(&connector, a.address())).await, Ok(vec![1]));
    sleep(Duration::from_secs(2)).await;
    connector.disconnect(a.address().ip());
    let (again, at) = emit(&connector, a.address()).await.unwrap();
    assert_eq!((again, at - start), (Ok(vec![2]), RECONNECT_PAUSE));
    assert_eq!(a.connections(), 2);
    // Once 10 s have passed since the last one, a session is created at once.
    sleep(RECONNECT_PAUSE).await;
    connector.disconnect(a.address().ip());
    let created = Instant::now();
    let (third, at) = emit(&connector, a.address()).await.unwrap();
    assert_eq!((third, at - created), (Ok(vec![3]), Duration::ZERO));
}

#[tokio::test(start_paused = true)]
async fn disconnect_terminates_the_session_and_its_requests() {
    let a = MockPeer::scripted(ip(1), Script::new()).await.unwrap();
    let connector = Connector::with_dial(MockPeer::dial(&[&a]));
    let pending = emit(&connector, a.address());
    sleep(ms(10)).await;
    PeerTransport::disconnect(&connector, a.address().ip());
    assert_eq!(answer(pending).await, Err(SessionError::Disconnect));
    // Disconnecting an IP without a session does nothing.
    connector.disconnect(ip(9));
}

#[tokio::test(start_paused = true)]
async fn a_session_that_reports_an_error_is_forgotten() {
    let text = Reply::Frames(vec![MockFrame::Text("{}".into())]);
    let script = Script::new()
        .on(RouteId::GetStatus, text)
        .on(RouteId::GetStatus, Reply::Payload(vec![2]));
    let a = MockPeer::scripted(ip(1), script).await.unwrap();
    let connector = Connector::with_dial(MockPeer::dial(&[&a]));
    let start = Instant::now();
    // The text frame is reported: the session is terminated, and its request fails with the
    // protocol error (the reference's fails as disconnected; both are of the class Error).
    assert_eq!(
        answer(emit(&connector, a.address())).await,
        Err(SessionError::Protocol(ProtocolError::TextFrame))
    );
    // The next request opens a new session, 10 s after the first was created.
    let (answer, at) = emit(&connector, a.address()).await.unwrap();
    assert_eq!((answer, at - start), (Ok(vec![2]), RECONNECT_PAUSE));
    assert_eq!(a.connections(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_session_closed_for_good_stays_until_the_peer_is_disconnected() {
    let ping = Reply::Frames(vec![MockFrame::Ping]);
    let script = Script::new()
        .on(RouteId::GetStatus, ping)
        .on(RouteId::GetStatus, Reply::Payload(vec![2]));
    let a = MockPeer::scripted(ip(1), script).await.unwrap();
    let connector = Connector::with_dial(MockPeer::dial(&[&a]));
    assert_eq!(
        answer(emit(&connector, a.address())).await,
        Err(SessionError::Disconnect)
    );
    sleep(RECONNECT_PAUSE).await;
    assert_eq!(
        answer(emit(&connector, a.address())).await,
        Err(SessionError::Disconnect)
    );
    assert_eq!(a.connections(), 1);
    connector.disconnect(a.address().ip());
    assert_eq!(answer(emit(&connector, a.address())).await, Ok(vec![2]));
    assert_eq!(a.connections(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_failed_first_connection_stays_while_it_reconnects() {
    let connector = Connector::with_dial(MockPeer::dial(&[]));
    let nobody = SocketAddr::new(ip(7), 4002);
    let terminated = SessionError::Ws("Connection terminated while waiting to connect".into());
    assert_eq!(answer(emit(&connector, nobody)).await, Err(terminated));
    // The session waits 1 s to reconnect; meanwhile a request fails at once.
    let (answer_now, _) = emit(&connector, nobody).await.unwrap();
    assert_eq!(answer_now, Err(SessionError::Disconnect));
    // The reconnect fails and is reported, so the session is forgotten; the next one waits for
    // the rest of the 10 s.
    sleep(ms(1_001)).await;
    let start = Instant::now();
    let (answer_later, at) = emit(&connector, nobody).await.unwrap();
    assert!(matches!(answer_later, Err(SessionError::Ws(_))));
    assert_eq!(at - start, ms(8_999));
}

#[tokio::test(start_paused = true)]
async fn the_last_error_is_kept_per_ip() {
    let connector = Connector::new();
    assert_eq!(connector.last_error(ip(1)), None);
    PeerTransport::set_error(&connector, ip(1), "Error");
    connector.set_error(ip(2), "TypeError");
    assert_eq!(connector.last_error(ip(1)), Some("Error"));
    PeerTransport::forget_error(&connector, ip(1));
    assert_eq!(connector.last_error(ip(1)), None);
    assert_eq!(connector.last_error(ip(2)), Some("TypeError"));
}

#[tokio::test(start_paused = true)]
async fn the_transport_trait_emits_through_the_connector() {
    let a = peer(1, 1).await;
    let connector = Connector::with_dial(MockPeer::dial(&[&a]));
    let answer = PeerTransport::emit(
        &connector,
        a.address(),
        RouteId::GetStatus,
        vec![1],
        LONG,
        0,
    );
    assert_eq!(answer.await, Ok(vec![1]));
    assert_eq!(a.requests(), vec![(RouteId::GetStatus, vec![1])]);
}

#[tokio::test]
async fn the_connector_reaches_a_mock_peer_over_tcp() {
    let a = peer(1, 1).await;
    let connector = Connector::new();
    let answer = connector
        .emit(a.address(), RouteId::GetStatus, vec![1], LONG, 0)
        .await;
    assert_eq!(answer, Ok(vec![1]));
}
