//! The forger's crash test (`difftest forger crash`): the `forger-child` binary, which runs the
//! forger's slot loop, relay client and block signer, against a mock relay on [`RELAY_IP`] on a
//! test network with a 3 s block time, killed with SIGKILL at random moments, most of them while
//! the relay holds the answer to a `postBlock`, so after the signing record's write.
//!
//! The mock relay answers the forger's calls from the time: the child's delegate is the forger of
//! every slot, and every network state names a new random previous block and every round a new
//! reward, so a block signed again for a slot would differ from the first (as it would with the
//! child's fresh random aux alone). A posted block becomes the relay's last block once its answer
//! is sent, after a random hold; in a run that is to be killed during a hold, the answer is never
//! sent.
//!
//! After every restart no slot may have two different blocks at the mock relay or in the child's
//! reports, a block posted for a recorded slot must be the recorded one, and a restart inside a
//! recorded slot whose block the relay has not taken sends the recorded bytes again when it sends
//! at all (it sends nothing once less than 2,000 ms of the slot are left). Such a restart whose
//! first status request (the forger takes the time left just before it) comes with at least
//! 2,000 ms of the slot left, while the relay has not taken the slot's block, and that is not
//! killed within 500 ms of it, must send its block again; and at least one restart must.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use heartwood_crypto::crypto::slots;
use heartwood_crypto::genesis::derive;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, Milestones};
use heartwood_kernel::config::crypto;
use heartwood_p2p::testing::{Answer, MockPeer, Reply};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::{blocks, transactions};
use heartwood_p2p_wire::internal::{
    self, CurrentRound, DelegateWallet, InternalBody, LastBlockData, NetworkState,
    NetworkStateStatus, QuorumDetails, SlotNumber, SlotNumberRequest,
};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
use heartwood_p2p_wire::status::{
    self, NetworkConfig, PeerConfig, SignedState, StatusHeaderData, TokenConfig,
};
use serde_json::{Map, Value, json};

use crate::errors::{Error, Result};
use crate::report::{CaseResult, Outcome, Report};
use crate::store::ScratchDir;
use crate::synthetic::{DERIVATION_TAG, DEVNET_SEED};

/// What a crash test runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillOptions {
    /// The number of kills.
    pub kills: u32,
    /// The seed of the kill moments and the relay's values.
    pub seed: u64,
    /// The `forger-child` binary.
    pub child: PathBuf,
}

/// The mock relay's address, in the loopback block the crash test uses.
pub const RELAY_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 14, 0, 1));
/// The test network's block time, in seconds.
const BLOCK_TIME: u64 = 3;
/// The share of kills made while the relay holds a `postBlock` answer, in percent.
const AFTER_RECORD: u64 = 60;
/// The longest hold of a `postBlock` answer, and of a kill after a held post.
const HOLD_MS: u64 = 1_500;
/// How long the child may take to start, and to post once started.
const PATIENCE: Duration = Duration::from_secs(10);
/// How long stray lines and posts of a killed child are waited for.
const SETTLE: Duration = Duration::from_millis(100);
/// The time a forger keeps before a slot's end: it sends nothing with less left.
const MINIMUM_MS: i64 = 2_000;
/// How long a child may take from its status request to its signer's outcome: the slot calls
/// and a re-send, which writes nothing.
const SEND_MS: i64 = 500;

/// SplitMix64: the kill moments and the relay's values, reproducible from the seed.
struct Moments(u64);

impl Moments {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value below `bound` (at least 1).
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
}

/// When the running child is killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kill {
    /// While the relay holds the answer to its first `postBlock`.
    AfterRecord,
    /// This long after it is ready.
    At(Duration),
}

/// What reaches the harness: a line of a run's output, the end of it, or a block posted to the
/// relay.
enum Event {
    Line(u32, Value),
    Closed(u32),
    Posted {
        slot: i64,
        id: String,
    },
    /// A status request answered at `at`, and the slot of the block the relay had taken then.
    Status {
        at: i64,
        taken: Option<i64>,
    },
}

/// What the harness saw.
#[derive(Default)]
struct Seen {
    /// Per slot, the id of the first block posted.
    posted: BTreeMap<i64, String>,
    /// Per slot, the id the child reported recording.
    recorded: BTreeMap<i64, String>,
    /// Restarts inside a recorded slot whose block the relay had not taken.
    restarts_in_recorded_slot: usize,
    /// Of those, the ones whose first block was the recorded one, sent again.
    resent_after_restart: usize,
    /// Of those, the ones that started with less than [`MINIMUM_MS`] of the slot left.
    restarts_late: usize,
    /// Of those, the ones whose first status request came with at least [`MINIMUM_MS`] of the
    /// slot left and which ran [`SEND_MS`] after it, each of which must send its block again.
    restarts_due: usize,
    /// Kills made while the relay held a `postBlock` answer.
    held_kills: usize,
    /// The longest sign-and-record call the child reported, in milliseconds.
    slowest_ms: u64,
    /// Slots the child's signer refused.
    refused: usize,
    /// The problems found.
    problems: Vec<(i64, Outcome)>,
}

/// A run of the child: its number, and the recorded slot it restarted in, if any.
struct Run {
    number: u32,
    in_recorded_slot: Option<i64>,
    /// Whether the signer has reported an outcome in this run.
    signed: bool,
    /// When it was ready (`i64::MAX` before), when it was killed, and its first status request
    /// after it was ready, with the slot of the block the relay had taken then.
    ready_at: i64,
    killed_at: i64,
    status: Option<(i64, Option<i64>)>,
    /// Whether its first outcome was the recorded slot's block, sent again.
    resent: bool,
}

/// Run the crash test of `options`.
pub fn run(options: &KillOptions) -> Result<Report> {
    let scratch = ScratchDir::new("forger-crash")?;
    let (config, data) = (scratch.path().join("config"), scratch.path().join("data"));
    let (milestones, public_key) = write_config(&config)?;
    let (sender, events) = mpsc::channel();
    let relay = Arc::new(Relay::new(
        milestones,
        &public_key,
        options.seed,
        sender.clone(),
    )?);
    let server = RelayServer::start(relay.clone())?;
    let mut moments = Moments(options.seed);
    let mut seen = Seen::default();
    let mut outcome = Ok(());
    for number in 0..options.kills {
        let kill = if moments.below(100) < AFTER_RECORD {
            Kill::AfterRecord
        } else {
            Kill::At(Duration::from_millis(moments.below(BLOCK_TIME * 1_000)))
        };
        relay.state().hold_posts = kill == Kill::AfterRecord;
        let mut child = match Command::new(&options.child)
            .args([Path::new("--config"), &config, Path::new("--data"), &data])
            .arg("--relay")
            .arg(server.address.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                outcome = Err(Error::io(&options.child, error));
                break;
            }
        };
        let reader = read_lines(&mut child, number, sender.clone());
        let mut run = Run {
            number,
            in_recorded_slot: None,
            signed: false,
            ready_at: i64::MAX,
            killed_at: i64::MAX,
            status: None,
            resent: false,
        };
        let driven = drive(&events, kill, &relay, &mut run, &mut moments, &mut seen);
        run.killed_at = now_ms();
        let _ = child.kill();
        let _ = child.wait();
        if let Some(reader) = reader {
            let _ = reader.join();
        }
        // The killed child's last lines, and a post still on its way to the relay.
        while let Ok(event) = events.recv_timeout(SETTLE) {
            handle(event, &mut run, &mut seen);
        }
        if let (Some(slot), Some((at, taken))) = (run.in_recorded_slot, run.status) {
            let due = relay.slot(at) == slot
                && taken != Some(slot)
                && relay.slot_end(slot).saturating_sub(at) >= MINIMUM_MS
                && run.killed_at >= at.saturating_add(SEND_MS);
            seen.restarts_due += usize::from(due);
            if due && !run.resent {
                let left = relay.slot_end(slot).saturating_sub(at);
                let message = format!(
                    "a restart that asked for the status with {left} ms of recorded slot {slot} \
                     left did not send its block again"
                );
                seen.problems.push((slot, Outcome::HarnessError(message)));
            }
        }
        if let Err(error) = driven {
            outcome = Err(error);
            break;
        }
    }
    server.stop();
    outcome?;
    Ok(report(options, seen))
}

/// The report of a finished test.
fn report(options: &KillOptions, mut seen: Seen) -> Report {
    let mut results: Vec<CaseResult> = (seen.posted.keys())
        .map(|slot| result(*slot, Outcome::Pass))
        .collect();
    results.extend(
        seen.problems
            .drain(..)
            .map(|(slot, outcome)| result(slot, outcome)),
    );
    if options.kills >= 5 && seen.resent_after_restart == 0 {
        let message = "no restart inside a recorded slot sent its block again".to_owned();
        results.push(result(0, Outcome::HarnessError(message)));
    }
    Report {
        title: "Forger crash test".into(),
        source: format!(
            "{} kills ({} while a postBlock answer was held), seed {}; {} restarts inside a \
             recorded slot the relay had not taken: {} sent the recorded block again, {} sent \
             nothing ({} of them started with less than {} ms of the slot left); {} asked for the \
             status with at least {} ms left and ran {} ms more, each of which must send it \
             again; {} slots posted, {} refused; slowest sign-and-record {} ms",
            options.kills,
            seen.held_kills,
            options.seed,
            seen.restarts_in_recorded_slot,
            seen.resent_after_restart,
            seen.restarts_in_recorded_slot - seen.resent_after_restart,
            seen.restarts_late,
            MINIMUM_MS,
            seen.restarts_due,
            MINIMUM_MS,
            SEND_MS,
            seen.posted.len(),
            seen.refused,
            seen.slowest_ms
        ),
        files: 1,
        results,
    }
}

fn result(slot: i64, outcome: Outcome) -> CaseResult {
    CaseResult {
        file: "forger crash".into(),
        line: usize::try_from(slot).unwrap_or(0),
        op: "forger.crash".into(),
        label: format!("slot {slot}"),
        height: u64::try_from(slot).unwrap_or(0),
        outcome,
    }
}

/// Serve one run of the child until it is to be killed as `kill` says.
fn drive(
    events: &Receiver<Event>,
    kill: Kill,
    relay: &Relay,
    run: &mut Run,
    moments: &mut Moments,
    seen: &mut Seen,
) -> Result<()> {
    let waiting = Instant::now() + PATIENCE;
    loop {
        let left = waiting.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(Event::Line(number, line))
                if number == run.number && line.get("ready").is_some() =>
            {
                break;
            }
            Ok(Event::Closed(number)) if number == run.number => {
                return Err(usage("the child stopped before it was ready"));
            }
            Ok(event) => handle(event, run, seen),
            Err(_) => return Err(usage("the child did not start")),
        }
    }
    let now = now_ms();
    run.ready_at = now;
    let slot = relay.slot(now);
    let taken = relay.state().taken(now).map(|tip| tip.slot);
    if seen.recorded.contains_key(&slot) && taken != Some(slot) {
        seen.restarts_in_recorded_slot += 1;
        run.in_recorded_slot = Some(slot);
        if relay.slot_end(slot).saturating_sub(now) < MINIMUM_MS {
            seen.restarts_late += 1;
        }
    }
    let started = Instant::now();
    let mut deadline = match kill {
        Kill::AfterRecord => started + PATIENCE,
        Kill::At(after) => started + after,
    };
    let mut held = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = match events.recv_timeout(left) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => return Err(usage("the relay stopped")),
        };
        if matches!(event, Event::Closed(number) if number == run.number) {
            return Err(usage("the child stopped"));
        }
        let posted = matches!(event, Event::Posted { .. });
        handle(event, run, seen);
        if posted && kill == Kill::AfterRecord && !held {
            held = true;
            deadline = Instant::now() + Duration::from_millis(moments.below(HOLD_MS));
        }
    }
    if held {
        seen.held_kills += 1;
    }
    Ok(())
}

/// Take in one event of `run`, or a late one of an earlier run.
fn handle(event: Event, run: &mut Run, seen: &mut Seen) {
    match event {
        Event::Posted { slot, id } => {
            let posted = seen.posted.entry(slot).or_insert_with(|| id.clone());
            if *posted != id {
                let problem = differs("posted", posted, &id);
                seen.problems.push((slot, problem));
            }
            if let Some(recorded) = seen.recorded.get(&slot)
                && *recorded != id
            {
                seen.problems.push((slot, differs("posted", recorded, &id)));
            }
        }
        Event::Status { at, taken } => {
            if run.status.is_none() && at >= run.ready_at {
                run.status = Some((at, taken));
            }
        }
        Event::Closed(_) => {}
        Event::Line(number, line) => {
            let current = number == run.number;
            if let Some(recorded) = line.get("recorded") {
                let (slot, id) = slot_and_id(recorded);
                seen.slowest_ms = seen.slowest_ms.max(recorded["ms"].as_u64().unwrap_or(0));
                let earlier = seen.recorded.insert(slot, id.clone());
                for earlier in earlier.iter().chain(seen.posted.get(&slot)) {
                    if *earlier != id {
                        seen.problems
                            .push((slot, differs("recorded", earlier, &id)));
                    }
                }
            } else if let Some(resent) = line.get("resent") {
                let (slot, id) = slot_and_id(resent);
                match seen.recorded.get(&slot) {
                    Some(recorded) if *recorded != id => {
                        seen.problems
                            .push((slot, differs("re-sent", recorded, &id)));
                    }
                    Some(_) if current && !run.signed && run.in_recorded_slot == Some(slot) => {
                        seen.resent_after_restart += 1;
                        run.resent = true;
                    }
                    _ => {}
                }
            } else if line.get("refused").is_some() {
                seen.refused += 1;
            } else if line.get("ready").is_none() {
                let message = format!("the child wrote {line}");
                seen.problems.push((0, Outcome::HarnessError(message)));
            }
            if current && line.get("ready").is_none() {
                run.signed = true;
            }
        }
    }
}

fn slot_and_id(value: &Value) -> (i64, String) {
    let slot = value["slot"].as_i64().unwrap_or(i64::MIN);
    (slot, value["id"].as_str().unwrap_or_default().to_owned())
}

fn differs(what: &str, first: &str, then: &str) -> Outcome {
    Outcome::HarnessError(format!(
        "two different blocks for one slot: {first}, then {what} {then}"
    ))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The child's output lines as JSON, read on a thread of their own until the output closes.
fn read_lines(child: &mut Child, number: u32, events: Sender<Event>) -> Option<JoinHandle<()>> {
    let output = child.stdout.take()?;
    Some(std::thread::spawn(move || {
        for line in BufReader::new(output)
            .lines()
            .map_while(std::io::Result::ok)
        {
            let value = serde_json::from_str(&line).unwrap_or(Value::String(line));
            if events.send(Event::Line(number, value)).is_err() {
                return;
            }
        }
        let _ = events.send(Event::Closed(number));
    }))
}

fn usage(message: impl Into<String>) -> Error {
    Error::Usage(format!("forger crash: {}", message.into()))
}

/// The last block the relay took: a posted block, from the moment its answer is sent.
#[derive(Clone)]
struct Tip {
    slot: i64,
    header: StatusHeaderData,
    taken_at: i64,
}

/// What the mock relay keeps between answers.
struct RelayState {
    values: Moments,
    /// Whether posts are held without an answer (a run killed during a hold).
    hold_posts: bool,
    tip: Option<Tip>,
}

impl RelayState {
    /// The last block taken by `now`.
    fn taken(&self, now: i64) -> Option<&Tip> {
        self.tip.as_ref().filter(|tip| tip.taken_at <= now)
    }
}

/// The mock relay's answers.
struct Relay {
    milestones: Milestones,
    epoch: i64,
    wallet: DelegateWallet,
    public_key: String,
    config: PeerConfig,
    posts: Mutex<Sender<Event>>,
    state: Mutex<RelayState>,
}

impl Relay {
    fn new(
        milestones: Milestones,
        public_key: &str,
        seed: u64,
        posts: Sender<Event>,
    ) -> Result<Relay> {
        let epoch = slots::epoch_unix_ms(milestones.at(1).epoch())
            .ok_or_else(|| usage("the milestones' epoch does not read"))?;
        let wallet = object(json!({
            "address": "",
            "publicKey": public_key,
            "balance": "0",
            "nonce": "0",
            "delegate": { "username": "genesis_1", "voteBalance": "0", "rank": 1 },
        }));
        let wallet = DelegateWallet::new(wallet).map_err(|e| usage(e.to_string()))?;
        let config = PeerConfig {
            version: "4.3.1".into(),
            network: NetworkConfig {
                version: 90,
                name: "devnet".into(),
                nethash: "0".repeat(64),
                explorer: String::new(),
                token: TokenConfig {
                    name: "dROOT".into(),
                    symbol: "dRT".into(),
                },
            },
            plugins: Vec::new(),
        };
        Ok(Relay {
            milestones,
            epoch,
            wallet,
            public_key: public_key.to_owned(),
            config,
            posts: Mutex::new(posts),
            state: Mutex::new(RelayState {
                values: Moments(seed ^ 0x5eed_0f7e_1a70_0000),
                hold_posts: false,
                tip: None,
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, RelayState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The relay's slot at `now` (Unix milliseconds).
    fn slot(&self, now: i64) -> i64 {
        slots::slot_number(slots::epoch_time(now, self.epoch), BLOCK_TIME)
    }

    /// The end of `slot`, in Unix milliseconds.
    fn slot_end(&self, slot: i64) -> i64 {
        let start = slots::slot_time(slot.saturating_add(1), BLOCK_TIME);
        self.epoch.saturating_add(start.saturating_mul(1_000))
    }

    fn answer(&self, route: RouteId, payload: &[u8]) -> Reply {
        let now = now_ms();
        let slot = self.slot(now);
        let height = u32::try_from(slot.max(1)).unwrap_or(u32::MAX);
        let mut state = self.state();
        match route {
            RouteId::GetCurrentRound => {
                let reward = state.values.below(1_000_000_000).to_string();
                let last_block = object(json!({ "height": height, "id": "0".repeat(64) }));
                let Ok(last_block) = LastBlockData::new(last_block) else {
                    return Reply::Silence;
                };
                json_reply(&CurrentRound {
                    all_delegates: vec![self.wallet.clone()],
                    can_forge: true,
                    current: 1,
                    current_forger: Some(self.wallet.clone()),
                    delegates: vec![self.wallet.clone()],
                    last_block,
                    next_forger: Some(self.wallet.clone()),
                    reward,
                    timestamp: slots::slot_time(slot, BLOCK_TIME),
                })
            }
            RouteId::GetSlotNumber => {
                let asked = internal::decode::<SlotNumberRequest>(payload).ok();
                let timestamp = asked.and_then(|request| request.timestamp);
                let timestamp = timestamp.unwrap_or_else(|| slots::epoch_time(now, self.epoch));
                json_reply(&SlotNumber(slots::slot_number(timestamp, BLOCK_TIME)))
            }
            RouteId::GetNetworkState => {
                let mut previous = [0u8; 32];
                for chunk in previous.chunks_mut(8) {
                    chunk.copy_from_slice(&state.values.next().to_le_bytes());
                }
                json_reply(&NetworkState {
                    status: NetworkStateStatus::Default,
                    node_height: Some(height),
                    last_block_id: Some(hex::encode(&previous)),
                    last_generator: None,
                    last_slot_number: Some(slot.saturating_sub(1)),
                    quorum_details: QuorumDetails {
                        has_quorum: 53,
                        ..QuorumDetails::default()
                    },
                })
            }
            RouteId::GetStatus => {
                let taken = state.taken(now).map(|tip| tip.slot);
                let _ = lock(&self.posts).send(Event::Status { at: now, taken });
                let header = match state.taken(now) {
                    Some(tip) => tip.header.clone(),
                    None => self.earlier_block(slot, height),
                };
                let signed = SignedState {
                    height: header.height,
                    forging_allowed: true,
                    current_slot: slot,
                    header,
                };
                Reply::Payload(status::response(&signed, &self.config, &[]).encode())
            }
            RouteId::GetUnconfirmedTransactions => Reply::Payload(
                transactions::get_unconfirmed_transactions_encode_response(&Default::default()),
            ),
            RouteId::PostBlock => {
                let posted = blocks::post_block_decode_request(payload).ok();
                let bytes = posted.and_then(|request| request.block).unwrap_or_default();
                let Ok(block) = Block::decode_trusted(&bytes, &self.milestones) else {
                    return Reply::Error {
                        status: 400,
                        payload: br#"{"error":"Bad Request","message":"not a block"}"#.to_vec(),
                    };
                };
                let header = StatusHeaderData::from_block(&block);
                let block_slot = slots::slot_number(i64::from(header.timestamp), BLOCK_TIME);
                let event = Event::Posted {
                    slot: block_slot,
                    id: header.id.clone(),
                };
                let _ = lock(&self.posts).send(event);
                if state.hold_posts {
                    return Reply::Silence;
                }
                let hold = state.values.below(HOLD_MS);
                let answer = PostBlockResponse {
                    status: Some(true),
                    height: Some(header.height),
                    ..PostBlockResponse::default()
                };
                state.tip = Some(Tip {
                    slot: block_slot,
                    header,
                    taken_at: now.saturating_add(i64::try_from(hold).unwrap_or(0)),
                });
                let answer = Reply::Payload(blocks::post_block_encode_response(&answer));
                Reply::After(Duration::from_millis(hold), Box::new(answer))
            }
            _ => Reply::Silence,
        }
    }

    /// The relay's last block when it has taken none of the child's: one of the slot before.
    fn earlier_block(&self, slot: i64, height: u32) -> StatusHeaderData {
        let timestamp = slots::slot_time(slot.saturating_sub(1), BLOCK_TIME);
        StatusHeaderData {
            id: "0".repeat(64),
            version: 0,
            timestamp: u32::try_from(timestamp).unwrap_or(0),
            previous_block: "0".repeat(64),
            height,
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward: 0,
            payload_length: 0,
            payload_hash: "00".repeat(32),
            generator_public_key: self.public_key.clone(),
            block_signature: "00".repeat(64),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn json_reply<T: InternalBody>(body: &T) -> Reply {
    Reply::Payload(internal::encode(body))
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(fields) => fields,
        _ => Map::new(),
    }
}

/// The mock relay's listener, on a thread and runtime of its own.
struct RelayServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl RelayServer {
    fn start(relay: Arc<Relay>) -> Result<RelayServer> {
        let stop = Arc::new(AtomicBool::new(false));
        let (started, address) = mpsc::channel();
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = started.send(Err(error.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let answer: Answer = Arc::new(move |route, payload| relay.answer(route, payload));
                let peer = match MockPeer::answering((RELAY_IP, 0).into(), answer).await {
                    Ok(peer) => peer,
                    Err(error) => {
                        let _ = started.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = started.send(Ok(peer.address()));
                while !stopping.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                drop(peer);
            });
        });
        let address = address
            .recv()
            .map_err(|_| usage("the mock relay did not start"))?
            .map_err(|error| usage(format!("the mock relay: {error}")))?;
        Ok(RelayServer {
            address,
            stop,
            thread,
        })
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.thread.join();
    }
}

/// The child's configuration: the committed corpus's crypto files with a 3 s block time, and a
/// private key file with the seed's first delegate. Returns the milestones and the delegate's
/// public key.
fn write_config(config: &Path) -> Result<(Milestones, String)> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/txgen-one-node/crypto");
    let crypto_dir = config.join("crypto");
    std::fs::create_dir_all(&crypto_dir).map_err(|e| Error::io(&crypto_dir, e))?;
    for file in [
        "network.json",
        "genesisBlock.json",
        "exceptions.json",
        "milestones.json",
    ] {
        let path = source.join(file);
        let mut text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        if file == "milestones.json" {
            let mut milestones: Value =
                serde_json::from_str(&text).map_err(|e| usage(format!("{file}: {e}")))?;
            for milestone in milestones.as_array_mut().into_iter().flatten() {
                if let Some(time) = milestone.get_mut("blockTime") {
                    *time = json!(BLOCK_TIME);
                }
            }
            text = serde_json::to_string_pretty(&milestones).unwrap_or_default();
        }
        let target = crypto_dir.join(file);
        std::fs::write(&target, text).map_err(|e| Error::io(&target, e))?;
    }
    let files = crypto::load(&crypto_dir).map_err(|e| usage(format!("the crypto files: {e}")))?;
    let delegate = derive::role_key(DERIVATION_TAG, "delegate/1", DEVNET_SEED, 90)
        .map_err(|e| usage(format!("the delegate's key: {e}")))?;
    let public_key = delegate.keys.public_key().to_compressed().to_hex();
    let keys = json!({"keys": [delegate.keys.secret_key().to_hex().as_str()]}).to_string();
    let file = config.join("delegates.json");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut out = options.open(&file).map_err(|e| Error::io(&file, e))?;
    out.write_all(keys.as_bytes())
        .map_err(|e| Error::io(&file, e))?;
    Ok((files.milestones, public_key))
}
