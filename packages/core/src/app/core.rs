//! The core role's composition root: one process holding the relay's and the forger's locks of
//! its instance besides its own ([`heartwood_cli::Role::also_locks`]), the relay's components
//! started as [`crate::app::relay`] starts them, then the forger as [`crate::app::forger`] does, on
//! its own thread and runtime, reaching its relay's peer-to-peer listener on loopback (`hosts`
//! defaults to `127.0.0.1:<CORE_P2P_PORT>`; a `remoteAccess` without a loopback address stops the
//! start, naming the option). Shutdown in the core order: the relay's first two steps, the
//! forger's three, then the relay's last two. Logs go to `<token>-<network>-core-current.log`.

use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use heartwood_cli::Role;
use heartwood_forger::{Options, load_keys};
use heartwood_kernel::config;
use heartwood_p2p::ServerOptions;
use thiserror::Error;
use tokio::runtime::Builder;
use tokio::sync::oneshot;

use crate::app::forger::{self, ForgerError};
use crate::app::relay::{Beside, RelayError, launch, process_context};
use crate::commands::core::CoreRun;
use crate::config::{ForgerConfig, Package, RelayConfig};

/// The core role could not start, or stopped on a fatal error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    /// The configuration was refused.
    #[error("{0}")]
    Config(String),
    /// The relay's side failed.
    #[error(transparent)]
    Relay(#[from] RelayError),
    /// The forger's side failed.
    #[error(transparent)]
    Forger(#[from] ForgerError),
}

/// Run the core role with the flags `run` until a signal or a fatal error, with the process's
/// environment and directories, as `relay run` and `forger run` do.
pub fn run_core(run: &CoreRun) -> Result<(), CoreError> {
    let (process, home, cwd) = process_context().map_err(CoreError::Config)?;
    let relay = RelayConfig::load_as(config::Role::Core, &run.relay, &process, &home, &cwd)?;
    let forger = relay.forger();
    let forger = forger.ok_or_else(|| {
        CoreError::Config(format!(
            "the core profile of app.json has no {}",
            Package::Forger.name()
        ))
    })?;
    check(&relay, &forger)?;
    let (stop, stopping) = oneshot::channel();
    let (fail, failed) = oneshot::channel();
    let worker: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::default();
    let (running, joining) = (Arc::clone(&worker), worker);
    let started = move || {
        let spawned = std::thread::Builder::new().name("heartwood-forger".into());
        let spawned = spawned.spawn(move || forge(forger, stopping, fail));
        *running.lock().unwrap_or_else(PoisonError::into_inner) = spawned.ok();
    };
    let stopped = move || {
        heartwood_kernel::info!("Stopping the forger");
        let _ = stop.send(());
        let joined = joining
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = joined {
            let _ = thread.join();
        }
        heartwood_kernel::info!("The forger has stopped");
    };
    let beside = Beside {
        started: Some(Box::new(started)),
        failed: Some(failed),
        stopped: Some(Box::new(stopped)),
    };
    Ok(launch(Role::Core, relay, &run.relay, beside)?)
}

/// What stops the start before anything runs: the forger's key file and options, and a
/// `remoteAccess` that admits no loopback address, since the forger reaches the relay there.
fn check(relay: &RelayConfig, forger: &ForgerConfig) -> Result<(), CoreError> {
    let config = |error: String| CoreError::Config(error);
    load_keys(&relay.paths.config).map_err(|error| config(error.to_string()))?;
    let env = |name: &str| relay.env.get(name).map(str::to_owned);
    Options::from_json(&forger.forger, &env).map_err(|error| config(error.to_string()))?;
    let p2p = crate::options::p2p(&relay.entry(Package::P2p), &relay.env);
    let admitted = ServerOptions::from_p2p(&p2p.map_err(|error| config(error.to_string()))?);
    if admitted.remote_access.iter().any(|ip| ip.is_loopback()) {
        return Ok(());
    }
    Err(config(
        "remoteAccess lists no loopback address: the forger of a core role reaches its relay on 127.0.0.1, so the option must admit it".into(),
    ))
}

/// The forger's thread: its own current-thread runtime, run until `stopping` ends. Its end
/// before that, with an error or none, is told to the relay through `fail`.
fn forge(forger: ForgerConfig, stopping: oneshot::Receiver<()>, fail: oneshot::Sender<String>) {
    let stopped = async {
        let _ = stopping.await;
    };
    let runtime = Builder::new_current_thread().enable_all().build();
    let result = match runtime {
        Ok(runtime) => runtime.block_on(forger::serve(forger, stopped)),
        Err(error) => Err(ForgerError::Component(error.to_string())),
    };
    // The relay logs the failure as its own, once its node has stopped.
    let failure = match result {
        Ok(()) => "the forger stopped".to_owned(),
        Err(error) => format!("the forger stopped: {error}"),
    };
    let _ = fail.send(failure);
}
