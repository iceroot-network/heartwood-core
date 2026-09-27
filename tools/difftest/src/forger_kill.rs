//! The forger's crash test (`difftest forger crash`): the `forger-child` binary against a mock
//! relay on a test network with a 3 s block time, killed with SIGKILL at random moments, most of
//! them between the signing record's write (which the child reports on its output) and the relay's
//! answer to its `postBlock`. The mock relay makes the child's delegate the forger of every slot
//! and holds each `postBlock` answer for a random time.
//!
//! After every restart no slot may have two different blocks at the mock relay, and a restart
//! inside a recorded slot must send the recorded bytes again.
//!
//! The mock relay is this process, over the child's standard input and output (see the child's
//! documentation for the lines). Its round names its own current slot, and a new previous block
//! and reward at every request, so a block signed again for a slot would differ from the first;
//! the child signs with a fresh random aux too. The network is the committed corpus's
//! (`txgen-one-node`) with its block time set to 3 s.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use heartwood_crypto::crypto::slots;
use heartwood_crypto::genesis::derive;
use heartwood_crypto::utils::hex;
use serde_json::{Value, json};

use crate::errors::{Error, Result};
use crate::report::{CaseResult, Outcome, Report};
use crate::store::ScratchDir;
use crate::synthetic::{DERIVATION_TAG, DEVNET_SEED};

/// What a crash test runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillOptions {
    /// The number of kills.
    pub kills: u32,
    /// The seed of the kill moments and the relay's delays.
    pub seed: u64,
    /// The `forger-child` binary.
    pub child: PathBuf,
}

/// The test network's block time, in seconds.
const BLOCK_TIME: u64 = 3;
/// The share of kills made while the relay holds a `postBlock` answer, in percent.
const AFTER_RECORD: u64 = 60;
/// The longest hold of a `postBlock` answer.
const HOLD_MS: u64 = 1_500;
/// How long the child may take to start, and to post once its round is answered.
const PATIENCE: Duration = Duration::from_secs(10);

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
    /// While the relay holds the answer to its next `postBlock`.
    AfterRecord,
    /// This long after it is ready.
    At(Duration),
}

/// What the mock relay saw.
#[derive(Default)]
struct Seen {
    /// Per slot, the id of the first block posted.
    posted: BTreeMap<i64, String>,
    /// Per slot, the id the child reported recording.
    recorded: BTreeMap<i64, String>,
    /// Blocks of a recorded slot sent again after a restart.
    resent_after_restart: usize,
    /// Kills made while the relay held a `postBlock` answer.
    held_kills: usize,
    /// The longest sign-and-record call the child reported, in milliseconds.
    slowest_ms: u64,
    /// Slots the child's signer refused.
    refused: usize,
    /// The problems found.
    problems: Vec<(i64, Outcome)>,
}

/// Run the crash test of `options`.
pub fn run(options: &KillOptions) -> Result<Report> {
    let scratch = ScratchDir::new("forger-crash")?;
    let (config, data) = (scratch.path().join("config"), scratch.path().join("data"));
    let epoch = write_config(&config)?;
    let mut moments = Moments(options.seed);
    let mut seen = Seen::default();
    for _ in 0..options.kills {
        let kill = if moments.below(100) < AFTER_RECORD {
            Kill::AfterRecord
        } else {
            Kill::At(Duration::from_millis(moments.below(BLOCK_TIME * 1_000)))
        };
        let mut child = Command::new(&options.child)
            .args([Path::new("--config"), &config, Path::new("--data"), &data])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::io(&options.child, e))?;
        let outcome = drive(&mut child, kill, epoch, &mut moments, &mut seen);
        let _ = child.kill();
        let _ = child.wait();
        outcome?;
    }
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
    Ok(Report {
        title: "Forger crash test".into(),
        source: format!(
            "{} kills ({} while a postBlock answer was held), seed {}; {} blocks of a recorded \
             slot sent again after a restart; {} slots posted, {} refused; slowest \
             sign-and-record {} ms",
            options.kills,
            seen.held_kills,
            options.seed,
            seen.resent_after_restart,
            seen.posted.len(),
            seen.refused,
            seen.slowest_ms
        ),
        files: 1,
        results,
    })
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

/// Serve one run of the child until it is killed as `kill` says.
fn drive(
    child: &mut Child,
    kill: Kill,
    epoch: i64,
    moments: &mut Moments,
    seen: &mut Seen,
) -> Result<()> {
    let lines = read_lines(child)?;
    let mut input = child.stdin.take().ok_or_else(|| usage("no child input"))?;
    let ready = lines
        .recv_timeout(PATIENCE)
        .map_err(|_| usage("the child did not start"))?;
    if ready.get("ready").is_none() {
        return Err(usage(format!("the child did not start: {ready}")));
    }
    let started = Instant::now();
    let deadline = |kill: Kill| match kill {
        Kill::AfterRecord => started + PATIENCE,
        Kill::At(after) => started + after,
    };
    let mut first_round = true;
    loop {
        let left = deadline(kill).saturating_duration_since(Instant::now());
        let message = match lines.recv_timeout(left) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => return Ok(()),
            Err(RecvTimeoutError::Disconnected) => return Err(usage("the child stopped")),
        };
        if message.get("call") == Some(&json!("round")) {
            answer(&mut input, &round(epoch, moments))?;
        } else if let Some(recorded) = message.get("recorded") {
            let (slot, id) = slot_and_id(recorded);
            seen.slowest_ms = seen.slowest_ms.max(recorded["ms"].as_u64().unwrap_or(0));
            if let Some(earlier) = seen.recorded.insert(slot, id.clone())
                && earlier != id
            {
                seen.problems
                    .push((slot, differs("recorded", &earlier, &id)));
            }
            first_round = false;
        } else if let Some(resent) = message.get("resent") {
            let (slot, id) = slot_and_id(resent);
            if first_round && seen.recorded.contains_key(&slot) {
                seen.resent_after_restart += 1;
            }
            match seen.recorded.get(&slot) {
                Some(recorded) if *recorded != id => {
                    seen.problems
                        .push((slot, differs("re-sent", recorded, &id)));
                }
                _ => {}
            }
            first_round = false;
        } else if message.get("refused").is_some() {
            seen.refused += 1;
            first_round = false;
        } else if message.get("call") == Some(&json!("post")) {
            let (slot, id) = slot_and_id(&message);
            let posted = seen.posted.entry(slot).or_insert_with(|| id.clone());
            if *posted != id {
                let problem = differs("posted", posted, &id);
                seen.problems.push((slot, problem));
            }
            // The relay holds its answer; an AfterRecord kill comes during the hold.
            let hold = Duration::from_millis(moments.below(HOLD_MS));
            if kill == Kill::AfterRecord {
                std::thread::sleep(hold);
                seen.held_kills += 1;
                return Ok(());
            }
            std::thread::sleep(hold.min(deadline(kill).saturating_duration_since(Instant::now())));
            if Instant::now() >= deadline(kill) {
                return Ok(());
            }
            answer(&mut input, &json!({"posted": true}))?;
        } else {
            return Err(usage(format!("the child sent {message}")));
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

/// The relay's round now: its own slot, a new previous block and reward, no transactions.
fn round(epoch: i64, moments: &mut Moments) -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let slot = slots::slot_number(slots::epoch_time(now, epoch), BLOCK_TIME);
    let mut previous = [0u8; 32];
    for chunk in previous.chunks_mut(8) {
        chunk.copy_from_slice(&moments.next().to_le_bytes());
    }
    json!({
        "slot": slot,
        "previousId": hex::encode(&previous),
        "previousHeight": slot,
        "timestamp": slots::slot_time(slot, BLOCK_TIME),
        "reward": moments.below(1_000_000_000).to_string(),
        "transactions": [],
    })
}

fn answer(input: &mut ChildStdin, message: &Value) -> Result<()> {
    writeln!(input, "{message}")
        .and_then(|()| input.flush())
        .map_err(|e| usage(format!("the child's input: {e}")))
}

/// The child's output lines as JSON, read on a thread of their own.
fn read_lines(child: &mut Child) -> Result<Receiver<Value>> {
    let output = child
        .stdout
        .take()
        .ok_or_else(|| usage("no child output"))?;
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output)
            .lines()
            .map_while(std::io::Result::ok)
        {
            let value = serde_json::from_str(&line).unwrap_or(Value::String(line));
            if send.send(value).is_err() {
                return;
            }
        }
    });
    Ok(receive)
}

fn usage(message: impl Into<String>) -> Error {
    Error::Usage(format!("forger crash: {}", message.into()))
}

/// The child's configuration: the committed corpus's crypto files with a 3 s block time, and a
/// private key file with the seed's first delegate. Returns the network's epoch in milliseconds.
fn write_config(config: &Path) -> Result<i64> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/txgen-one-node/crypto");
    let crypto = config.join("crypto");
    std::fs::create_dir_all(&crypto).map_err(|e| Error::io(&crypto, e))?;
    let mut epoch = None;
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
                epoch =
                    epoch.or_else(|| milestone["epoch"].as_str().and_then(slots::epoch_unix_ms));
            }
            text = serde_json::to_string_pretty(&milestones).unwrap_or_default();
        }
        let target = crypto.join(file);
        std::fs::write(&target, text).map_err(|e| Error::io(&target, e))?;
    }
    let delegate = derive::role_key(DERIVATION_TAG, "delegate/1", DEVNET_SEED, 90)
        .map_err(|e| usage(format!("the delegate's key: {e}")))?;
    let keys = json!({"keys": [delegate.keys.secret_key().to_hex().as_str()]}).to_string();
    let file = config.join("delegates.json");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut out = options.open(&file).map_err(|e| Error::io(&file, e))?;
    out.write_all(keys.as_bytes())
        .map_err(|e| Error::io(&file, e))?;
    epoch.ok_or_else(|| usage("the milestones have no epoch"))
}
