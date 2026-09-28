//! Mode B on loopback: the candidate rule (other addresses of 127.0.0.0/8, never the node's own
//! nor a `remoteAccess` address), outbound sockets bound to the node's address (the connector and
//! the port probe), the runtime guards, and the server over real TCP: its accept loop, the forger's
//! sessions closed first, and its stop.
//!
//! The tests use the addresses 127.11.0.x and real sockets.

#![cfg(all(feature = "devnet", feature = "difftest"))]

mod server_support;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use heartwood_p2p::connector::Connector;
use heartwood_p2p::devnet::{Addressing, guard, is_candidate};
use heartwood_p2p::seams::{PeerRules, PortProbe};
use heartwood_p2p::seeds::HttpProbe;
use heartwood_p2p::server::ServerOptions;
use heartwood_p2p::utils::rules::Rules;
use heartwood_p2p::{P2pOptions, P2pServer, ServerParts};
use heartwood_p2p_wire::RouteId;
use server_support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn alias(address: &str) -> Addressing {
    Addressing::Alias {
        address: ip(address),
    }
}

/// A mode B node at 127.11.0.11 whose forgers are at its own address and 127.11.0.111.
fn node_options() -> P2pOptions {
    P2pOptions {
        host: ip("127.11.0.11"),
        addressing: alias("127.11.0.11"),
        remote_access: vec!["127.11.0.11".to_owned(), "127.11.0.111".to_owned()],
        ..P2pOptions::default()
    }
}

#[test]
fn mode_b_candidates_are_the_other_loopback_addresses() {
    let forgers = [ip("127.11.0.111")];
    let node = alias("127.11.0.11");
    assert!(is_candidate(&node, ip("127.11.0.12"), &forgers));
    assert!(is_candidate(&node, ip("127.0.0.1"), &forgers));
    assert!(is_candidate(&node, ip("::ffff:127.11.0.12"), &forgers));
    for refused in [
        "127.11.0.11",
        "127.11.0.111",
        "10.0.0.1",
        "192.0.2.1",
        "::1",
    ] {
        assert!(!is_candidate(&node, ip(refused), &forgers), "{refused}");
    }
    assert!(is_candidate(&Addressing::Off, ip("127.11.0.11"), &forgers));
}

#[test]
fn the_rules_take_the_devnet_rule_in_mode_b_and_the_reference_rule_otherwise() {
    let rules = Rules::from_options(&node_options());
    let valid = |text: &str, interfaces: &[IpAddr]| {
        let mut address = text.to_owned();
        (
            rules.is_valid_peer(&mut address, false, interfaces),
            address,
        )
    };
    assert_eq!(valid("127.11.0.12", &[]), (true, "127.11.0.12".to_owned()));
    assert_eq!(
        valid("::ffff:127.11.0.13", &[]),
        (true, "127.11.0.13".to_owned())
    );
    for refused in [
        "127.11.0.11",
        "127.11.0.111",
        "8.8.8.8",
        "not an address",
        "::1",
    ] {
        assert!(!valid(refused, &[]).0, "{refused}");
    }
    assert!(
        !valid("127.0.0.1", &[ip("127.0.0.1")]).0,
        "the host's own address"
    );
    let reference = Rules::from_options(&P2pOptions::default());
    let mut loopback = "127.11.0.12".to_owned();
    assert!(!reference.is_valid_peer(&mut loopback, false, &[]));
    let mut public = "8.8.8.8".to_owned();
    assert!(reference.is_valid_peer(&mut public, false, &[]));
}

#[test]
fn the_guards_hold_a_node_to_a_loopback_devnet() {
    assert_eq!(guard(&node_options(), "devnet"), Ok(()));
    assert_eq!(guard(&node_options(), "devnet-f"), Ok(()));
    let refused = |options: P2pOptions, network: &str| guard(&options, network).is_err();
    assert!(refused(node_options(), "testnet"));
    assert!(refused(node_options(), "mydevnet"));
    let elsewhere = P2pOptions {
        host: ip("10.0.0.11"),
        addressing: alias("10.0.0.11"),
        ..node_options()
    };
    assert!(refused(elsewhere, "devnet"));
    let wildcard = P2pOptions {
        host: ip("0.0.0.0"),
        ..node_options()
    };
    assert!(refused(wildcard, "devnet"));
    for remote_access in [vec![], vec!["127.11.0.11", "10.0.0.0/8"], vec!["192.0.2.1"]] {
        let remote_access = remote_access.into_iter().map(str::to_owned).collect();
        let options = P2pOptions {
            remote_access,
            ..node_options()
        };
        assert!(refused(options, "devnet"));
    }
    let off = P2pOptions {
        remote_access: Vec::new(),
        ..P2pOptions::default()
    };
    assert_eq!(guard(&off, "testnet"), Ok(()));
}

/// A listener on `at` that answers each connection with `answer` and reports its source.
async fn recorder(
    at: &str,
    answer: &'static [u8],
) -> (SocketAddr, tokio::sync::mpsc::Receiver<IpAddr>) {
    let listener = TcpListener::bind((ip(at), 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sources, received) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        while let Ok((mut stream, source)) = listener.accept().await {
            let _ = sources.send(source.ip()).await;
            let _ = stream.read(&mut [0; 1024]).await;
            let _ = stream.write_all(answer).await;
        }
    });
    (address, received)
}

#[tokio::test]
async fn the_port_probe_binds_the_node_address_in_mode_b() {
    let (address, mut sources) = recorder("127.11.0.21", b"HTTP/1.1 200 OK\r\n\r\n").await;
    let probe = HttpProbe {
        bind: alias("127.11.0.22").bind_address(),
        ..HttpProbe::default()
    };
    assert_eq!(probe.head(address.ip(), address.port()).await, Some(200));
    assert_eq!(sources.recv().await, Some(ip("127.11.0.22")));
    assert_eq!(
        HttpProbe::default()
            .head(address.ip(), address.port())
            .await,
        Some(200)
    );
    assert_eq!(sources.recv().await, Some(ip("127.0.0.1")));
}

/// A server on 127.11.0.31 over the fakes, serving over TCP until the sender turns true.
async fn served(options: ServerOptions) -> (Arc<P2pServer>, Fakes, watch::Sender<bool>) {
    let fakes = Fakes::new();
    let parts = ServerParts {
        options,
        node: fakes.node.clone(),
        peers: fakes.peers.clone(),
        handlers: fakes.handlers.clone(),
        events: fakes.events.clone(),
    };
    let server = Arc::new(
        P2pServer::bind_with_clock(parts, fakes.clock.clone())
            .await
            .unwrap(),
    );
    let (stop, shutdown) = watch::channel(false);
    let serving = Arc::clone(&server);
    tokio::spawn(async move { serving.serve(shutdown).await });
    (server, fakes, stop)
}

#[tokio::test]
async fn the_connector_binds_the_node_address_and_the_server_sees_it() {
    let options = ServerOptions {
        host: ip("127.11.0.31"),
        ..options()
    };
    let (server, fakes, stop) = served(options).await;
    let connector = Connector::for_addressing(&alias("127.11.0.32"));
    let answer = connector.emit(
        server.local_addr(),
        RouteId::GetStatus,
        status_request(VERSION),
        Duration::from_secs(5),
        0,
    );
    assert_eq!(answer.await, Ok(Vec::new()));
    let requests = fakes.handlers.requests();
    assert_eq!(requests[0].1.ip, ip("127.11.0.32"));
    let _ = stop.send(true);
}

#[tokio::test]
async fn the_forger_sessions_close_first_and_the_server_stops_with_every_socket() {
    let options = ServerOptions {
        host: ip("127.11.0.41"),
        remote_access: vec![ip("127.11.0.42")],
        ..options()
    };
    let (server, fakes, stop) = served(options).await;
    let (forger, peer) = (alias("127.11.0.42"), alias("127.11.0.43"));
    let forger = Connector::for_addressing(&forger);
    let peer = Connector::for_addressing(&peer);
    let address = server.local_addr();
    let wait = Duration::from_secs(5);
    let internal = forger.emit(address, RouteId::GetSlotNumber, b"{}".to_vec(), wait, 0);
    assert_eq!(internal.await, Ok(Vec::new()));
    let public = peer.emit(
        address,
        RouteId::GetStatus,
        status_request(VERSION),
        wait,
        0,
    );
    assert_eq!(public.await, Ok(Vec::new()));
    server.close_forger_sessions();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let closed = |events: &[(_, IpAddr, bool)], at: &str| {
        events
            .iter()
            .any(|(_, source, open)| !open && *source == ip(at))
    };
    let events = fakes.events.events();
    assert!(closed(&events, "127.11.0.42") && !closed(&events, "127.11.0.43"));
    let _ = stop.send(true);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(closed(&fakes.events.events(), "127.11.0.43"));
    // The listener is closed.
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}
