//! The relay client over a scripted relay: each call's route, codec, headers and deadline, the
//! reference's error texts, `emitEvent`'s local host rule, the 20 MiB receive limit that stays in
//! force between answers, and the close of the sessions.
//!
//! The relays answer over in-memory connections on tokio's paused clock, so the deadlines are
//! measured exactly.

#[path = "support/mock_relay.rs"]
mod mock_relay;

use std::time::Duration;

use heartwood_forger::errors::RelayError;
use heartwood_forger::seams::{AttestLink, Deadline, ForgerEvent, RelayLink};
use heartwood_p2p::testing::{Reply, Script};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::{blocks, transactions};
use heartwood_p2p_wire::internal::{
    self, NetworkState, SlotNumber, StatusSignatures, StatusToSignResponse,
};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::peer::GetStatusRequest;
use mock_relay::*;
use serde_json::json;
use tokio::time::Instant;

/// The forger's headers, `{version, port: 0}`, as the relay reads them.
fn assert_headers(headers: Option<&heartwood_p2p_wire::messages::shared::Headers>) {
    let headers = headers.expect("headers");
    assert_eq!(headers.version.as_deref(), Some(VERSION));
    assert_eq!(headers.port, Some(0));
}

#[tokio::test(start_paused = true)]
async fn each_call_has_its_route_codec_and_headers() {
    let offer = StatusToSignResponse::Offer {
        status: "{}".into(),
        public_keys: vec![hex(1)],
        challenge: "0c".repeat(32),
    };
    let script = Script::new()
        .on(
            RouteId::GetCurrentRound,
            json(&round(72, 9, &[1, 2], &[3], Some(0), Some(1))),
        )
        .on(RouteId::GetSlotNumber, json(&SlotNumber(9)))
        .on(RouteId::GetSlotNumber, json(&SlotNumber(10)))
        .on(RouteId::GetNetworkState, json(&state(9)))
        .on(RouteId::GetStatus, status_reply(9, 64, &hex(2)))
        .on(
            RouteId::GetUnconfirmedTransactions,
            pool_reply(4, vec![vec![1, 2], vec![3]]),
        )
        .on(RouteId::PostBlock, posted_reply(10))
        .on(RouteId::SyncBlockchain, text("true"))
        .on(RouteId::EmitEvent, text("true"))
        .on(RouteId::GetStatusToSign, json(&offer))
        .on(RouteId::PostStatusSignatures, text("true"));
    let relay = relay(1, script).await;
    // The second host is never reached: it only gives the events a host whose text is local.
    let local = heartwood_forger::options::RelayHost::new("127.0.0.1", 4002).unwrap();
    let client = client(&[host(1), local], &[&relay]);
    let deadline = || Deadline::after(Duration::from_secs(4));

    let round = client.get_round().await.unwrap();
    assert_eq!((round.timestamp, round.last_block.height), (72, 9));
    assert_eq!(client.get_slot_number(Some(72), deadline()).await, Ok(9));
    assert_eq!(client.get_slot_number(None, deadline()).await, Ok(10));
    assert_eq!(
        client.get_network_state(true, deadline()).await,
        Ok(state(9))
    );
    let status = client.get_status(deadline()).await.unwrap();
    let header = status.message.state.unwrap().header.unwrap();
    assert_eq!(
        (header.timestamp, header.generator_public_key),
        (Some(64), Some(hex(2)))
    );
    let pool = client.get_transactions(deadline()).await.unwrap().unwrap();
    assert_eq!(
        (pool.pool_size, pool.transactions),
        (4, vec![vec![1, 2], vec![3]])
    );
    client.broadcast_block(&[0xab, 0xcd], deadline()).await;
    assert_eq!(client.sync_with_network().await, Ok(()));
    let failed = ForgerEvent::Failed { error: "x".into() };
    client.emit_event(failed).await;
    assert_eq!(client.status_to_sign(&[hex(1), hex(1)]).await, Ok(offer));
    let signatures = StatusSignatures {
        offer: "ff".into(),
        signatures: vec!["00".into()],
        proofs: vec!["01".into()],
    };
    assert_eq!(client.post_signatures(signatures).await, Ok(()));

    let requests = relay.requests();
    let payload = |route: RouteId, nth: usize| {
        let mut sent = requests.iter().filter(|(r, _)| *r == route).map(|(_, p)| p);
        String::from_utf8_lossy(sent.nth(nth).expect("a request")).into_owned()
    };
    assert_eq!(payload(RouteId::GetCurrentRound, 0), "{}");
    assert_eq!(payload(RouteId::GetSlotNumber, 0), r#"{"timestamp":72}"#);
    assert_eq!(payload(RouteId::GetSlotNumber, 1), "{}");
    assert_eq!(payload(RouteId::GetNetworkState, 0), r#"{"log":true}"#);
    assert_eq!(payload(RouteId::SyncBlockchain, 0), "{}");
    assert_eq!(
        payload(RouteId::EmitEvent, 0),
        r#"{"event":"forger.failed","body":{"error":"x"}}"#
    );
    let keys = format!(r#"{{"publicKeys":["{0}","{0}"]}}"#, hex(1));
    assert_eq!(payload(RouteId::GetStatusToSign, 0), keys);
    assert_eq!(
        payload(RouteId::PostStatusSignatures, 0),
        r#"{"offer":"ff","signatures":["00"],"proofs":["01"]}"#
    );
    let sent = |route| {
        requests
            .iter()
            .find(|(r, _)| *r == route)
            .unwrap()
            .1
            .clone()
    };
    assert_headers(
        GetStatusRequest::decode(&sent(RouteId::GetStatus))
            .unwrap()
            .headers
            .as_ref(),
    );
    let pool = transactions::get_unconfirmed_transactions_decode_request(&sent(
        RouteId::GetUnconfirmedTransactions,
    ))
    .unwrap();
    assert_headers(pool.request.headers.as_ref());
    assert!(pool.exclude.is_empty() && pool.request.exclude == Some(Vec::new()));
    let post = blocks::post_block_decode_request(&sent(RouteId::PostBlock)).unwrap();
    assert_headers(post.headers.as_ref());
    assert_eq!(post.block, Some(vec![0xab, 0xcd]));
    assert_eq!(
        relay.connections(),
        1,
        "every call on the selected host's one session"
    );
}

#[tokio::test(start_paused = true)]
async fn failed_calls_carry_the_reference_texts() {
    let (lines, _guard) = Lines::capture();
    let script = Script::new()
        .on(RouteId::GetCurrentRound, not_ready())
        .on(RouteId::GetSlotNumber, text("{nope"))
        .on(RouteId::GetNetworkState, text(r#"{"status":7}"#))
        .on(
            RouteId::PostBlock,
            Reply::Error {
                status: 400,
                payload: b"bad".to_vec(),
            },
        )
        .on(RouteId::SyncBlockchain, text("oops"));
    let relay = relay(1, script).await;
    let client = client(&[host(1)], &[&relay]);
    let deadline = || Deadline::after(Duration::from_secs(4));
    let endpoint = |route: RouteId| format!("127.10.0.1:6002<{}>", route.id());

    let not_ready = client.get_round().await.unwrap_err();
    assert!(not_ready.is_app_not_ready());
    let expected = format!(
        r#"Request to {} failed, because of '{{"error":"Unknown","message":"App is not ready"}}' :bangbang:"#,
        endpoint(RouteId::GetCurrentRound)
    );
    assert_eq!(not_ready.to_string(), expected);
    let RelayError::Communication {
        endpoint: at,
        message,
    } = client.get_slot_number(None, deadline()).await.unwrap_err()
    else {
        panic!("a communication error");
    };
    assert_eq!(at, endpoint(RouteId::GetSlotNumber));
    assert!(
        message.starts_with("an internal body is not JSON text"),
        "{message}"
    );
    let refused = client
        .get_network_state(false, deadline())
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("status"), "{refused}");

    client.broadcast_block(&[1], deadline()).await;
    assert_eq!(
        client.sync_with_network().await,
        Ok(()),
        "a failed call is logged"
    );
    let failed = format!(
        "ERROR: Broadcast block failed: Request to {} failed, because of 'bad'",
        endpoint(RouteId::PostBlock)
    );
    assert_eq!(lines.with("Broadcast block failed"), [failed]);
    assert_eq!(
        lines.with("wake-up"),
        ["DEBUG: Sending wake-up check to relay node 127.10.0.1"]
    );
    let sync = lines.with("Could not sync check");
    assert_eq!(sync.len(), 1);
    assert!(sync[0].starts_with(&format!(
        "ERROR: Could not sync check: Request to {}",
        endpoint(RouteId::SyncBlockchain)
    )));

    let nobody = client_over(
        &[host(2), host(3)],
        heartwood_p2p::testing::MockPeer::dial(&[]),
    );
    let error = nobody.get_round().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "127.10.0.2,127.10.0.3 didn't respond. Trying again later :sparkler:"
    );
    let line =
        r#"DEBUG: No open socket connection to any host: ["127.10.0.2:6002","127.10.0.3:6002"]"#;
    assert_eq!(lines.with("No open socket"), [line]);
    // A sync whose selection finds no host returns the selection's error to the slot check.
    assert_eq!(nobody.sync_with_network().await, Err(error));
    assert_eq!(lines.with("Could not sync check").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn every_call_gives_up_at_its_deadline() {
    let (lines, _guard) = Lines::capture();
    // The relay pings every 5 s, so the session outlives the calls it leaves unanswered.
    let script = Script {
        pings: vec![Duration::from_secs(5); 40],
        ..Script::new().on(RouteId::GetSlotNumber, json(&SlotNumber(1)))
    };
    let relay = relay(1, script).await;
    let local = heartwood_forger::options::RelayHost::new("127.0.0.1", 4002).unwrap();
    let client = client(&[host(1), local], &[&relay]);
    client
        .get_slot_number(None, Deadline::after(Duration::from_secs(1)))
        .await
        .unwrap();
    let timed_out = |error: RelayError| error.to_string().contains("'Request timed out'");

    let start = Instant::now();
    assert!(timed_out(client.get_round().await.unwrap_err()));
    assert_eq!(start.elapsed(), Duration::from_secs(20), "getCurrentRound");
    for (route, seconds) in [
        (RouteId::GetNetworkState, 4),
        (RouteId::GetStatus, 2),
        (RouteId::GetUnconfirmedTransactions, 9),
    ] {
        let start = Instant::now();
        let deadline = Deadline::after(Duration::from_secs(seconds));
        let error = match route {
            RouteId::GetNetworkState => client.get_network_state(true, deadline).await.unwrap_err(),
            RouteId::GetStatus => client.get_status(deadline).await.unwrap_err(),
            _ => client.get_transactions(deadline).await.unwrap_err(),
        };
        assert!(timed_out(error));
        assert_eq!(start.elapsed(), Duration::from_secs(seconds), "{route:?}");
    }
    let start = Instant::now();
    client
        .broadcast_block(&[1], Deadline::after(Duration::from_secs(5)))
        .await;
    assert_eq!(start.elapsed(), Duration::from_secs(5), "postBlock");
    let start = Instant::now();
    assert_eq!(client.sync_with_network().await, Ok(()));
    assert_eq!(start.elapsed(), Duration::from_secs(20), "syncBlockchain");
    let start = Instant::now();
    client
        .emit_event(ForgerEvent::Failed { error: "x".into() })
        .await;
    assert_eq!(start.elapsed(), Duration::from_secs(20), "emitEvent");
    let start = Instant::now();
    assert!(timed_out(client.status_to_sign(&[]).await.unwrap_err()));
    assert_eq!(start.elapsed(), Duration::from_secs(15), "getStatusToSign");
    let start = Instant::now();
    let signatures = StatusSignatures {
        offer: String::new(),
        signatures: Vec::new(),
        proofs: Vec::new(),
    };
    assert!(timed_out(
        client.post_signatures(signatures).await.unwrap_err()
    ));
    assert_eq!(
        start.elapsed(),
        Duration::from_secs(2),
        "postStatusSignatures"
    );
    let expired = Deadline::after(Duration::ZERO);
    assert!(
        timed_out(client.get_status(expired).await.unwrap_err()),
        "a deadline already past"
    );
    assert_eq!(lines.with("Failed to emit").len(), 1);
    assert_eq!(lines.with("Broadcast block failed").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn events_need_a_host_whose_text_names_the_loopback_but_go_to_the_selected_host() {
    let (lines, _guard) = Lines::capture();
    let script = Script::new()
        .on(
            RouteId::GetCurrentRound,
            json(&round(8, 1, &[1], &[], Some(0), None)),
        )
        .on(RouteId::EmitEvent, text("true"))
        .on(RouteId::EmitEvent, not_ready());
    let relay = relay(1, script).await;
    let local = heartwood_forger::options::RelayHost::new("127.0.0.11", 4002).unwrap();
    let client = client(&[host(1), local], &[&relay]);
    client.get_round().await.unwrap();
    client
        .emit_event(ForgerEvent::Failed { error: "a".into() })
        .await;
    client
        .emit_event(ForgerEvent::Failed { error: "b".into() })
        .await;
    let sent = relay
        .requests()
        .into_iter()
        .filter(|(route, _)| *route == RouteId::EmitEvent)
        .count();
    assert_eq!(
        sent, 2,
        "127.0.0.11 contains 127.0.0.1; both went to the selected relay"
    );
    assert_eq!(
        lines.with("Failed to emit"),
        [r#"ERROR: Failed to emit "forger.failed" to "127.0.0.11:4002""#]
    );

    let remote = heartwood_forger::options::RelayHost::new("127.1.0.11", 4002).unwrap();
    let others = client_over(
        &[host(1), remote],
        heartwood_p2p::testing::MockPeer::dial(&[&relay]),
    );
    let sending = others.emit_event(ForgerEvent::Started {
        active_delegates: Vec::new(),
    });
    assert_eq!(
        lines.with("emitEvent"),
        ["ERROR: emitEvent: unable to find any local hosts"],
        "at the call"
    );
    sending.await;
    assert_eq!(
        relay
            .requests()
            .iter()
            .filter(|(route, _)| *route == RouteId::EmitEvent)
            .count(),
        2
    );
    let ipv6 = heartwood_forger::options::RelayHost::new("::1", 4002).unwrap();
    let loopback = client_over(&[ipv6], heartwood_p2p::testing::MockPeer::dial(&[]));
    loopback
        .emit_event(ForgerEvent::Failed { error: "c".into() })
        .await;
    assert_eq!(lines.with("emitEvent").len(), 1, "::1 is a local host");
    // The rule reads the address as configured, not as the address it names is written.
    for spelled in ["::ffff:7f00:1", "0:0:0:0:0:0:0:1"] {
        let host = heartwood_forger::options::RelayHost::new(spelled, 4002).unwrap();
        let other = client_over(&[host], heartwood_p2p::testing::MockPeer::dial(&[]));
        other
            .emit_event(ForgerEvent::Failed { error: "d".into() })
            .await;
    }
    assert_eq!(lines.with("emitEvent").len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_large_answer_after_another_answer_is_read() {
    let big: Vec<u32> = (0..60).collect();
    let registered: Vec<u32> = (100..1_000).collect();
    let large = round(8, 1, &big, &registered, Some(0), Some(1));
    let size = internal::encode(&large).len();
    assert!(
        size > 100 * 1024,
        "the round is {size} bytes, above the default 100 KiB"
    );
    let script = Script::new()
        .on(
            RouteId::GetStatusToSign,
            Reply::After(
                Duration::from_millis(100),
                Box::new(json(&StatusToSignResponse::Unchanged)),
            ),
        )
        .on(
            RouteId::GetCurrentRound,
            Reply::After(Duration::from_millis(200), Box::new(json(&large))),
        );
    let relay = relay(1, script).await;
    let client = client(&[host(1)], &[&relay]);
    client.select_host().await.unwrap();
    let (poll, round) = tokio::join!(client.status_to_sign(&[]), client.get_round());
    assert_eq!(poll, Ok(StatusToSignResponse::Unchanged));
    assert_eq!(round.unwrap().all_delegates.len(), 960);
}

#[tokio::test(start_paused = true)]
async fn dispose_closes_every_session() {
    let script = Script::new().on(
        RouteId::GetCurrentRound,
        json(&round(8, 1, &[1], &[], Some(0), None)),
    );
    let relay = relay(1, script).await;
    let client = client(&[host(1)], &[&relay]);
    client.get_round().await.unwrap();
    client.dispose();
    sleep_ms(1).await;
    let error = client
        .get_network_state(true, Deadline::after(Duration::from_secs(4)))
        .await;
    assert!(
        error
            .unwrap_err()
            .to_string()
            .contains("Failed to send message - server disconnected")
    );
    assert!(
        client.select_host().await.is_err(),
        "no new session after the dispose"
    );
    assert_eq!(relay.connections(), 1);
    let parsed: NetworkState =
        internal::decode(json!({"status": 3}).to_string().as_bytes()).unwrap();
    assert!(!parsed.can_forge(53));
}

#[tokio::test(start_paused = true)]
async fn the_attestation_routes_go_to_a_relay_on_the_forgers_host_only() {
    let offer = StatusToSignResponse::Offer {
        status: "{}".into(),
        public_keys: vec![hex(1)],
        challenge: "0c".repeat(32),
    };
    let script = Script::new()
        .on(
            RouteId::GetCurrentRound,
            json(&round(72, 9, &[1], &[], Some(0), Some(0))),
        )
        .on(RouteId::GetStatusToSign, json(&offer))
        .on(RouteId::PostStatusSignatures, text("true"));
    let relay = relay(1, script).await;
    // A relay configured by an address that is not a loopback one, which the relay's own rule
    // would refuse as a source: the forger asks it for no status to sign and posts it nothing.
    let inner = heartwood_p2p::testing::MockPeer::dial(&[&relay]);
    let dial: heartwood_p2p::transport::Dial =
        std::sync::Arc::new(move |address| inner(std::net::SocketAddr::new(ip(1), address.port())));
    let remote = heartwood_forger::options::RelayHost::new("192.0.2.1", PORT).unwrap();
    let client = client_over(&[remote], dial);
    assert!(client.get_round().await.is_ok());
    let refused = client.status_to_sign(&[hex(1)]).await.unwrap_err();
    assert!(
        refused.to_string().contains("on the forger's host only"),
        "{refused}"
    );
    let signatures = StatusSignatures {
        offer: "ff".into(),
        signatures: vec!["00".into()],
        proofs: vec!["01".into()],
    };
    assert!(client.post_signatures(signatures).await.is_err());
    let routes: Vec<RouteId> = relay
        .requests()
        .into_iter()
        .map(|(route, _)| route)
        .collect();
    assert_eq!(routes, [RouteId::GetCurrentRound]);
}
