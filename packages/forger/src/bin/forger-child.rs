//! The crash test's child (feature `difftest`): the forger's slot loop, relay client and block
//! signer over a configuration directory, against the harness's mock relay, which kills it with
//! SIGKILL at random moments.
//!
//!   forger-child --config DIR --data DIR --relay IP:PORT
//!
//! The configuration directory holds `delegates.json` and the four crypto files in `crypto/`; the
//! signing record goes to the data directory. The child runs the forger's service over a client
//! of the one relay and the production signer with a fresh random aux for every signature, so
//! only the signing record keeps a slot's bytes the same. It writes JSON lines on its standard
//! output:
//!
//! - `{"ready": {"publicKey"}}` once the keys, the crypto files and the record are loaded;
//! - the signer's outcome for every block the loop asks for: `{"recorded": {slot, id, ms}}` once
//!   a new block is recorded (before it is posted), `{"resent": {slot, id}}` for a block the record
//!   already held, or `{"refused": {slot, error}}`.
//!
//! Its blocks reach the relay as the forger posts them. It stops its loop, letting a post in
//! flight finish, when its standard input closes.

#![forbid(unsafe_code)]

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::{Aux, PublicKeyBytes};
use heartwood_forger::client::{Client, ClientOptions};
use heartwood_forger::signing_record::{self, SigningRecord};
use heartwood_forger::{
    BlockSigner, ForgeInputs, ForgerParts, ForgerService, Options, RelayHost, ShutdownSignal,
    SignError, SignedOutcome, Signer, load_keys,
};
use heartwood_kernel::config::crypto;
use heartwood_kernel::{Clock, SystemClock};
use serde_json::{Value, json};

/// The version the child's public calls carry in their headers.
const VERSION: &str = concat!("4.3.1-heartwood.", env!("CARGO_PKG_VERSION"));

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
    let (mut config, mut data, mut relay) = (None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next();
        match flag.as_str() {
            "--config" => config = value.map(PathBuf::from),
            "--data" => data = value.map(PathBuf::from),
            "--relay" => relay = value.and_then(|text| text.parse::<SocketAddr>().ok()),
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    let usage = || "usage: forger-child --config DIR --data DIR --relay IP:PORT".to_owned();
    let (config, data) = (config.ok_or_else(usage)?, data.ok_or_else(usage)?);
    let relay = relay.ok_or_else(usage)?;
    let files = crypto::load(&config.join("crypto")).map_err(|e| e.to_string())?;
    let milestones = files.milestones;
    let keys = load_keys(&config).map_err(|e| e.to_string())?;
    let delegates = keys.delegates(files.network.pub_key_hash);
    let key = *delegates.first().ok_or("no delegate")?.public_key();
    let path = signing_record::path(&data);
    let record = SigningRecord::open(&path, &files.network.nethash).map_err(|e| e.to_string())?;
    let signer = Reporting(Signer::new(
        delegates,
        record,
        Aux::random(),
        milestones.clone(),
    ));
    let hosts = vec![RelayHost {
        hostname: relay.ip(),
        port: relay.port(),
    }];
    let client = Client::new(ClientOptions {
        hosts: hosts.clone(),
        local_address: None,
        version: VERSION.into(),
    });
    let options = Options {
        hosts,
        aux: None,
        delay: Duration::ZERO,
        local_address: None,
    };
    let parts = ForgerParts {
        link: Arc::new(client),
        signer,
        delegates: keys.declared(),
        options,
        milestones,
        clock: Arc::new(SystemClock) as Arc<dyn Clock>,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let (trigger, shutdown) = ShutdownSignal::new();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        trigger.trigger();
    });
    say(&json!({"ready": {"publicKey": key.to_hex()}}))?;
    runtime.block_on(async {
        let mut service = ForgerService::new(parts);
        service.boot().await.map_err(|e| e.to_string())?;
        service.run(shutdown).await;
        service.dispose().await;
        Ok(())
    })
}

/// The production signer, each outcome written on the standard output for the harness.
struct Reporting(Signer);

impl BlockSigner for Reporting {
    fn sign_or_resend(
        &mut self,
        key: &PublicKeyBytes,
        inputs: &ForgeInputs<'_>,
        own_slot: i64,
        slot: i64,
    ) -> Result<SignedOutcome, SignError> {
        let started = SystemClock.now_ms();
        let outcome = self.0.sign_or_resend(key, inputs, own_slot, slot);
        let ms = SystemClock.now_ms().saturating_sub(started);
        let line = match &outcome {
            Ok(SignedOutcome::New(block)) => {
                json!({"recorded": {"slot": slot, "id": block.id.to_hex(), "ms": ms}})
            }
            Ok(SignedOutcome::Resent(block)) => {
                json!({"resent": {"slot": slot, "id": block.id.to_hex()}})
            }
            Err(error) => json!({"refused": {"slot": slot, "error": error.to_string()}}),
        };
        // An output the harness no longer reads does not stop the forger.
        let _ = say(&line);
        outcome
    }
}

/// Write `message` as one line on the standard output.
fn say(message: &Value) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{message}")
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}
