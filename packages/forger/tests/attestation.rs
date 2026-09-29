//! The attester: the offers it signs (the reference's status preimages of vector class V04) and
//! those it refuses (a text that is not a status tree or not its canonical form, or too long,
//! another height in the header, a slot away from the forger's clock, another network, a key it
//! does not hold or offered more often than declared, a malformed challenge), its signatures,
//! its proofs of the session's challenge and the offer's name, and its long poll.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{KeyPair, Milestones, Network};
use heartwood_forger::attestation::{POLL_FLOOR, RETRY, offer_hash};
use heartwood_forger::{
    AttestError, AttestLink, Attester, DelegateKeys, RelayError, ShutdownSignal,
};
use heartwood_kernel::ManualClock;
use heartwood_p2p_wire::errors::PreimageError;
use heartwood_p2p_wire::internal::{StatusSignatures, StatusToSignResponse};
use heartwood_p2p_wire::status::{session_proof, verify_attestation};
use serde_json::Value;

const BLOCK_TIME: i64 = 8;

fn vectors(file: &str) -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/crypto/tests/vectors")
        .join(file);
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The committed devnet's milestones.
fn milestones() -> Milestones {
    let digest = vectors("V02-transactions.jsonl")[0]["network"]["milestonesSha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let genesis = vectors("V10-genesis.jsonl");
    let files = (genesis.iter().skip(1))
        .map(|record| &record["output"]["files"])
        .find(|files| {
            let text = files["crypto/milestones.json"].as_str().unwrap();
            hex::encode(&sha256(text.as_bytes())) == digest
        })
        .unwrap();
    let network = Network::load(files["crypto/network.json"].as_str().unwrap()).unwrap();
    Milestones::load(files["crypto/milestones.json"].as_str().unwrap(), &network).unwrap()
}

/// The reference's status preimages of V04, with their slots.
fn preimages() -> Vec<(i64, String)> {
    (vectors("V04-signatures.jsonl").iter())
        .filter(|record| record["op"] == "status.preimage")
        .map(|record| {
            let bytes = hex::decode(record["output"]["preimage"].as_str().unwrap()).unwrap();
            let slot = record["input"]["currentSlot"].as_i64().unwrap();
            (slot, String::from_utf8(bytes).unwrap())
        })
        .collect()
}

fn pair(passphrase: &str) -> KeyPair {
    KeyPair::from_passphrase(passphrase).unwrap()
}

fn public(passphrase: &str) -> String {
    pair(passphrase).public_key().to_hex()
}

/// An attester for the keys of `passphrases`, on the network `nethash`, whose clock is in `slot`.
fn make_attester(passphrases: &[&str], nethash: &str, slot: i64) -> (Attester, Arc<ManualClock>) {
    let milestones = milestones();
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap();
    let clock = Arc::new(ManualClock::new(epoch + slot * BLOCK_TIME * 1_000 + 500));
    let keys = DelegateKeys::from_keys(passphrases.iter().map(|p| pair(p)).collect());
    let attester = Attester::new(Arc::new(keys), nethash.into(), milestones, clock.clone());
    (attester, clock)
}

const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// A session's challenge.
const CHALLENGE: &str = "5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed";

#[test]
fn the_references_statuses_are_signed_with_every_offered_key_in_order() {
    let preimages = preimages();
    assert_eq!(preimages.len(), 5);
    for (slot, status) in preimages {
        let declared = ["delegate b", "delegate a", "delegate b"];
        let (attester, _) = make_attester(&declared, ZERO, slot);
        let offered = [
            public("delegate b"),
            public("delegate a"),
            public("delegate b"),
        ];
        let signed = attester.sign_offer(&status, &offered, CHALLENGE).unwrap();
        assert_eq!(signed.offer, hex::encode(&sha256(status.as_bytes())));
        assert_eq!(signed.offer, offer_hash(&status));
        assert_eq!(signed.signatures.len(), 3);
        for (key, signature) in offered.iter().zip(&signed.signatures) {
            assert!(verify_attestation(status.as_bytes(), key, signature));
        }
        // Each key also signs the session's challenge, in the same order.
        let proof = session_proof(CHALLENGE).unwrap();
        assert_eq!(signed.proofs.len(), 3);
        for (key, proven) in offered.iter().zip(&signed.proofs) {
            assert!(verify_attestation(&proof, key, proven));
        }
        // The legacy scheme is deterministic: a key declared and offered twice signs the same
        // signature twice.
        assert_eq!(signed.signatures[0], signed.signatures[2]);
        assert!(
            attester
                .sign_offer(&status, &[], CHALLENGE)
                .unwrap()
                .signatures
                .is_empty()
        );
    }
}

#[test]
fn a_text_that_is_not_a_canonical_status_is_refused() {
    let (slot, status) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a"], ZERO, slot);
    let keys = [public("delegate a")];
    let refused = |text: &str| attester.sign_offer(text, &keys, CHALLENGE).unwrap_err();
    assert_eq!(
        refused("{}"),
        AttestError::Preimage(PreimageError::Shape {
            path: "config".into()
        })
    );
    assert_eq!(
        refused("not json"),
        AttestError::Preimage(PreimageError::NotJson)
    );
    let spaced = status.replacen(':', ": ", 1);
    assert_eq!(
        refused(&spaced),
        AttestError::Preimage(PreimageError::NotCanonical)
    );
    let extra = status.replacen(r#""height":2}"#, r#""height":2,"extra":1}"#, 1);
    assert!(matches!(
        refused(&extra),
        AttestError::Preimage(PreimageError::Shape { .. })
    ));
}

#[test]
fn a_status_whose_height_is_not_its_headers_is_refused() {
    let (slot, status) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a"], ZERO, slot);
    // The state's height is the last key of the text; the header's is inside it.
    let other = format!("{}3}}}}", status.strip_suffix("2}}").unwrap());
    assert_eq!(
        attester.sign_offer(&other, &[public("delegate a")], CHALLENGE),
        Err(AttestError::Height {
            state: 3,
            header: 2
        })
    );
}

#[test]
fn a_status_more_than_one_slot_from_the_forgers_clock_is_refused() {
    let (slot, status) = preimages().remove(1);
    assert_eq!(slot, 2);
    let keys = [public("delegate a")];
    for own in [1, 2, 3] {
        let (attester, _) = make_attester(&["delegate a"], ZERO, own);
        assert!(
            attester.sign_offer(&status, &keys, CHALLENGE).is_ok(),
            "{own}"
        );
    }
    for own in [0, 4, -5] {
        let (attester, _) = make_attester(&["delegate a"], ZERO, own);
        assert_eq!(
            attester.sign_offer(&status, &keys, CHALLENGE),
            Err(AttestError::Slot { offered: 2, own })
        );
    }
    // The clock moves: the same offer is refused two slots later.
    let (attester, clock) = make_attester(&["delegate a"], ZERO, 2);
    assert!(attester.sign_offer(&status, &keys, CHALLENGE).is_ok());
    clock.advance(2 * BLOCK_TIME * 1_000);
    assert!(matches!(
        attester.sign_offer(&status, &keys, CHALLENGE),
        Err(AttestError::Slot { .. })
    ));
}

#[test]
fn a_status_of_another_network_or_for_a_key_it_does_not_hold_is_refused() {
    let (slot, status) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a"], &"11".repeat(32), slot);
    assert_eq!(
        attester.sign_offer(&status, &[public("delegate a")], CHALLENGE),
        Err(AttestError::Network {
            offered: ZERO.into(),
            own: "11".repeat(32)
        })
    );
    let (attester, _) = make_attester(&["delegate a"], ZERO, slot);
    let stranger = public("delegate c");
    assert_eq!(
        attester.sign_offer(
            &status,
            &[public("delegate a"), stranger.clone()],
            CHALLENGE
        ),
        Err(AttestError::ForeignKey {
            public_key: stranger
        })
    );
    let upper = public("delegate a").to_uppercase();
    assert!(matches!(
        attester.sign_offer(&status, &[upper], CHALLENGE),
        Err(AttestError::ForeignKey { .. })
    ));
    // A challenge of another form is refused before anything is signed.
    let keys = [public("delegate a")];
    for challenge in ["", &CHALLENGE[1..], &CHALLENGE.to_uppercase()] {
        assert!(matches!(
            attester.sign_offer(&status, &keys, challenge),
            Err(AttestError::Challenge { .. })
        ));
    }
}

#[test]
fn an_offer_of_a_key_more_often_than_declared_or_of_an_oversized_status_is_refused() {
    let (slot, status) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a", "delegate b"], ZERO, slot);
    let (a, b) = (public("delegate a"), public("delegate b"));
    assert!(
        attester
            .sign_offer(&status, &[b.clone(), a.clone()], CHALLENGE)
            .is_ok()
    );
    // Each declared key signs at most as often as it is declared.
    let copies = vec![a.clone(); 1_000];
    assert_eq!(
        attester.sign_offer(&status, &copies, CHALLENGE),
        Err(AttestError::Repeated { public_key: a })
    );
    // A canonical status whose explorer text is 1 MiB is refused before it is read.
    let explorer = format!(r#""explorer":"{}""#, "e".repeat(1024 * 1024));
    let at = status.find(r#""explorer":"#).unwrap();
    let end = at + status[at + 12..].find('"').unwrap() + 13;
    let large = format!("{}{explorer}{}", &status[..at], &status[end..]);
    assert!(heartwood_p2p_wire::status::parse_preimage(large.as_bytes()).is_ok());
    assert_eq!(
        attester.sign_offer(&large, std::slice::from_ref(&b), CHALLENGE),
        Err(AttestError::Oversized {
            length: large.len()
        })
    );
}

/// A relay's attestation routes, answering from a script and recording what was posted; one
/// that is `remote` (off the forger's host) refuses, as the client refuses it.
#[derive(Default)]
struct ScriptedRelay {
    answers: Mutex<VecDeque<Result<StatusToSignResponse, RelayError>>>,
    polls: Mutex<Vec<Vec<String>>>,
    polled_at: Mutex<Vec<tokio::time::Instant>>,
    posted: Mutex<Vec<StatusSignatures>>,
    remote: AtomicBool,
}

impl AttestLink for ScriptedRelay {
    fn selected_is_local(&self) -> bool {
        !self.remote.load(Ordering::SeqCst)
    }

    async fn status_to_sign(&self, keys: &[String]) -> Result<StatusToSignResponse, RelayError> {
        self.polls.lock().unwrap().push(keys.to_vec());
        self.polled_at
            .lock()
            .unwrap()
            .push(tokio::time::Instant::now());
        if self.remote.load(Ordering::SeqCst) {
            return Err(RelayError::Communication {
                endpoint: "192.0.2.1:4002<p2p.internal.getStatusToSign>".into(),
                message: "status attestations go to a relay on the forger's host only".into(),
            });
        }
        let next = self.answers.lock().unwrap().pop_front();
        match next {
            Some(answer) => answer,
            // The long poll: nothing new for 10 s.
            None => {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(StatusToSignResponse::Unchanged)
            }
        }
    }

    async fn post_signatures(&self, signatures: StatusSignatures) -> Result<(), RelayError> {
        self.posted.lock().unwrap().push(signatures);
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn the_long_poll_answers_each_good_offer_and_stops_at_the_shutdown() {
    let (slot, status) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a", "delegate a"], ZERO, slot);
    let offer = |status: &str| {
        Ok(StatusToSignResponse::Offer {
            status: status.into(),
            public_keys: vec![public("delegate a")],
            challenge: CHALLENGE.into(),
        })
    };
    let relay = ScriptedRelay::default();
    relay.answers.lock().unwrap().extend([
        offer(&status),
        Ok(StatusToSignResponse::Unchanged),
        offer("{}"),
        Err(RelayError::Communication {
            endpoint: "127.0.0.1:4002<p2p.internal.getStatusToSign>".into(),
            message: "App is not ready".into(),
        }),
        offer(&status),
    ]);
    let (trigger, shutdown) = ShutdownSignal::new();
    let started = tokio::time::Instant::now();
    let stop = async {
        tokio::time::sleep(Duration::from_secs(25)).await;
        trigger.trigger();
    };
    tokio::join!(attester.run(&relay, shutdown), stop);
    // It stopped at the shutdown, within its long poll.
    assert_eq!(started.elapsed(), Duration::from_secs(25));
    let posted = relay.posted.lock().unwrap();
    assert_eq!(posted.len(), 2, "the refused offer is not answered");
    assert!(posted.iter().all(|p| p.offer == offer_hash(&status)));
    let polls = relay.polls.lock().unwrap();
    // Five scripted answers, 250 ms apart but for the pause of 1 s after the error, then long
    // polls of 10 s from 2 s, 12 s and 22 s.
    assert_eq!(polls.len(), 8);
    let polled = relay.polled_at.lock().unwrap();
    let from = |at: &tokio::time::Instant| (*at - started).as_millis();
    assert_eq!(
        polled.iter().map(from).collect::<Vec<_>>(),
        [0, 250, 500, 750, 1_750, 2_000, 12_000, 22_000]
    );
    let declared = vec![public("delegate a"), public("delegate a")];
    assert!(polls.iter().all(|keys| *keys == declared));
    assert!(RETRY <= Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn a_relay_that_answers_unchanged_at_once_is_polled_four_times_a_second_at_most() {
    let (slot, _) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a"], ZERO, slot);
    let relay = ScriptedRelay::default();
    let unchanged = (0..8).map(|_| Ok(StatusToSignResponse::Unchanged));
    relay.answers.lock().unwrap().extend(unchanged);
    let (trigger, shutdown) = ShutdownSignal::new();
    let stop = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        trigger.trigger();
    };
    let started = tokio::time::Instant::now();
    tokio::join!(attester.run(&relay, shutdown), stop);
    // Eight quick answers, then the long poll of the ninth.
    let polled: Vec<Duration> = (relay.polled_at.lock().unwrap().iter())
        .map(|at| *at - started)
        .collect();
    assert_eq!(polled.len(), 9);
    for (n, at) in polled.iter().enumerate() {
        assert_eq!(*at, POLL_FLOOR * u32::try_from(n).unwrap());
    }
}

#[tokio::test(start_paused = true)]
async fn a_relay_that_offers_at_once_is_polled_four_times_a_second_at_most() {
    let (slot, status) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a"], ZERO, slot);
    let relay = ScriptedRelay::default();
    let offer = || {
        Ok(StatusToSignResponse::Offer {
            status: status.clone(),
            public_keys: vec![public("delegate a")],
            challenge: CHALLENGE.into(),
        })
    };
    relay
        .answers
        .lock()
        .unwrap()
        .extend((0..8).map(|_| offer()));
    let (trigger, shutdown) = ShutdownSignal::new();
    let stop = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        trigger.trigger();
    };
    let started = tokio::time::Instant::now();
    tokio::join!(attester.run(&relay, shutdown), stop);
    // Each offer is signed and answered at once; the next poll waits for the floor.
    assert_eq!(relay.posted.lock().unwrap().len(), 8);
    let polled: Vec<Duration> = (relay.polled_at.lock().unwrap().iter())
        .map(|at| *at - started)
        .collect();
    assert_eq!(polled.len(), 9);
    for (n, at) in polled.iter().enumerate() {
        assert_eq!(*at, POLL_FLOOR * u32::try_from(n).unwrap());
    }
}

#[tokio::test(start_paused = true)]
async fn a_relay_off_the_forgers_host_is_not_polled_until_the_selection_comes_back() {
    let (slot, _) = preimages().remove(0);
    let (attester, _) = make_attester(&["delegate a"], ZERO, slot);
    let relay = ScriptedRelay::default();
    relay.remote.store(true, Ordering::SeqCst);
    let (trigger, shutdown) = ShutdownSignal::new();
    let script = async {
        tokio::time::sleep(Duration::from_millis(4_500)).await;
        relay.remote.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(5_500)).await;
        trigger.trigger();
    };
    let started = tokio::time::Instant::now();
    tokio::join!(attester.run(&relay, shutdown), script);
    // The selection is looked at once a second; the first poll is the one after it came back,
    // and the long poll holds until the shutdown.
    let polled: Vec<Duration> = (relay.polled_at.lock().unwrap().iter())
        .map(|at| *at - started)
        .collect();
    assert_eq!(polled, [RETRY * 5]);
}
