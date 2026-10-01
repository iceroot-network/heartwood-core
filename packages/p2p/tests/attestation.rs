//! The forger's attestation session and the served status, over fakes of the node's services with
//! tokio's paused clock (the wall clock follows it): the session's rules (a loopback source only,
//! one session, its replacement, the long poll's 10 s, every signature verified against the offer
//! it names, stale signatures dropped, the end after the socket closes or 15 s without a poll, the
//! attested set), the status cache and the 500 ms wait, and the offers the status's timer makes at
//! a new tip, the slot boundary and the half-slot flip without a getStatus. The served status's
//! bytes are also compared with the reference's in class V31 (`status.serve`), read from
//! `tests/vectors` of the crate, which must hold it (`make vectors-install`), or from the directory
//! in `P2P_VECTORS`, which may lack it (the test then says so and passes).

#![cfg(feature = "node")]

mod node_fakes;
mod peer_fakes;

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, KeyPair, PublicKeyBytes, SecretKey};
use heartwood_kernel::ManualClock;
use heartwood_p2p::attestation_session::DelegatesOnThisNode;
use heartwood_p2p::node_services::AttestedKeys;
use heartwood_p2p::server::seams::{ServerRefusal, SessionEvents, SessionId};
use heartwood_p2p::status_server::StatusServer;
use heartwood_p2p_wire::internal::{
    self, StatusSignatures, StatusToSignRequest, StatusToSignResponse,
};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::peer::GetStatusResponse;
use heartwood_p2p_wire::status::{
    NetworkConfig, PeerConfig, PluginConfig, TokenConfig, attest, parse_preimage, session_proof,
};
use node_fakes::{Node, Server, TokioClock, forger};
use peer_fakes::{Chain, DEVNET, START_MS, block, delegate};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::{Instant, advance, sleep};

/// The forger's keys, in file order: delegates 1 and 2, delegate 1 again, and a key that is no
/// delegate.
fn forger_keys() -> Vec<KeyPair> {
    let stranger = KeyPair::from_passphrase("not a delegate").unwrap();
    vec![delegate(1), delegate(2), delegate(1), stranger]
}

fn declared() -> StatusToSignRequest {
    StatusToSignRequest {
        public_keys: forger_keys()
            .iter()
            .map(|k| k.public_key().to_compressed().to_hex())
            .collect(),
    }
}

fn key(i: usize) -> PublicKeyBytes {
    delegate(i).public_key().to_compressed()
}

fn tokio_server() -> Server {
    Server::new(Node::at(10), Arc::new(TokioClock::at(START_MS)))
}

/// The forger's signatures of `status` with the keys offered, in their order, and their proofs
/// of the session's `challenge`.
fn sign(status: &str, keys: &[String], challenge: &str) -> StatusSignatures {
    let keys: Vec<KeyPair> = keys
        .iter()
        .map(|hex| {
            forger_keys()
                .into_iter()
                .find(|k| k.public_key().to_compressed().to_hex() == *hex)
                .unwrap()
        })
        .collect();
    let proof = session_proof(challenge).unwrap();
    let signed = |text: &[u8]| -> Vec<String> {
        (keys.iter())
            .map(|k| attest(text, k).unwrap().signature.to_hex())
            .collect()
    };
    StatusSignatures {
        offer: hex::encode(&sha256(status.as_bytes())),
        signatures: signed(status.as_bytes()),
        proofs: signed(&proof),
    }
}

/// An offer's text, keys and challenge.
fn offered(answer: StatusToSignResponse) -> (String, Vec<String>, String) {
    match answer {
        StatusToSignResponse::Offer {
            status,
            public_keys,
            challenge,
        } => (status, public_keys, challenge),
        StatusToSignResponse::Unchanged => panic!("no offer"),
    }
}

fn session(n: u64) -> SessionId {
    SessionId(n)
}

// ---- the session ------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn only_a_loopback_source_gets_a_session() {
    let server = tokio_server();
    let remote = IpAddr::from([192, 0, 2, 7]);
    let refused = server
        .session
        .status_to_sign(session(1), remote, declared())
        .await;
    assert_eq!(refused.unwrap_err().status, 499);
    assert!(!server.session.is_attached());
    // IPv6's loopback is a loopback source too.
    server.session.offer("{}".into());
    let answer = server
        .session
        .status_to_sign(session(1), "::1".parse().unwrap(), declared());
    assert!(matches!(
        answer.await,
        Ok(StatusToSignResponse::Offer { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn a_declaration_of_more_than_256_keys_is_refused() {
    let server = tokio_server();
    server.session.offer("first".into());
    let declare = |count: usize| StatusToSignRequest {
        public_keys: vec![key(1).to_hex(); count],
    };
    let refused = server
        .session
        .status_to_sign(session(1), forger(), declare(257))
        .await;
    assert_eq!(refused.unwrap_err().status, 499);
    assert!(!server.session.is_attached());
    let answer = server
        .session
        .status_to_sign(session(1), forger(), declare(256))
        .await;
    assert!(matches!(answer, Ok(StatusToSignResponse::Offer { .. })));
}

#[tokio::test(start_paused = true)]
async fn the_long_poll_answers_a_new_status_at_once_or_unchanged_after_ten_seconds() {
    let server = tokio_server();
    let started = Instant::now();
    let answer = server
        .session
        .status_to_sign(session(1), forger(), declared())
        .await;
    assert_eq!(answer.unwrap(), StatusToSignResponse::Unchanged);
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    // An offer waiting for the poll: answered at once, with the declared keys that are registered
    // delegates, in declared order, duplicates kept, and the session's challenge.
    server.session.offer("first".into());
    let (status, keys, challenge) = offered(
        server
            .session
            .status_to_sign(session(1), forger(), declared())
            .await
            .unwrap(),
    );
    assert_eq!(status, "first");
    assert_eq!(keys, [key(1), key(2), key(1)].map(|k| k.to_hex()));
    assert!(session_proof(&challenge).is_some());
    // An offer made while the poll waits answers it then.
    let poll = server
        .session
        .status_to_sign(session(1), forger(), declared());
    let offering = async {
        sleep(Duration::from_secs(3)).await;
        server.session.offer("second".into());
    };
    let started = Instant::now();
    let (answer, ()) = tokio::join!(poll, offering);
    assert_eq!(offered(answer.unwrap()).0, "second");
    assert_eq!(started.elapsed(), Duration::from_secs(3));
    // The same status again is no new offer.
    server.session.offer("second".into());
    let answer = server
        .session
        .status_to_sign(session(1), forger(), declared())
        .await;
    assert_eq!(answer.unwrap(), StatusToSignResponse::Unchanged);
}

#[tokio::test(start_paused = true)]
async fn signatures_are_verified_against_the_offer_they_name_and_stale_ones_dropped() {
    let server = tokio_server();
    server.session.offer("first".into());
    let (status, keys, challenge) = offered(
        server
            .session
            .status_to_sign(session(1), forger(), declared())
            .await
            .unwrap(),
    );
    // One signature is of other bytes: its key is not attested.
    let mut signatures = sign(&status, &keys, &challenge);
    signatures.signatures[1] = attest(b"other", &delegate(2)).unwrap().signature.to_hex();
    server
        .session
        .post_signatures(session(1), signatures)
        .unwrap();
    assert_eq!(server.session.attested(), [key(1), key(1)]);
    // A proof of another session's challenge, or no proof: that key is not attested either.
    let mut signatures = sign(&status, &keys, &challenge);
    signatures.proofs[0] = sign(&status, &keys, &"0".repeat(64)).proofs[0].clone();
    signatures.proofs.truncate(2);
    server
        .session
        .post_signatures(session(1), signatures)
        .unwrap();
    assert_eq!(server.session.attested(), [key(2)]);
    // Every signature verifies: the attested set in declared order, duplicates kept.
    server
        .session
        .post_signatures(session(1), sign(&status, &keys, &challenge))
        .unwrap();
    assert_eq!(server.session.attested(), [key(1), key(2), key(1)]);
    // Signatures naming no offer of the session, and of a session that is not this one: dropped.
    server
        .session
        .post_signatures(session(1), sign("never offered", &keys, &challenge))
        .unwrap();
    server
        .session
        .post_signatures(session(2), sign(&status, &keys, &challenge))
        .unwrap();
    assert_eq!(server.session.attested(), [key(1), key(2), key(1)]);
    // Once the status has changed, the old offer's signatures are dropped.
    server.session.offer("second".into());
    let mut stale = sign(&status, &keys, &challenge);
    stale.signatures.truncate(1);
    server.session.post_signatures(session(1), stale).unwrap();
    assert_eq!(server.session.attested(), [key(1), key(2), key(1)]);
    // An offer that was not delivered yet cannot be signed.
    server
        .session
        .post_signatures(session(1), sign("second", &keys, &challenge))
        .unwrap();
    let wait = server.session.signatures("second", Duration::ZERO).await;
    assert_eq!(wait, heartwood_p2p::attestation_session::Signing::Unsigned);
}

#[tokio::test(start_paused = true)]
async fn a_newer_session_replaces_the_older_one() {
    let server = Arc::new(tokio_server());
    server.session.offer("first".into());
    let (status, keys, challenge) = offered(
        server
            .session
            .status_to_sign(session(1), forger(), declared())
            .await
            .unwrap(),
    );
    server
        .session
        .post_signatures(session(1), sign(&status, &keys, &challenge))
        .unwrap();
    let older = {
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            server
                .session
                .status_to_sign(session(1), forger(), declared())
                .await
        })
    };
    sleep(Duration::from_secs(1)).await;
    // The newer session gets the status at once; the older one's poll ends, and so does its
    // attested set.
    let (again, _, second) = offered(
        server
            .session
            .status_to_sign(session(2), forger(), declared())
            .await
            .unwrap(),
    );
    assert_ne!(second, challenge);
    assert_eq!(again, status);
    let ended = older.await.unwrap().unwrap_err();
    assert_eq!(
        ended,
        ServerRefusal::new(499, "The attestation session has ended")
    );
    assert!(server.session.attested().is_empty());
    // The older session's signatures are dropped.
    server
        .session
        .post_signatures(session(1), sign(&status, &keys, &challenge))
        .unwrap();
    assert!(server.session.attested().is_empty());
    server
        .session
        .post_signatures(session(2), sign(&status, &keys, &second))
        .unwrap();
    assert_eq!(server.session.attested().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_session_ends_with_its_socket_or_after_fifteen_seconds_without_a_poll() {
    let server = tokio_server();
    server.session.offer("first".into());
    let (status, keys, challenge) = offered(
        server
            .session
            .status_to_sign(session(1), forger(), declared())
            .await
            .unwrap(),
    );
    server
        .session
        .post_signatures(session(1), sign(&status, &keys, &challenge))
        .unwrap();
    // Another socket's close changes nothing; the session's own ends it.
    server.session.closed(session(9), forger());
    assert!(server.session.is_attached());
    server.session.closed(session(1), forger());
    assert!(!server.session.is_attached());
    assert!(server.session.attested().is_empty());
    // A new session, then no poll: it lasts 15 s.
    server.session.offer("second".into());
    let (status, keys, challenge) = offered(
        server
            .session
            .status_to_sign(session(3), forger(), declared())
            .await
            .unwrap(),
    );
    server
        .session
        .post_signatures(session(3), sign(&status, &keys, &challenge))
        .unwrap();
    advance(Duration::from_millis(14_999)).await;
    assert_eq!(server.session.attested().len(), 3);
    advance(Duration::from_millis(1)).await;
    assert!(server.session.attested().is_empty());
    assert!(!server.session.is_attached());
    // A pending poll keeps a session: 10 s of poll, then 15 s from its end.
    server.session.offer("third".into());
    let (status, keys, challenge) = offered(
        server
            .session
            .status_to_sign(session(4), forger(), declared())
            .await
            .unwrap(),
    );
    server
        .session
        .post_signatures(session(4), sign(&status, &keys, &challenge))
        .unwrap();
    let answer = server
        .session
        .status_to_sign(session(4), forger(), declared())
        .await;
    assert_eq!(answer.unwrap(), StatusToSignResponse::Unchanged);
    advance(Duration::from_millis(14_999)).await;
    assert!(server.session.is_attached());
    advance(Duration::from_millis(1)).await;
    assert!(!server.session.is_attached());
}

#[tokio::test(start_paused = true)]
async fn the_delegates_on_this_node_are_the_attested_keys_of_the_round() {
    let server = tokio_server();
    let on_node = DelegatesOnThisNode(Arc::clone(&server.session));
    assert!(on_node.attested().is_empty());
    server.session.offer("first".into());
    let (status, keys, challenge) = offered(
        server
            .session
            .status_to_sign(session(1), forger(), declared())
            .await
            .unwrap(),
    );
    server
        .session
        .post_signatures(session(1), sign(&status, &keys, &challenge))
        .unwrap();
    // Delegate 2 is attested but not in the round of the tip plus one.
    *server.node.active.lock().unwrap() = vec![key(0), key(1)];
    assert_eq!(on_node.attested(), [key(1), key(1)]);
    assert_eq!(server.session.attested(), [key(1), key(2), key(1)]);
}

// ---- the served status ------------------------------------------------------------------------

fn served(bytes: &[u8]) -> GetStatusResponse {
    GetStatusResponse::decode(bytes).unwrap()
}

/// A forger that answers every offer of `server` at once with its signatures.
fn honest_forger(server: &Arc<Server>, id: u64) -> tokio::task::JoinHandle<()> {
    let server = Arc::clone(server);
    tokio::spawn(async move {
        loop {
            match server
                .session
                .status_to_sign(session(id), forger(), declared())
                .await
            {
                Ok(StatusToSignResponse::Offer {
                    status,
                    public_keys,
                    challenge,
                }) => {
                    let signatures = sign(&status, &public_keys, &challenge);
                    server
                        .session
                        .post_signatures(session(id), signatures)
                        .unwrap();
                }
                Ok(StatusToSignResponse::Unchanged) => {}
                Err(_) => return,
            }
        }
    })
}

#[tokio::test(start_paused = true)]
async fn a_session_without_the_keys_cannot_attest_with_the_signatures_the_status_serves() {
    let server = Arc::new(tokio_server());
    let forger_task = honest_forger(&server, 1);
    sleep(Duration::from_millis(10)).await;
    let signed = served(&server.status.get_status().await.unwrap());
    assert_eq!(signed.public_keys[0], key(1).to_hex());
    forger_task.abort();
    // Another loopback caller, which holds no key, declares delegate 1 thirty times and posts
    // the served signature of delegate 1 for each copy, as its signature and as its proof.
    let copies = StatusToSignRequest {
        public_keys: vec![key(1).to_hex(); 30],
    };
    let other: IpAddr = "::1".parse().unwrap();
    let answer = server.session.status_to_sign(session(2), other, copies);
    let Ok(StatusToSignResponse::Offer { status, .. }) = answer.await else {
        panic!("no offer");
    };
    let copied = vec![signed.signatures[0].clone(); 30];
    let offer = hex::encode(&sha256(status.as_bytes()));
    let body = json!({ "offer": offer, "signatures": copied, "proofs": copied });
    let posted = internal::decode::<StatusSignatures>(body.to_string().as_bytes()).unwrap();
    server.session.post_signatures(session(2), posted).unwrap();
    assert_eq!(server.session.attested(), []);
    let on_node = DelegatesOnThisNode(Arc::clone(&server.session));
    assert_eq!(on_node.attested(), []);
}

#[tokio::test(start_paused = true)]
async fn without_a_forger_the_status_is_served_unsigned_and_cached() {
    let server = tokio_server();
    let first = server.status.get_status().await.unwrap();
    let status = served(&first);
    assert!(status.public_keys.is_empty() && status.signatures.is_empty());
    let state = status.state.unwrap();
    assert_eq!(
        (state.height, state.current_slot, state.forging_allowed),
        (Some(10), Some(125), Some(true))
    );
    // The cache holds it while the key holds.
    advance(Duration::from_secs(1)).await;
    assert_eq!(server.status.get_status().await.unwrap(), first);
    // The half-slot flip changes the key.
    advance(Duration::from_secs(3)).await;
    let flipped = served(&server.status.get_status().await.unwrap());
    assert_eq!(flipped.state.unwrap().forging_allowed, Some(false));
}

#[tokio::test(start_paused = true)]
async fn with_a_forger_the_status_carries_its_signatures_in_the_offered_order() {
    let server = Arc::new(tokio_server());
    let forger_task = honest_forger(&server, 1);
    sleep(Duration::from_millis(10)).await;
    let started = Instant::now();
    let bytes = server.status.get_status().await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    let status = served(&bytes);
    assert_eq!(
        status.public_keys,
        [key(1), key(2), key(1)].map(|k| k.to_hex())
    );
    assert_eq!(status.signatures.len(), 3);
    // The signed status is the reference's: the peer fakes' status of the same block and slot.
    let tip = server.node.chain.last.lock().unwrap().clone();
    let reference = peer_fakes::status_reply(
        &tip,
        125,
        &server.node.config,
        &[delegate(1), delegate(2), delegate(1)],
    );
    assert_eq!(bytes, reference);
    // Cached: served again without a wait, even with the forger gone.
    forger_task.abort();
    assert_eq!(server.status.get_status().await.unwrap(), bytes);
}

#[tokio::test(start_paused = true)]
async fn a_slow_forger_costs_one_wait_of_500_ms_then_the_status_is_served_unsigned_until_signed() {
    let server = Arc::new(tokio_server());
    // A forger that polls but does not sign yet.
    let quiet = {
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            server
                .session
                .status_to_sign(session(1), forger(), declared())
                .await
        })
    };
    sleep(Duration::from_millis(10)).await;
    let started = Instant::now();
    let unsigned = served(&server.status.get_status().await.unwrap());
    assert_eq!(started.elapsed(), Duration::from_millis(500));
    assert!(unsigned.public_keys.is_empty());
    // Not cached, and no second wait for the same status.
    let started = Instant::now();
    assert!(
        served(&server.status.get_status().await.unwrap())
            .public_keys
            .is_empty()
    );
    assert_eq!(started.elapsed(), Duration::ZERO);
    // The signatures come: the next getStatus serves them and caches.
    let (status, keys, challenge) = offered(quiet.await.unwrap().unwrap());
    server
        .session
        .post_signatures(session(1), sign(&status, &keys, &challenge))
        .unwrap();
    let signed = server.status.get_status().await.unwrap();
    assert_eq!(served(&signed).public_keys.len(), 3);
    assert_eq!(server.status.get_status().await.unwrap(), signed);
}

#[tokio::test(start_paused = true)]
async fn the_wait_for_signatures_is_the_one_the_status_server_is_given() {
    let server = Arc::new(tokio_server());
    let status = StatusServer::new(
        server.node.clone(),
        server.session.clone(),
        server.clock.clone(),
    )
    .with_wait(Duration::from_millis(120));
    // A forger that polls but does not sign.
    let _quiet = {
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            server
                .session
                .status_to_sign(session(1), forger(), declared())
                .await
        })
    };
    sleep(Duration::from_millis(10)).await;
    let started = Instant::now();
    let unsigned = served(&status.get_status().await.unwrap());
    assert_eq!(started.elapsed(), Duration::from_millis(120));
    assert!(unsigned.public_keys.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_cached_unsigned_status_is_signed_once_a_forger_attaches() {
    let server = Arc::new(tokio_server());
    assert!(
        served(&server.status.get_status().await.unwrap())
            .public_keys
            .is_empty()
    );
    let _forger = honest_forger(&server, 1);
    sleep(Duration::from_millis(10)).await;
    assert_eq!(
        served(&server.status.get_status().await.unwrap())
            .public_keys
            .len(),
        3
    );
}

/// What `run` offered, read back from each offer: the slot, whether forging is allowed, the
/// height.
async fn offers_until(server: &Arc<Server>, end: Duration) -> Vec<(u32, bool, u32)> {
    let deadline = Instant::now() + end;
    let mut seen = Vec::new();
    loop {
        let poll = server
            .session
            .status_to_sign(session(1), forger(), declared());
        let answer = tokio::select! {
            answer = poll => answer.unwrap(),
            () = tokio::time::sleep_until(deadline) => return seen,
        };
        if let StatusToSignResponse::Offer { status, .. } = answer {
            let (state, _) = parse_preimage(status.as_bytes()).unwrap();
            let slot = u32::try_from(state.current_slot).unwrap();
            seen.push((slot, state.forging_allowed, state.height));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn the_timer_offers_each_new_tip_slot_and_half_slot_flip_without_a_get_status() {
    // 1 s into slot 125: the flip is at 1,004 s, the next slot at 1,008 s.
    let server = Arc::new(Server::new(
        Node::at(10),
        Arc::new(TokioClock::at(START_MS + 1_000)),
    ));
    let (stop, shutdown) = watch::channel(false);
    let timer = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { server.status.run(shutdown).await })
    };
    let seen = offers_until(&server, Duration::from_millis(7_500)).await;
    assert_eq!(seen, [(125, true, 10), (125, false, 10), (126, true, 10)]);
    // A new tip is offered within the timer's look at the tip.
    server.node.tip(block(11));
    let seen = offers_until(&server, Duration::from_millis(150)).await;
    assert_eq!(seen, [(126, true, 11)]);
    let _ = stop.send(true);
    timer.await.unwrap();
}

// ---- the attestation record (a harness build) ---------------------------------------------------

/// The attestation record's steps and their status hashes, in order.
#[cfg(feature = "difftest")]
fn steps_of(server: &Server) -> Vec<(String, i64)> {
    use heartwood_p2p::node_services::AttestationStep;
    let records = server.node.attestations.lock().unwrap();
    let name = |step: AttestationStep| match step {
        AttestationStep::Offered => "offered".to_owned(),
        AttestationStep::Signed { attestations } => format!("signed {attestations}"),
        AttestationStep::ServedUnsigned => "unsigned".to_owned(),
    };
    records.iter().map(|r| (name(r.step), r.at_ms)).collect()
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn the_record_has_an_offer_and_the_attachment_of_the_signatures() {
    let server = Arc::new(tokio_server());
    let forger_task = honest_forger(&server, 1);
    sleep(Duration::from_millis(10)).await;
    server.status.get_status().await.unwrap();
    let steps = steps_of(&server);
    let names: Vec<&str> = steps.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["offered", "signed 3"]);
    // The record names the status the forger signed, and the slot the status is of.
    let records = server.node.attestations.lock().unwrap().clone();
    assert_eq!(records[0].status, records[1].status);
    assert_eq!(records[0].status.len(), 64);
    assert_eq!(records[1].slot, 125);
    assert!(records[1].at_ms >= records[0].at_ms);
    // A cached status is not recorded again.
    server.status.get_status().await.unwrap();
    assert_eq!(steps_of(&server).len(), 2);
    forger_task.abort();
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn a_status_served_unsigned_after_the_wait_is_recorded_500_ms_after_its_offer() {
    let server = Arc::new(tokio_server());
    let quiet = {
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            server
                .session
                .status_to_sign(session(1), forger(), declared())
                .await
        })
    };
    sleep(Duration::from_millis(10)).await;
    server.status.get_status().await.unwrap();
    let steps = steps_of(&server);
    let names: Vec<&str> = steps.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["offered", "unsigned"]);
    assert_eq!(steps[1].1 - steps[0].1, 500);
    // The second getStatus for the same status serves it unsigned without a wait: the offer is
    // not repeated, and the unsigned status is recorded again (at once).
    server.status.get_status().await.unwrap();
    let names: Vec<String> = steps_of(&server)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["offered", "unsigned", "unsigned"]);
    quiet.abort();
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn without_a_forger_only_the_offer_is_recorded() {
    let server = tokio_server();
    server.status.get_status().await.unwrap();
    let names: Vec<String> = steps_of(&server)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["offered"]);
}

/// A build without the harness (a release build) records nothing at any of the three points: an
/// offer with its signatures, and an offer served unsigned after the wait.
#[cfg(not(feature = "difftest"))]
#[tokio::test(start_paused = true)]
async fn a_build_without_the_harness_records_no_attestation_step() {
    let signed = Arc::new(tokio_server());
    let forger_task = honest_forger(&signed, 1);
    sleep(Duration::from_millis(10)).await;
    let answer = served(&signed.status.get_status().await.unwrap());
    assert_eq!(answer.public_keys.len(), 3);
    forger_task.abort();
    let unsigned = Arc::new(tokio_server());
    let quiet = {
        let server = Arc::clone(&unsigned);
        tokio::spawn(async move {
            server
                .session
                .status_to_sign(session(1), forger(), declared())
                .await
        })
    };
    sleep(Duration::from_millis(10)).await;
    let answer = served(&unsigned.status.get_status().await.unwrap());
    assert!(answer.public_keys.is_empty());
    quiet.abort();
    for server in [&signed, &unsigned] {
        assert!(server.node.attestations.lock().unwrap().is_empty());
    }
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn the_record_file_has_one_json_line_per_step() {
    use heartwood_p2p::testing::attestation::AttestationRecordFile;
    let dir = std::env::temp_dir().join(format!("heartwood-p2p-attest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("attest-0000.jsonl");
    let _ = std::fs::remove_file(&path);
    let file = AttestationRecordFile::open(&path).unwrap();
    let server = Arc::new(tokio_server());
    let forger_task = honest_forger(&server, 1);
    sleep(Duration::from_millis(10)).await;
    server.status.get_status().await.unwrap();
    for record in server.node.attestations.lock().unwrap().iter() {
        file.write(record);
    }
    forger_task.abort();
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["step"], "offered");
    assert_eq!(lines[0]["attestations"], 0);
    assert_eq!(lines[1]["step"], "signed");
    assert_eq!(lines[1]["attestations"], 3);
    assert_eq!(lines[1]["slot"], 125);
    assert!(lines[1]["t"].as_str().unwrap().ends_with('Z'));
    assert_eq!(lines[0]["status"], lines[1]["status"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- class V31: the served status -------------------------------------------------------------

const CLASS: &str = "V31-server";

/// The class's `status.serve` records, or `None` when the directory `P2P_VECTORS` names lacks
/// the class. The crate's own directory must hold it.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("P2P_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        // A directory named in the environment may lack the class; the crate's own must hold it.
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    Some(
        records
            .filter(|record| record["op"] == "status.serve")
            .collect(),
    )
}

/// The tip of a record: the block the oracle made, as its bytes.
fn tip_block(tip: &Value) -> Block {
    let bytes = hex::decode(tip["hex"].as_str().unwrap()).unwrap();
    Block::decode(&bytes, &DEVNET.milestones).unwrap()
}

/// A status's configuration as the oracle writes it.
fn peer_config(config: &Value) -> PeerConfig {
    let text = |value: &Value| value.as_str().unwrap().to_owned();
    let network = &config["network"];
    let plugins = config["plugins"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, plugin)| {
            let port = plugin["port"].as_u64().unwrap() as u32;
            let plugin = PluginConfig {
                enabled: plugin["enabled"].as_bool().unwrap(),
                port,
                estimate_total_count: plugin.get("estimateTotalCount").and_then(Value::as_bool),
            };
            (name.clone(), plugin)
        });
    PeerConfig {
        version: text(&config["version"]),
        network: NetworkConfig {
            version: network["version"].as_u64().unwrap() as u8,
            name: text(&network["name"]),
            nethash: text(&network["nethash"]),
            explorer: text(&network["explorer"]),
            token: TokenConfig {
                name: text(&network["token"]["name"]),
                symbol: text(&network["token"]["symbol"]),
            },
        },
        plugins: plugins.collect(),
    }
}

/// The served status of a record's input: the tip, the registered delegates and the relay's
/// configuration, with a session of a forger declaring `forgerKeys` that signs every offer.
async fn serve(input: &Value) -> Value {
    let tip = tip_block(&input["tip"]);
    let keys: Vec<KeyPair> = (input["forgerKeys"].as_array().unwrap().iter())
        .map(|key| KeyPair::from_secret_key(SecretKey::from_hex(key.as_str().unwrap()).unwrap()))
        .collect();
    let mut chain = Chain::at(tip.height());
    *chain.last.lock().unwrap() = Arc::new(tip);
    chain.delegates = (input["registered"].as_array().unwrap().iter())
        .map(|key| PublicKeyBytes::from_hex(key.as_str().unwrap()).unwrap())
        .collect();
    let mut node = Node::over(chain);
    node.config = peer_config(&input["peerConfig"]);
    let clock = Arc::new(ManualClock::new(input["now"].as_i64().unwrap()));
    let server = Arc::new(Server::new(node, clock));
    if !keys.is_empty() {
        let request = StatusToSignRequest {
            public_keys: keys
                .iter()
                .map(|k| k.public_key().to_compressed().to_hex())
                .collect(),
        };
        let signer = {
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                while let Ok(answer) = server
                    .session
                    .status_to_sign(session(1), forger(), request.clone())
                    .await
                {
                    if let StatusToSignResponse::Offer {
                        status,
                        public_keys,
                        challenge,
                    } = answer
                    {
                        let proof = session_proof(&challenge).unwrap();
                        let signed = |text: &[u8]| -> Vec<String> {
                            (public_keys.iter())
                                .map(|hex| {
                                    let key = keys
                                        .iter()
                                        .find(|k| k.public_key().to_compressed().to_hex() == *hex);
                                    attest(text, key.unwrap()).unwrap().signature.to_hex()
                                })
                                .collect()
                        };
                        let offer = hex::encode(&sha256(status.as_bytes()));
                        let (signatures, proofs) = (signed(status.as_bytes()), signed(&proof));
                        let posted = StatusSignatures {
                            offer,
                            signatures,
                            proofs,
                        };
                        server.session.post_signatures(session(1), posted).unwrap();
                    }
                }
            })
        };
        tokio::task::yield_now().await;
        let bytes = server.status.get_status().await.unwrap();
        signer.abort();
        return json!({ "hex": hex::encode(&bytes) });
    }
    let bytes = server.status.get_status().await.unwrap();
    json!({ "hex": hex::encode(&bytes) })
}

#[tokio::test]
async fn the_served_status_matches_the_reference() {
    let Some(records) = records() else {
        eprintln!("{CLASS} is not in the directory P2P_VECTORS names");
        return;
    };
    assert!(!records.is_empty(), "{CLASS} has no status.serve record");
    for record in &records {
        let actual = serve(&record["input"]).await;
        assert_eq!(actual["hex"], record["output"]["hex"], "{}", record["name"]);
    }
}
