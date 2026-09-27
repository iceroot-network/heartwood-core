//! The crash test's child (feature `difftest`): a forger over a configuration directory, driven
//! by the harness's mock relay, which kills it with SIGKILL at random moments.
//!
//!   forger-child --config DIR --data DIR
//!
//! The configuration directory holds `delegates.json` and the four crypto files in `crypto/`; the
//! signing record goes to the data directory. The child talks to the mock relay in JSON lines, its
//! requests on its standard output and the relay's answers on its standard input:
//!
//! - `{"ready": {"publicKey"}}` once the keys, the crypto files and the record are loaded;
//! - at every slot, `{"call": "round"}`, answered with the round (`slot`, `previousId`,
//!   `previousHeight`, `timestamp`, `reward`, `transactions` as hex);
//! - the block signer's outcome for its first delegate: `{"recorded": {slot, id, ms}}` once a new
//!   block is recorded, `{"resent": {slot, id}}` for a block the record already held, or
//!   `{"refused": {slot, error}}`;
//! - for a block, `{"call": "post", "slot", "id", "hex"}`, answered with `{"posted": true}` when
//!   the relay chooses.
//!
//! Until the forger's loop joins it, the child drives the block signer once per slot of its own
//! clock from the relay's round. It ends when its standard input closes.

#![forbid(unsafe_code)]

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockId, Transaction};
use heartwood_forger::signing_record::{self, SigningRecord};
use heartwood_forger::{BlockSigner, ForgeInputs, SignedOutcome, Signer, load_keys};
use heartwood_kernel::config::crypto;
use heartwood_kernel::{Clock, SystemClock};
use serde_json::{Value, json};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("forger-child: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (mut config, mut data) = (None, None);
    while let Some(flag) = args.next() {
        let value = args.next().map(PathBuf::from);
        match flag.as_str() {
            "--config" => config = value,
            "--data" => data = value,
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    let usage = || "usage: forger-child --config DIR --data DIR".to_owned();
    let (config, data) = (config.ok_or_else(usage)?, data.ok_or_else(usage)?);
    let files = crypto::load(&config.join("crypto")).map_err(|e| e.to_string())?;
    let milestones = files.milestones;
    let keys = load_keys(&config).map_err(|e| e.to_string())?;
    let delegates = keys.delegates(files.network.pub_key_hash);
    let key = *delegates.first().ok_or("no delegate")?.public_key();
    let path = signing_record::path(&data);
    let record = SigningRecord::open(&path, &files.network.nethash).map_err(|e| e.to_string())?;
    // A fresh random aux for every signature: only the record keeps a slot's bytes the same.
    let mut signer = Signer::new(delegates, record, Aux::random(), milestones.clone());
    let first = milestones.at(1);
    let epoch = slots::epoch_unix_ms(first.epoch()).ok_or("the epoch does not read")?;
    let block_time = first.block_time();
    let own_slot =
        || slots::slot_number(slots::epoch_time(SystemClock.now_ms(), epoch), block_time);
    let mut relay = Relay::new();
    relay.send(&json!({"ready": {"publicKey": key.to_hex()}}))?;
    loop {
        let slot = own_slot();
        let Some(round) = relay.call(json!({"call": "round"}))? else {
            return Ok(());
        };
        let number = |name: &str| {
            round
                .get(name)
                .and_then(Value::as_u64)
                .ok_or(format!("no {name}"))
        };
        let previous_height =
            u32::try_from(number("previousHeight")?).map_err(|e| e.to_string())?;
        let params = milestones.at(previous_height);
        let transactions = (round
            .get("transactions")
            .and_then(Value::as_array)
            .into_iter())
        .flatten()
        .map(|t| hex::decode(t.as_str().unwrap_or_default()).map_err(|e| e.to_string()))
        .map(|bytes| Transaction::decode_trusted(&bytes?, params).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
        let previous = round
            .get("previousId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let inputs = ForgeInputs {
            transactions: &transactions,
            previous_id: BlockId::from_hex(previous).map_err(|e| e.to_string())?,
            previous_height,
            timestamp: u32::try_from(number("timestamp")?).map_err(|e| e.to_string())?,
            reward: (round.get("reward").and_then(Value::as_str))
                .and_then(|r| r.parse().ok())
                .ok_or("no reward")?,
        };
        let asked = round.get("slot").and_then(Value::as_i64).ok_or("no slot")?;
        let started = SystemClock.now_ms();
        let outcome = signer.sign_or_resend(&key, &inputs, slot, asked);
        let took = SystemClock.now_ms().saturating_sub(started);
        let block = match outcome {
            Ok(SignedOutcome::New(block)) => {
                let recorded = json!({"slot": asked, "id": block.id.to_hex(), "ms": took});
                relay.send(&json!({ "recorded": recorded }))?;
                Some(block)
            }
            Ok(SignedOutcome::Resent(block)) => {
                relay.send(&json!({"resent": {"slot": asked, "id": block.id.to_hex()}}))?;
                Some(block)
            }
            Err(error) => {
                let refused = json!({"slot": asked, "error": error.to_string()});
                relay.send(&json!({ "refused": refused }))?;
                None
            }
        };
        if let Some(block) = block {
            let hex = hex::encode(&block.bytes);
            let post = json!({"call": "post", "slot": asked, "id": block.id.to_hex(), "hex": hex});
            if relay.call(post)?.is_none() {
                return Ok(());
            }
        }
        // Sleep until the next slot of the forger's own clock.
        let next = slots::slot_time(slot.saturating_add(1), block_time).saturating_mul(1_000);
        let wait = epoch
            .saturating_add(next)
            .saturating_sub(SystemClock.now_ms());
        std::thread::sleep(Duration::from_millis(u64::try_from(wait).unwrap_or(0)));
    }
}

/// The mock relay: JSON lines on the standard output and input.
struct Relay {
    lines: std::io::Lines<std::io::StdinLock<'static>>,
}

impl Relay {
    fn new() -> Relay {
        Relay {
            lines: std::io::stdin().lock().lines(),
        }
    }

    fn send(&self, message: &Value) -> Result<(), String> {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{message}")
            .and_then(|()| out.flush())
            .map_err(|e| e.to_string())
    }

    /// Send `request` and read the answer; `None` once the relay has closed the input.
    fn call(&mut self, request: Value) -> Result<Option<Value>, String> {
        self.send(&request)?;
        match self.lines.next() {
            None => Ok(None),
            Some(line) => {
                let line = line.map_err(|e| e.to_string())?;
                serde_json::from_str(&line)
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
        }
    }
}
