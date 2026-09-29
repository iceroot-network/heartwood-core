//! What a forger's link to its relays needs of the crate without the `node` feature: a session's
//! readiness, which host selection reads, and a TCP dial bound to a local address.
//!
//! The readiness tests run on tokio's paused clock over in-memory connections; the dial tests use
//! real loopback sockets.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_p2p::errors::SessionError;
use heartwood_p2p::nes::{ConnectOptions, ErrorHook, Session};
use heartwood_p2p::testing::{MockPeer, Reply, Script, upgrade};
use heartwood_p2p::transport::{self, Dial, Stream};
use heartwood_p2p_wire::RouteId;
use tokio::io::DuplexStream;
use tokio::net::TcpListener;
use tokio::time::sleep;

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn quiet() -> ErrorHook {
    Arc::new(|_: SessionError| {})
}

fn options() -> ConnectOptions {
    ConnectOptions {
        timeout: Duration::from_secs(5),
        retries: 1,
        ..ConnectOptions::default()
    }
}

/// A dial whose one connection is `client`; later dials are refused.
fn once(client: DuplexStream) -> Dial {
    let client = Arc::new(Mutex::new(Some(client)));
    Arc::new(move |_| {
        let stream = client.lock().unwrap().take();
        Box::pin(async move {
            stream
                .map(|stream| Box::new(stream) as Box<dyn Stream>)
                .ok_or_else(|| io::ErrorKind::ConnectionRefused.into())
        })
    })
}

#[tokio::test(start_paused = true)]
async fn a_session_is_ready_while_its_websocket_is_open() {
    let peer = MockPeer::scripted(LOCALHOST, Script::new()).await.unwrap();
    let (session, connected) =
        Session::connect(peer.address(), options(), quiet(), MockPeer::dial(&[&peer]));
    assert!(!session.is_ready(), "not before the session's task has run");
    connected.await.unwrap();
    assert!(session.is_ready());
    let clone = session.clone();
    session.terminate();
    sleep(Duration::from_millis(1)).await;
    assert!(!clone.is_ready(), "not after the session was terminated");
}

#[tokio::test(start_paused = true)]
async fn a_session_is_ready_from_the_upgrade_on_before_the_hello_is_answered() {
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    let address = SocketAddr::new(LOCALHOST, 4002);
    let (session, connected) = Session::connect(address, options(), quiet(), once(client));
    upgrade(&mut server).await.unwrap();
    sleep(Duration::from_millis(1)).await;
    assert!(
        session.is_ready(),
        "the reference's socket is open once upgraded"
    );
    drop(server);
    assert!(connected.await.is_err(), "the hello was never answered");
    assert!(!session.is_ready(), "not after the connection closed");
}

#[tokio::test(start_paused = true)]
async fn a_session_whose_peer_refuses_is_not_ready() {
    let address = SocketAddr::new(LOCALHOST, 4002);
    let refused: Dial = Arc::new(|_| {
        Box::pin(async { Err::<Box<dyn Stream>, _>(io::ErrorKind::ConnectionRefused.into()) })
    });
    let (session, connected) = Session::connect(address, options(), quiet(), refused);
    assert!(connected.await.is_err());
    assert!(!session.is_ready());
    sleep(Duration::from_secs(2)).await;
    assert!(!session.is_ready(), "nor after its reconnect failed");
}

#[tokio::test(start_paused = true)]
async fn the_attestation_routes_travel_over_a_session() {
    let script = Script::new()
        .on(
            RouteId::GetStatusToSign,
            Reply::Payload(br#"{"status":"unchanged"}"#.to_vec()),
        )
        .on(
            RouteId::PostStatusSignatures,
            Reply::Payload(b"{}".to_vec()),
        );
    let peer = MockPeer::scripted(LOCALHOST, script).await.unwrap();
    let (session, connected) =
        Session::connect(peer.address(), options(), quiet(), MockPeer::dial(&[&peer]));
    connected.await.unwrap();
    let poll = session
        .request(
            RouteId::GetStatusToSign,
            br#"{"publicKeys":[]}"#.to_vec(),
            Duration::from_secs(15),
            0,
        )
        .await
        .unwrap();
    assert_eq!(poll, br#"{"status":"unchanged"}"#);
    let posted = session
        .request(
            RouteId::PostStatusSignatures,
            br#"{"offer":"","signatures":[]}"#.to_vec(),
            Duration::from_secs(2),
            0,
        )
        .await
        .unwrap();
    assert_eq!(posted, b"{}");
    let routes: Vec<RouteId> = peer.requests().into_iter().map(|(r, _)| r).collect();
    assert_eq!(
        routes,
        [RouteId::GetStatusToSign, RouteId::PostStatusSignatures]
    );
}

/// Accept one connection on `listener` and give the peer's address.
async fn source_of(listener: &TcpListener, dial: Dial) -> io::Result<IpAddr> {
    let address = listener.local_addr()?;
    let (_, accepted) = tokio::join!(dial(address), listener.accept());
    Ok(accepted?.1.ip())
}

#[tokio::test]
async fn the_bound_dial_connects_from_its_local_address() {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let source = source_of(&listener, transport::tcp_bound(LOCALHOST)).await;
    assert_eq!(source.unwrap(), LOCALHOST);
}

/// On Linux the whole of 127.0.0.0/8 is local, which the devnet's address plans use.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_bound_dial_connects_from_another_loopback_address() {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let local = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 111));
    let source = source_of(&listener, transport::tcp_bound(local)).await;
    assert_eq!(source.unwrap(), local);
}

#[tokio::test]
async fn the_bound_dial_refuses_an_address_of_the_other_family() {
    let dial = transport::tcp_bound(IpAddr::V6(Ipv6Addr::LOCALHOST));
    let error = match dial(SocketAddr::new(LOCALHOST, 4002)).await {
        Ok(_) => panic!("an IPv4 address was dialled from an IPv6 one"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn a_session_over_the_bound_dial_reaches_a_mock_peer() {
    let script = Script::new().on(RouteId::GetSlotNumber, Reply::Payload(b"12".to_vec()));
    let peer = MockPeer::scripted(LOCALHOST, script).await.unwrap();
    let (session, connected) = Session::connect(
        peer.address(),
        options(),
        quiet(),
        transport::tcp_bound(LOCALHOST),
    );
    connected.await.unwrap();
    assert!(session.is_ready());
    let slot = session
        .request(
            RouteId::GetSlotNumber,
            b"{}".to_vec(),
            Duration::from_secs(2),
            0,
        )
        .await
        .unwrap();
    assert_eq!(slot, b"12");
    assert_eq!(peer.connections(), 1);
}
