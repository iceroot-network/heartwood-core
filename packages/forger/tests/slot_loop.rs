//! The slot loop, the attempts of a slot and the quorum's reading, against a fake relay and a
//! recording block signer on tokio's paused clock: every branch and line of the reference's
//! `ForgerService`, the error count, the 200 ms floor with a clock ahead, the graceful error log,
//! the deadlines, and the shutdown with a `postBlock` in flight.
//!
//! The fake relay models a chain of 8-second slots: the round's forger is the active delegate at
//! the slot's index, a posted block becomes the tip, and a test can make any call fail or answer
//! otherwise. Its clock and the forger's are tokio's paused clock unless a test moves the
//! forger's.

#[path = "support/mock_relay.rs"]
mod mock_relay;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::crypto::sig::{self, SchemeId, SigningDomain};
use heartwood_crypto::{Aux, Block, BlockHeader, PublicKeyBytes};
use heartwood_forger::errors::{RelayError, SignError};
use heartwood_forger::forger_service::{AttemptEnd, ForgerParts, ForgerService};
use heartwood_forger::options::Options;
use heartwood_forger::seams::{
    BlockSigner, Deadline, ForgeInputs, ForgerEvent, ForgingTransactions, RelayLink,
    ShutdownSignal, ShutdownTrigger, Signed, SignedOutcome,
};
use heartwood_kernel::{Clock, ManualClock};
use heartwood_p2p_wire::internal::{
    CurrentRound, NetworkState, NetworkStateStatus, QuorumDelegates,
};
use heartwood_p2p_wire::status::ReceivedStatus;
use mock_relay::*;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

/// The tip of the fake relay's chain.
struct Tip {
    height: u32,
    slot: i64,
    generator: String,
}

/// A relay over a chain of 8-second slots, on tokio's clock.
struct FakeRelay {
    clock: Arc<TokioClock>,
    active: Mutex<Vec<u32>>,
    registered: Vec<u32>,
    tip: Mutex<Tip>,
    calls: Mutex<Vec<String>>,
    failures: Mutex<HashMap<&'static str, VecDeque<RelayError>>>,
    state: Mutex<Option<NetworkState>>,
    status: Mutex<Option<ReceivedStatus>>,
    rounds: Mutex<VecDeque<CurrentRound>>,
    /// Slot answers that replace the chain's, in order.
    slots: Mutex<VecDeque<i64>>,
    post_delay: Mutex<Duration>,
    /// The calls' deadlines, as the time left when each call was made.
    deadlines: Mutex<Vec<(&'static str, Duration)>>,
}

impl FakeRelay {
    fn new(start_ms: i64, active: &[u32], registered: &[u32]) -> Arc<FakeRelay> {
        Arc::new(FakeRelay {
            clock: TokioClock::at(start_ms),
            active: Mutex::new(active.to_vec()),
            registered: registered.to_vec(),
            tip: Mutex::new(Tip {
                height: 1,
                slot: 0,
                generator: hex(0),
            }),
            calls: Mutex::default(),
            failures: Mutex::default(),
            state: Mutex::default(),
            status: Mutex::default(),
            rounds: Mutex::default(),
            slots: Mutex::default(),
            post_delay: Mutex::default(),
            deadlines: Mutex::default(),
        })
    }

    fn slot(&self) -> i64 {
        (self.clock.now_ms() - EPOCH_MS).div_euclid(8_000)
    }

    fn fail(&self, call: &'static str, times: usize, error: RelayError) {
        let mut failures = self.failures.lock().unwrap();
        failures
            .entry(call)
            .or_default()
            .extend(std::iter::repeat_n(error, times));
    }

    /// Record `call`, and fail it if a failure is set for it.
    fn call(&self, call: &'static str, detail: String) -> Result<(), RelayError> {
        self.calls.lock().unwrap().push(format!("{call}{detail}"));
        let failure = self
            .failures
            .lock()
            .unwrap()
            .get_mut(call)
            .and_then(VecDeque::pop_front);
        failure.map_or(Ok(()), Err)
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, prefix: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }

    fn deadline(&self, call: &'static str, deadline: Deadline) {
        self.deadlines
            .lock()
            .unwrap()
            .push((call, deadline.remaining()));
    }
}

fn communication(route: &str, message: &str) -> RelayError {
    RelayError::Communication {
        endpoint: format!("127.10.0.1:6002<{route}>"),
        message: message.into(),
    }
}

impl RelayLink for FakeRelay {
    async fn get_round(&self) -> Result<CurrentRound, RelayError> {
        self.call("round", String::new())?;
        if let Some(round) = self.rounds.lock().unwrap().pop_front() {
            return Ok(round);
        }
        let (slot, active) = (self.slot(), self.active.lock().unwrap().clone());
        let index = |slot: i64| usize::try_from(slot.rem_euclid(active.len() as i64)).ok();
        let height = self.tip.lock().unwrap().height;
        Ok(round(
            slot * 8,
            height,
            &active,
            &self.registered,
            index(slot),
            index(slot + 1),
        ))
    }

    async fn get_slot_number(
        &self,
        timestamp: Option<i64>,
        deadline: Deadline,
    ) -> Result<i64, RelayError> {
        self.deadline("slot", deadline);
        self.call("slot", format!(" {timestamp:?}"))?;
        let scripted = self.slots.lock().unwrap().pop_front();
        Ok(scripted
            .unwrap_or_else(|| timestamp.map_or(self.slot(), |seconds| seconds.div_euclid(8))))
    }

    async fn get_network_state(
        &self,
        log: bool,
        deadline: Deadline,
    ) -> Result<NetworkState, RelayError> {
        self.deadline("state", deadline);
        self.call("state", format!(" {log}"))?;
        let height = self.tip.lock().unwrap().height;
        Ok(self
            .state
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| state(height)))
    }

    async fn get_status(&self, deadline: Deadline) -> Result<ReceivedStatus, RelayError> {
        self.deadline("status", deadline);
        self.call("status", String::new())?;
        if let Some(status) = self.status.lock().unwrap().clone() {
            return Ok(status);
        }
        let tip = self.tip.lock().unwrap();
        let timestamp = u32::try_from(tip.slot * 8).unwrap();
        Ok(status(tip.height, timestamp, &tip.generator))
    }

    async fn get_transactions(
        &self,
        deadline: Deadline,
    ) -> Result<Option<ForgingTransactions>, RelayError> {
        self.deadline("pool", deadline);
        self.call("pool", String::new())?;
        Ok(Some(ForgingTransactions::default()))
    }

    async fn broadcast_block(&self, bytes: &[u8], deadline: Deadline) {
        self.deadline("post", deadline);
        let delay = *self.post_delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        let block = Block::decode_trusted(bytes, &milestones()).unwrap();
        if self.call("post", format!(" {}", block.height())).is_ok() {
            let header = block.header();
            *self.tip.lock().unwrap() = Tip {
                height: header.height,
                slot: i64::from(header.timestamp) / 8,
                generator: header.generator_public_key.to_hex(),
            };
        }
    }

    async fn sync_with_network(&self) -> Result<(), RelayError> {
        self.call("sync", String::new())
    }

    async fn emit_event(&self, event: ForgerEvent) {
        let body = match &event {
            ForgerEvent::Started { active_delegates } => format!(" {}", active_delegates.len()),
            ForgerEvent::Failed { error } => format!(" {error}"),
            ForgerEvent::BlockForged { header } => format!(" {}", header["height"]),
            ForgerEvent::TransactionForged { .. } => String::new(),
        };
        let _ = self.call("event", format!(" {}{body}", event.name()));
    }

    fn dispose(&self) {
        let _ = self.call("dispose", String::new());
    }
}

/// A signer that makes each delegate's empty block from its inputs and records the calls; a slot
/// signed before gives its block back, as the signing record does.
#[derive(Clone, Default)]
struct Recorder {
    calls: Arc<Mutex<Vec<(String, i64, i64)>>>,
    signed: Arc<Mutex<HashMap<(String, i64), Signed>>>,
}

impl BlockSigner for Recorder {
    fn sign_or_resend(
        &mut self,
        public_key: &PublicKeyBytes,
        inputs: &ForgeInputs<'_>,
        own_slot: i64,
        slot: i64,
    ) -> Result<SignedOutcome, SignError> {
        let hex = public_key.to_hex();
        self.calls
            .lock()
            .unwrap()
            .push((hex.clone(), own_slot, slot));
        if let Some(signed) = self.signed.lock().unwrap().get(&(hex.clone(), slot)) {
            return Ok(SignedOutcome::Resent(signed.clone()));
        }
        let n = (0..100).find(|n| mock_relay::hex(*n) == hex).unwrap();
        let draft = BlockDraft {
            version: 0,
            timestamp: inputs.timestamp,
            height: inputs.previous_height + 1,
            previous_block: *inputs.previous_id.as_bytes(),
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward: inputs.reward,
            payload_length: 0,
            payload_hash: heartwood_crypto::crypto::hash::sha256(b""),
        };
        let block = factory::make(
            draft,
            Vec::new(),
            &key(n),
            Aux::fixed([1; 32]),
            &milestones(),
        )
        .unwrap();
        let signed = Signed {
            height: block.height(),
            slot,
            id: *block.id(),
            bytes: block.encode(),
        };
        self.signed
            .lock()
            .unwrap()
            .insert((hex, slot), signed.clone());
        Ok(SignedOutcome::New(signed))
    }
}

struct Forger {
    service: ForgerService<FakeRelay, Recorder>,
    signer: Recorder,
}

fn forger(relay: &Arc<FakeRelay>, keys: &[u32], clock: Arc<dyn Clock>) -> Forger {
    let signer = Recorder::default();
    let parts = ForgerParts {
        link: relay.clone(),
        signer: signer.clone(),
        delegates: keys
            .iter()
            .map(|n| key(*n).public_key().to_compressed())
            .collect(),
        options: Options::from_json(&Value::Null, &|_| None).unwrap(),
        milestones: milestones(),
        clock,
    };
    Forger {
        service: ForgerService::new(parts),
        signer,
    }
}

/// Run the loop of `forger` until `ms` milliseconds have passed, then shut it down.
async fn run_for(forger: &Forger, ms: u64) {
    let (trigger, signal): (ShutdownTrigger, ShutdownSignal) = ShutdownSignal::new();
    let stop = async {
        sleep_ms(ms).await;
        trigger.trigger();
    };
    tokio::join!(forger.service.run(signal), stop);
}

#[tokio::test(start_paused = true)]
async fn the_forger_boots_announces_its_delegates_and_forges_its_slots() {
    let (lines, _guard) = Lines::capture();
    // Slot 13's forger is delegate 4 and the next one 5; the forger's delegates 1 and 2 forge
    // slots 15 and 16.
    let relay = FakeRelay::new(in_slot(13, 0), &[1, 2, 3, 4, 5], &[7]);
    let mut forger = forger(&relay, &[1, 2, 1, 7, 9], relay.clock.clone());
    forger.service.boot().await.unwrap();
    let unregistered = format!("unregistered delegate (public key: {})", hex(9));
    assert_eq!(
        lines.all(),
        [
            "INFO: Loaded 2 active delegates: genesis_1, genesis_2".to_owned(),
            format!("INFO: Loaded 2 inactive delegates: genesis_7, {unregistered}"),
            "INFO: Forger Manager started".to_owned(),
        ]
    );
    assert_eq!(
        relay.calls(),
        ["round", "event forger.started 4"],
        "the delegates deduplicated"
    );
    lines.clear();
    run_for(&forger, 8_000 * 4).await;

    assert_eq!(
        lines.with("due to forge"),
        ["INFO: Delegate genesis_1 is due to forge in 8 seconds from now"]
    );
    assert_eq!(relay.count("sync"), 1);
    assert_eq!(
        lines.with("forged a new block"),
        [
            "INFO: Delegate genesis_1 forged a new block at height 2 with 0 transactions",
            "INFO: Delegate genesis_2 forged a new block at height 3 with 0 transactions",
        ]
    );
    assert_eq!(
        lines.with("Received"),
        ["DEBUG: Received 0 transactions from the pool containing 0 transactions total"; 2]
    );
    let signed: Vec<(String, i64, i64)> = forger.signer.calls.lock().unwrap().clone();
    assert_eq!(
        signed,
        [(hex(1), 15, 15), (hex(2), 16, 16)],
        "one signature per slot, on the forger's own slot"
    );
    assert_eq!(relay.count("post"), 2);
    assert_eq!(relay.count("event block.forged"), 2);
    // One slot, from its check: the first attempt forges, and the second one, a second later,
    // finds the block in the current slot and stops.
    let calls = relay.calls();
    let from = calls.iter().position(|call| call == "state true").unwrap();
    let to = calls
        .iter()
        .skip(from + 1)
        .position(|call| call == "state true")
        .unwrap()
        + from
        + 1;
    assert_eq!(
        calls[from..to],
        [
            "state true",
            "slot Some(120)",
            "pool",
            "status",
            "slot None",
            "slot Some(0)",
            "slot Some(120)",
            "post 2",
            "event block.forged 2",
            "round",
            "state false",
            "slot Some(120)",
            "status",
            "slot None",
            "slot Some(120)",
            "round",
        ]
    );
    // The calls of a slot's start have the next slot's end, 16 s away, as their deadline.
    let deadlines = relay.deadlines.lock().unwrap().clone();
    assert!(
        deadlines
            .iter()
            .take(4)
            .all(|(_, left)| *left == Duration::from_secs(16)),
        "{deadlines:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_forger_waits_for_a_responsive_host_and_its_relay_once() {
    let (lines, _guard) = Lines::capture();
    let relay = FakeRelay::new(in_slot(10, 0), &[3, 4], &[]);
    relay.fail(
        "round",
        1,
        RelayError::HostNoResponse {
            hosts: "127.10.0.1".into(),
        },
    );
    let not_ready = communication(
        "p2p.internal.getCurrentRound",
        r#"{"error":"Unknown","message":"App is not ready"}"#,
    );
    relay.fail("round", 3, not_ready);
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    assert_eq!(lines.all(), ["WARNING: Waiting for a responsive host"]);
    // The first check comes 2 s after the boot; the relay then answers three times, 2 s apart,
    // that it is not ready, and gives a round at 8 s.
    run_for(&forger, 9_000).await;
    assert_eq!(relay.count("round"), 5);
    assert_eq!(
        lines.with("ready"),
        ["INFO: Waiting for relay to become ready"],
        "once"
    );
    assert_eq!(lines.with("Forger Manager started").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_delegates_entering_and_leaving_the_active_positions_are_announced() {
    let (lines, _guard) = Lines::capture();
    let relay = FakeRelay::new(in_slot(10, 4_000), &[1, 9], &[1, 2, 3, 4]);
    let mut forger = forger(&relay, &[1, 2, 3, 4, 5], relay.clock.clone());
    forger.service.boot().await.unwrap();
    assert_eq!(
        lines.with("Loaded"),
        [
            "INFO: Loaded 1 active delegate: genesis_1".to_owned(),
            format!(
                "INFO: Loaded 4 inactive delegates: genesis_2, genesis_3, genesis_4, unregistered delegate (public key: {})",
                hex(5)
            ),
        ]
    );
    lines.clear();
    *relay.active.lock().unwrap() = vec![2, 3, 4, 9];
    run_for(&forger, 1).await;
    *relay.active.lock().unwrap() = vec![3, 9, 5];
    run_for(&forger, 4_000).await;
    assert_eq!(lines.with("position"), [
        "INFO: Delegates genesis_2, genesis_3 and genesis_4 are now in active forging positions".to_owned(),
        "INFO: Delegate genesis_1 is no longer in an active forging position".to_owned(),
        "INFO: Delegate genesis_5 is now in an active forging position".to_owned(),
        "INFO: Delegates genesis_2 and genesis_4 are no longer in active forging positions".to_owned(),
    ]);
}

/// A network state at `height` with these counts.
fn counted(status: NetworkStateStatus, has: u32, no: u32) -> NetworkState {
    let mut state = state(2);
    state.status = status;
    state.quorum_details.has_quorum = has;
    state.quorum_details.no_quorum = no;
    state
}

/// The status header of `block` as a relay keeps a peer's.
fn header(block: &Block) -> Value {
    let mut json = block.to_json(&milestones());
    json.as_object_mut()
        .unwrap()
        .retain(|key, _| !["transactions", "burnedFee", "donations"].contains(&key.as_str()));
    json
}

#[tokio::test(start_paused = true)]
async fn the_quorum_is_read_as_the_reference_reads_it() {
    let (lines, _guard) = Lines::capture();
    let relay = FakeRelay::new(in_slot(10, 0), &[1, 2], &[]);
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    lines.clear();
    let allowed = |state: &NetworkState| {
        forger
            .service
            .is_forging_allowed(state, &key(1).public_key().to_compressed())
    };
    for (status, line) in [
        (
            NetworkStateStatus::Unknown,
            "INFO: Failed to get network state from client. Will not forge",
        ),
        (
            NetworkStateStatus::ColdStart,
            "INFO: Skipping slot because of cold start. Will not forge",
        ),
        (
            NetworkStateStatus::BelowMinimumDelegates,
            "INFO: Not peered with enough delegates to get quorum. Will not forge",
        ),
    ] {
        assert!(!allowed(&counted(status, 53, 0)));
        assert_eq!(lines.all(), [line]);
        lines.clear();
    }
    assert!(allowed(&counted(NetworkStateStatus::Default, 27, 26)));
    assert!(
        allowed(&counted(NetworkStateStatus::Test, 0, 9)),
        "the test status always forges"
    );
    assert!(lines.all().is_empty());

    let mut refused = counted(NetworkStateStatus::Default, 26, 27);
    refused.quorum_details.delegates = QuorumDelegates {
        has_quorum: vec!["genesis_2".into()],
        ..QuorumDelegates::default()
    };
    let others = header(&block(2, 3, 16));
    refused
        .quorum_details
        .over_height_block_headers
        .insert("192.0.2.2".into(), others.clone());
    refused
        .quorum_details
        .over_height_block_headers
        .insert("192.0.2.3".into(), others.clone());
    assert!(!allowed(&refused));
    let dump = format!(
        "DEBUG: Network State: {}",
        heartwood_crypto::utils::js::stringify(
            &json!({
                "quorum": 26.0 / 53.0,
                "quorumDetails": {
                    "hasQuorum": 26, "noQuorum": 27, "overHeight": 0, "overHeightBlockHeaders": [others],
                    "forked": 0, "differentSlot": 0, "forgingNotAllowed": 0,
                    "delegates": {"hasQuorum": ["genesis_2"], "noQuorum": [], "noResponse": []},
                },
                "nodeHeight": 2, "lastBlockId": block_id(2), "lastSlotNumber": 0,
            }),
            2
        )
    );
    // The dump's lines are one log message.
    assert_eq!(
        lines.all().join("\n"),
        [
            "INFO: Detected 1 distinct overheight block header".to_owned(),
            "INFO: Not enough quorum to forge next block. Will not forge".to_owned(),
            dump,
        ]
        .join("\n")
    );
    lines.clear();

    // An over-height header by the delegate's own key: refused when its signature verifies,
    // whether or not its block is empty; a header whose signature fails is only counted.
    let own = header(&block(1, 3, 16));
    let mut non_empty = heartwood_crypto::blocks::factory::from_json(&own, &milestones())
        .unwrap()
        .header()
        .clone();
    non_empty.number_of_transactions = 1;
    non_empty.payload_length = 32;
    let secret = key(1);
    let signing = serialiser::signing_hash(&non_empty);
    non_empty.block_signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &signing,
        secret.secret_key(),
        Aux::fixed([3; 32]),
    )
    .unwrap();
    let non_empty = header_json(&non_empty);
    let mut forged = counted(NetworkStateStatus::Default, 53, 0);
    for (ip, header) in [("192.0.2.4", own.clone()), ("192.0.2.5", non_empty)] {
        forged.quorum_details.over_height_block_headers = Map::from_iter([(ip.to_owned(), header)]);
        assert!(!allowed(&forged));
        assert_eq!(
            lines.all(),
            [
                "INFO: Detected 1 distinct overheight block header",
                "WARNING: Delegate genesis_1 already forged a block. Will not forge",
            ]
        );
        lines.clear();
    }
    let mut unsigned = own;
    unsigned["blockSignature"] = json!("00".repeat(64));
    forged.quorum_details.over_height_block_headers =
        Map::from_iter([("192.0.2.6".to_owned(), unsigned)]);
    assert!(allowed(&forged));
    assert_eq!(
        lines.all(),
        ["INFO: Detected 1 distinct overheight block header"]
    );
}

/// A header's JSON with the fields of a relay's peer status.
fn header_json(header: &BlockHeader) -> Value {
    let id = serialiser::block_id(header).to_hex();
    let hex = heartwood_crypto::utils::hex::encode;
    json!({
        "id": id, "version": header.version, "timestamp": header.timestamp, "previousBlock": hex(&header.previous_block),
        "height": header.height, "numberOfTransactions": header.number_of_transactions,
        "totalAmount": header.total_amount.to_string(), "totalFee": header.total_fee.to_string(),
        "reward": header.reward.to_string(), "payloadLength": header.payload_length,
        "payloadHash": hex(&header.payload_hash), "generatorPublicKey": header.generator_public_key.to_hex(),
        "blockSignature": header.block_signature.to_hex(),
    })
}

#[tokio::test(start_paused = true)]
async fn late_attempts_have_the_reference_lines_and_the_loop_polls_no_faster_than_200_ms() {
    let (lines, _guard) = Lines::capture();
    // The forger's clock is 6.5 s ahead of its relay's at the start of its slot, 10.
    let relay = FakeRelay::new(in_slot(10, 0), &[1, 2], &[]);
    let ahead = Arc::new(ManualClock::new(in_slot(10, 6_500)));
    let mut forger = forger(&relay, &[2], ahead.clone());
    forger.service.boot().await.unwrap();
    assert_eq!(
        forger
            .service
            .forge_attempt(
                &key(2).public_key().to_compressed(),
                true,
                &round(80, 1, &[2], &[], Some(0), None)
            )
            .await,
        AttemptEnd::Late
    );
    assert_eq!(
        lines.with("Failed to forge"),
        [
            "WARNING: Failed to forge new block by delegate genesis_2, because there were 1500ms left in the current slot (less than 2000ms)"
        ]
    );
    ahead.set(in_slot(11, 0));
    lines.clear();
    let round = round(80, 1, &[2], &[], Some(0), None);
    assert_eq!(
        forger
            .service
            .forge_attempt(&key(2).public_key().to_compressed(), false, &round)
            .await,
        AttemptEnd::Late
    );
    assert_eq!(
        lines.with("Failed to forge"),
        ["WARNING: Failed to forge new block by delegate genesis_2, because already in next slot"]
    );
    assert!(forger.signer.calls.lock().unwrap().is_empty());

    // Every wait the loop computes is below zero with the forger's clock a minute ahead: it
    // checks every 200 ms, where the reference would poll without a pause.
    ahead.set(in_slot(18, 0));
    let before = relay.count("round");
    run_for(&forger, 900).await;
    assert_eq!(relay.count("round") - before, 5);
}

#[tokio::test(start_paused = true)]
async fn an_attempt_stops_when_the_slot_has_its_block() {
    let (lines, _guard) = Lines::capture();
    let relay = FakeRelay::new(in_slot(10, 0), &[1, 2], &[]);
    let mut forger = forger(&relay, &[1, 2], relay.clock.clone());
    forger.service.boot().await.unwrap();
    let ours = key(2).public_key().to_compressed();
    let slot_10 = round(80, 1, &[1, 2], &[], Some(0), Some(1));
    // Delegate 1's block of slot 10 is already the tip: the status's generator and slot.
    *relay.status.lock().unwrap() = Some(status(2, 80, &hex(1)));
    let first = key(1).public_key().to_compressed();
    assert_eq!(
        forger.service.forge_attempt(&first, true, &slot_10).await,
        AttemptEnd::Stopped
    );
    assert_eq!(
        lines.with("Not going"),
        [
            "WARNING: Not going to forge because delegate genesis_1 has already forged on another node"
        ]
    );
    lines.clear();
    assert_eq!(
        forger.service.forge_attempt(&first, false, &slot_10).await,
        AttemptEnd::Stopped
    );
    assert!(
        lines.with("Not going").is_empty(),
        "the line is for a slot's first attempt only"
    );
    // Delegate 2 is not the round's forger.
    *relay.status.lock().unwrap() = Some(status(1, 72, &hex(0)));
    assert_eq!(
        forger.service.forge_attempt(&ours, true, &slot_10).await,
        AttemptEnd::Stopped
    );
    assert!(lines.all().iter().all(|line| !line.starts_with("WARNING")));
    // The network state's last generator is compared with the delegate's key, as the reference
    // compares it: a reference relay writes a username there, so for it the clause never holds.
    let mut named = state(1);
    named.last_generator = Some(hex(2));
    named.last_slot_number = Some(10);
    *relay.state.lock().unwrap() = Some(named);
    let slot_10_ours = round(80, 1, &[2, 1], &[], Some(0), Some(1));
    *relay.status.lock().unwrap() = Some(status(1, 80, &hex(0)));
    assert_eq!(
        forger
            .service
            .forge_attempt(&ours, true, &slot_10_ours)
            .await,
        AttemptEnd::Stopped
    );
    assert_eq!(lines.with("Not going").len(), 1);
    assert!(forger.signer.calls.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_height_line_and_the_same_slot_answer() {
    let (lines, _guard) = Lines::capture();
    let relay = FakeRelay::new(in_slot(10, 0), &[1], &[]);
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    let ours = key(1).public_key().to_compressed();
    *relay.state.lock().unwrap() = Some(state(1_234));
    let slot_10 = round(80, 1_000, &[1], &[], Some(0), None);
    assert_eq!(
        forger.service.forge_attempt(&ours, true, &slot_10).await,
        AttemptEnd::Forged
    );
    assert_eq!(
        lines.with("out of sync"),
        [
            "WARNING: The NetworkState height (1,234) and round height (1,000) are out of sync. This indicates delayed blocks on the network"
        ]
    );
    assert_eq!(
        lines.with("forged a new block"),
        ["INFO: Delegate genesis_1 forged a new block at height 1,235 with 0 transactions"]
    );
    // The relay's slot of the block's timestamp is the slot of its last block: nothing is sent.
    let posts = relay.count("post");
    *relay.state.lock().unwrap() = None;
    relay.slots.lock().unwrap().extend([10, 10, 9, 9]);
    let slot_10 = round(80, 1_235, &[1], &[], Some(0), None);
    assert_eq!(
        forger.service.forge_attempt(&ours, false, &slot_10).await,
        AttemptEnd::SameSlot
    );
    assert_eq!(relay.count("post"), posts);
}

#[tokio::test(start_paused = true)]
async fn errors_end_a_slot_after_block_time_less_one_attempts() {
    let (lines, _guard) = Lines::capture();
    let relay = FakeRelay::new(in_slot(10, 0), &[1, 2, 3], &[]);
    let error = communication("p2p.peer.getStatus", "Request timed out");
    relay.fail("status", 100, error.clone());
    let mut forger = forger(&relay, &[2], relay.clock.clone());
    forger.service.boot().await.unwrap();
    lines.clear();
    // Slot 10 is delegate 2's; the attempts run 1 s apart and give up after 7 errors.
    run_for(&forger, 8_000 + 7_900).await;
    assert_eq!(relay.count("status"), 7);
    assert_eq!(
        lines.with("Request timed out"),
        vec![format!("WARNING: {error}").replace(" :bangbang:", ""); 7]
    );
    assert!(forger.signer.calls.lock().unwrap().is_empty());
    assert_eq!(
        relay.count("round"),
        1 + 1 + 7 + 1,
        "boot, the checks of slots 11 and 12, and each attempt's reload"
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_check_sends_forger_failed_and_retries_after_two_seconds() {
    let relay = FakeRelay::new(in_slot(10, 0), &[1, 2], &[]);
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    let mut no_forger = round(80, 1, &[1, 2], &[], None, Some(1));
    relay.rounds.lock().unwrap().push_back(no_forger.clone());
    no_forger.current_forger = Some(wallet(2));
    no_forger.next_forger = None;
    relay.rounds.lock().unwrap().push_back(no_forger);
    run_for(&forger, 4_100).await;
    let failed = "event forger.failed Cannot read properties of undefined (reading 'publicKey')";
    assert_eq!(relay.count(failed), 2, "{:?}", relay.calls());
    assert_eq!(
        relay.count("round"),
        1 + 3,
        "the checks at 0 s, 2 s and 4 s"
    );
}

#[tokio::test(start_paused = true)]
async fn before_the_epoch_the_loop_waits_out_each_slot() {
    let relay = FakeRelay::new(in_slot(-3, 5_000), &[1], &[]);
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    run_for(&forger, 3_000 + 8_000 + 1_000).await;
    // Checks at the boot, at once, then at the ends of slots -3 and -2; nothing is forged before
    // slot 0.
    assert_eq!(relay.count("round"), 4);
    assert_eq!(relay.count("state"), 0);
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_lets_the_post_in_flight_finish_then_the_sessions_close() {
    let relay = FakeRelay::new(in_slot(10, 0), &[1], &[]);
    *relay.post_delay.lock().unwrap() = Duration::from_secs(3);
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    let start = Instant::now();
    run_for(&forger, 1_000).await;
    assert_eq!(
        start.elapsed(),
        Duration::from_secs(3),
        "the loop ended when the post did"
    );
    assert_eq!(relay.count("post 2"), 1);
    assert_eq!(relay.count("dispose"), 0);
    forger.service.dispose().await;
    assert_eq!(relay.calls().last().map(String::as_str), Some("dispose"));
    assert!(forger.service.round().await.is_some());
}

#[tokio::test(start_paused = true)]
async fn a_retry_in_the_slot_resends_the_signed_block() {
    let relay = FakeRelay::new(in_slot(10, 0), &[1, 2], &[]);
    relay.fail("post", 2, communication("p2p.blocks.postBlock", "refused"));
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    run_for(&forger, 5_000).await;
    // Three attempts in slot 10: the first two posts are not taken, the third is; each posted the
    // bytes signed for the slot the first time.
    let calls = forger.signer.calls.lock().unwrap().clone();
    assert_eq!(calls, vec![(hex(1), 10, 10); 3]);
    assert_eq!(relay.count("post"), 3);
    assert_eq!(relay.count("event block.forged"), 3);
    assert_eq!(relay.tip.lock().unwrap().height, 2);
    assert_eq!(forger.signer.signed.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_sync_that_finds_no_host_checks_again_after_two_seconds() {
    let (lines, _guard) = Lines::capture();
    // Slot 14's forger is delegate 5; the forger's delegate 1 forges slot 15.
    let relay = FakeRelay::new(in_slot(14, 0), &[1, 2, 3, 4, 5], &[]);
    let hosts = "127.10.0.1".to_owned();
    relay.fail("sync", 1, RelayError::HostNoResponse { hosts });
    let mut forger = forger(&relay, &[1], relay.clock.clone());
    forger.service.boot().await.unwrap();
    run_for(&forger, 3_000).await;
    assert_eq!(
        lines.with("didn't respond"),
        ["WARNING: 127.10.0.1 didn't respond. Trying again later"]
    );
    assert_eq!(lines.with("due to forge").len(), 2);
    assert_eq!(
        relay.count("round"),
        1 + 2,
        "the boot, and the checks at 0 s and 2 s"
    );
}
