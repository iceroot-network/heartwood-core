//! Host selection and failover, as the reference's `selectHost` does them: up to 10 sweeps 250 ms
//! apart, the first ready host in list order, a new session for each host passed on the way whose
//! session is not ready, the hosts after the chosen one left alone, the selection reused by the
//! calls that do not select, and the return to the first host once it is back. A relay that stops
//! answering without closing is left through the calls' deadlines and the session's heartbeat, and
//! the attestation routes follow the selection.
//!
//! Each relay's connections pass a switch that lets them through, refuses them, or leaves them
//! hanging before the WebSocket upgrade, as a frozen relay's listener does.

#[path = "support/mock_relay.rs"]
mod mock_relay;

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_forger::client::Client;
use heartwood_forger::seams::{AttestLink, Deadline, RelayLink};
use heartwood_p2p::testing::{MockPeer, Reply, Script};
use heartwood_p2p::transport::{Dial, Stream};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::internal::{SlotNumber, StatusToSignResponse};
use mock_relay::*;
use tokio::io::DuplexStream;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Up,
    Refuse,
    Hang,
}

/// What each host's connections meet, and how many were dialled.
#[derive(Default)]
struct Switch {
    modes: Mutex<HashMap<IpAddr, Mode>>,
    dials: Mutex<HashMap<IpAddr, usize>>,
    hung: Mutex<Vec<DuplexStream>>,
}

impl Switch {
    fn set(&self, n: u8, mode: Mode) {
        self.modes.lock().unwrap().insert(ip(n), mode);
    }

    fn dials(&self, n: u8) -> usize {
        self.dials.lock().unwrap().get(&ip(n)).copied().unwrap_or(0)
    }
}

/// A client of `hosts` whose connections pass `switch` to `relays`.
fn switched(hosts: &[u8], relays: &[&MockPeer], switch: &Arc<Switch>) -> Client {
    let (inner, switch) = (MockPeer::dial(relays), switch.clone());
    let dial: Dial = Arc::new(move |address| {
        *switch
            .dials
            .lock()
            .unwrap()
            .entry(address.ip())
            .or_default() += 1;
        let mode = switch
            .modes
            .lock()
            .unwrap()
            .get(&address.ip())
            .copied()
            .unwrap_or(Mode::Up);
        match mode {
            Mode::Up => inner(address),
            Mode::Refuse => Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) }),
            Mode::Hang => {
                let (client, server) = tokio::io::duplex(64 * 1024);
                switch.hung.lock().unwrap().push(server);
                Box::pin(async move { Ok(Box::new(client) as Box<dyn Stream>) })
            }
        }
    });
    let hosts: Vec<_> = hosts.iter().map(|n| host(*n)).collect();
    client_over(&hosts, dial)
}

fn rounds(count: usize) -> Script {
    let mut script = Script::new();
    for _ in 0..count {
        script = script.on(
            RouteId::GetCurrentRound,
            json(&round(8, 1, &[1], &[], Some(0), None)),
        );
    }
    script
}

#[tokio::test(start_paused = true)]
async fn a_selection_takes_the_first_ready_host_and_renews_the_sessions_before_it() {
    let switch = Arc::new(Switch::default());
    switch.set(1, Mode::Refuse);
    let (second, third) = (relay(2, Script::new()).await, relay(3, Script::new()).await);
    let client = switched(&[1, 2, 3], &[&second, &third], &switch);
    assert_eq!(
        client.selected(),
        host(1),
        "the first host before any selection"
    );

    let start = Instant::now();
    assert_eq!(client.select_host().await, Ok(host(2)));
    assert_eq!(
        start.elapsed(),
        Duration::from_millis(250),
        "the second sweep found it ready"
    );
    assert_eq!(client.selected(), host(2));
    sleep_ms(1).await;
    // The first sweep opened a session to each host; the second replaced the first host's only.
    assert_eq!(
        (switch.dials(1), switch.dials(2), switch.dials(3)),
        (2, 1, 1)
    );

    let start = Instant::now();
    assert_eq!(client.select_host().await, Ok(host(2)));
    assert_eq!(start.elapsed(), Duration::ZERO, "ready at once");
    sleep_ms(1).await;
    assert_eq!(
        (switch.dials(1), switch.dials(2), switch.dials(3)),
        (3, 1, 1),
        "the host after it is left alone"
    );
    assert_eq!((second.connections(), third.connections()), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn no_ready_host_after_ten_sweeps_is_no_response() {
    let (lines, _guard) = Lines::capture();
    let switch = Arc::new(Switch::default());
    switch.set(1, Mode::Refuse);
    switch.set(2, Mode::Hang);
    let client = switched(&[1, 2], &[], &switch);
    let start = Instant::now();
    let error = client.get_round().await.unwrap_err();
    assert_eq!(start.elapsed(), Duration::from_millis(2_500));
    assert_eq!(
        error.to_string(),
        "127.10.0.1,127.10.0.2 didn't respond. Trying again later :sparkler:"
    );
    assert_eq!((switch.dials(1), switch.dials(2)), (10, 10));
    assert_eq!(lines.with("No open socket").len(), 1);
    assert!(
        lines.with("Connection terminated").is_empty(),
        "the reference does not log it"
    );
}

#[tokio::test(start_paused = true)]
async fn the_other_calls_reuse_the_selection_and_the_first_host_is_taken_back() {
    let switch = Arc::new(Switch::default());
    switch.set(1, Mode::Refuse);
    let first = relay(1, rounds(1).on(RouteId::GetNetworkState, json(&state(2)))).await;
    let second = relay(2, rounds(2).on(RouteId::GetNetworkState, json(&state(1)))).await;
    let client = switched(&[1, 2], &[&first, &second], &switch);
    let deadline = || Deadline::after(Duration::from_secs(4));

    client.get_round().await.unwrap();
    assert_eq!(client.selected(), host(2));
    switch.set(1, Mode::Up);
    assert_eq!(
        client.get_network_state(true, deadline()).await,
        Ok(state(1)),
        "the second host's"
    );
    // This selection renews the first host's session, which is not open yet when the second
    // host is found ready.
    client.get_round().await.unwrap();
    assert_eq!(client.selected(), host(2));
    sleep_ms(1).await;
    client.get_round().await.unwrap();
    assert_eq!(
        client.selected(),
        host(1),
        "back to the first host once it is ready"
    );
    assert_eq!(
        client.get_network_state(true, deadline()).await,
        Ok(state(2))
    );
    let routes = |relay: &MockPeer| {
        relay
            .requests()
            .into_iter()
            .map(|(route, _)| route)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        routes(&first),
        [RouteId::GetCurrentRound, RouteId::GetNetworkState]
    );
    assert_eq!(
        routes(&second),
        [
            RouteId::GetCurrentRound,
            RouteId::GetNetworkState,
            RouteId::GetCurrentRound
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_relay_that_stops_answering_is_left_through_the_deadlines_and_the_heartbeat() {
    let (lines, _guard) = Lines::capture();
    let switch = Arc::new(Switch::default());
    // The first relay answers one round, then nothing: no answer, no ping, and no new connection
    // gets past the upgrade.
    let frozen = relay(1, rounds(1)).await;
    // The second relay pings every 5 s, so its idle session lives on.
    let pinging = Script {
        pings: vec![Duration::from_secs(5); 10],
        ..rounds(1).on(RouteId::GetSlotNumber, json(&SlotNumber(3)))
    };
    let second = relay(2, pinging).await;
    let client = switched(&[1, 2], &[&frozen, &second], &switch);
    client.get_round().await.unwrap();
    assert_eq!(client.selected(), host(1));
    let answered = Instant::now();
    switch.set(1, Mode::Hang);

    let start = Instant::now();
    let error = client
        .get_network_state(true, Deadline::after(Duration::from_secs(4)))
        .await;
    assert!(
        error
            .unwrap_err()
            .to_string()
            .contains("'Request timed out'")
    );
    assert_eq!(start.elapsed(), Duration::from_secs(4), "its deadline");
    // The round's 20 s deadline is later than the heartbeat's: 15 s and 5 s after the last
    // message the session gives the relay up.
    let error = client.get_round().await.unwrap_err();
    assert_eq!(answered.elapsed(), Duration::from_secs(20));
    assert!(
        error
            .to_string()
            .contains("'Request failed - server disconnected'"),
        "{error}"
    );
    assert_eq!(
        lines.with("heartbeat"),
        ["ERROR: Disconnecting due to heartbeat timeout"]
    );
    // The session reconnects into the hanging listener; the next selection replaces it and takes
    // the second relay.
    let start = Instant::now();
    client.get_round().await.unwrap();
    assert_eq!(
        (client.selected(), start.elapsed()),
        (host(2), Duration::ZERO)
    );
    let slot = client
        .get_slot_number(None, Deadline::after(Duration::from_secs(2)))
        .await;
    assert_eq!(slot, Ok(3));
}

#[tokio::test(start_paused = true)]
async fn a_hung_host_that_is_not_selected_reconnects_without_a_log_line() {
    let (lines, _guard) = Lines::capture();
    let switch = Arc::new(Switch::default());
    switch.set(2, Mode::Hang);
    let first = relay(1, rounds(1)).await;
    let client = switched(&[1, 2], &[&first], &switch);
    client.get_round().await.unwrap();
    assert_eq!(client.selected(), host(1));
    sleep_ms(30_000).await;
    // The session to the second host gives up each attempt after 5 s and tries again; the
    // reference's forger sets no such timeout, so it logs nothing for a host that hangs.
    assert!(switch.dials(2) > 2, "{} dials", switch.dials(2));
    assert!(lines.with("timed out").is_empty(), "{:?}", lines.all());
}

#[tokio::test(start_paused = true)]
async fn the_attestation_routes_follow_the_selection() {
    let switch = Arc::new(Switch::default());
    let offer = StatusToSignResponse::Offer {
        status: "{}".into(),
        public_keys: vec![hex(1)],
    };
    let first = rounds(1)
        .on(
            RouteId::GetStatusToSign,
            json(&StatusToSignResponse::Unchanged),
        )
        .on(RouteId::GetStatusToSign, Reply::Close);
    let first = relay(1, first).await;
    let second = relay(2, rounds(1).on(RouteId::GetStatusToSign, json(&offer))).await;
    let client = switched(&[1, 2], &[&first, &second], &switch);
    client.get_round().await.unwrap();
    assert_eq!(
        client.status_to_sign(&[hex(1)]).await,
        Ok(StatusToSignResponse::Unchanged)
    );
    // The first relay goes away while the attester polls it.
    switch.set(1, Mode::Refuse);
    let error = client.status_to_sign(&[hex(1)]).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("127.10.0.1:6002<p2p.internal.getStatusToSign>")
    );
    client.get_round().await.unwrap();
    assert_eq!(client.selected(), host(2));
    assert_eq!(
        client.status_to_sign(&[hex(1)]).await,
        Ok(offer),
        "polled at the second relay"
    );
    let polls = |relay: &MockPeer| {
        relay
            .requests()
            .iter()
            .filter(|(r, _)| *r == RouteId::GetStatusToSign)
            .count()
    };
    assert_eq!((polls(&first), polls(&second)), (2, 1));
}
