//! The forger's composition root: the forger role's configuration (its profile in `app.json`, no
//! `peers.json`), the process lock `<data>/run/forger.lock` taken as the commands probe it, the
//! logger, the key file, the forger's options and its relays, the signing record, then the forger
//! component booted and its slot loop and attester run until a signal or a fatal error. The
//! forger opens no listening socket, no store and no API.
//!
//! The version floor is checked at the height of the relay's tip at the first round: a floor that
//! refuses this version stops the forger; a later one is a warning. A forger component that fails
//! to boot stops the process with an error naming it, where the reference's idles.
//!
//! The graceful shutdown stops scheduling, lets a `postBlock` in flight finish to its deadline,
//! then closes the relay sessions, so the relay's statuses become unsigned.

use std::fmt::Display;
use std::fs;
use std::sync::{Arc, Mutex, PoisonError};

use heartwood_cli::Role;
use heartwood_forger::client::{Client, ClientOptions};
use heartwood_forger::signing_record::{self, SigningRecord};
use heartwood_forger::{
    Attester, Forger, ForgerParts, ForgerService, Options, ShutdownSignal, Signer, load_keys,
};
use heartwood_kernel::lifecycle::Component;
use heartwood_kernel::{Clock, SystemClock};
use heartwood_p2p::P2pOptions;
use thiserror::Error;
use tokio::runtime::Builder;

use crate::commands::forger::ForgerRun;
use crate::config::ForgerConfig;
use crate::lock::{self, ProcessLock};
use crate::{VERSION, options, signals};

/// The forger could not start, or stopped on a fatal error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ForgerError {
    /// The configuration was refused.
    #[error("{0}")]
    Config(String),
    /// Another forger of the instance holds the process lock.
    #[error("{0}")]
    Locked(String),
    /// The forger component failed.
    #[error("{0}")]
    Component(String),
}

fn config(error: impl Display) -> ForgerError {
    ForgerError::Config(error.to_string())
}

/// Run a forger with the flags `run` until a signal or a fatal error, with the process's
/// environment, home directory and working directory.
pub fn run_forger(run: &ForgerRun) -> Result<(), ForgerError> {
    let (process, home, cwd) = super::relay::process_context().map_err(config)?;
    start(ForgerConfig::load(run, &process, &home, &cwd).map_err(config)?)
}

/// Run the forger of `forger`.
pub fn start(forger: ForgerConfig) -> Result<(), ForgerError> {
    let paths = &forger.base.paths;
    for dir in [&paths.data, &paths.log, &paths.temp] {
        fs::create_dir_all(dir).map_err(|error| config(format!("{}: {error}", dir.display())))?;
    }
    let (token, network) = (&forger.base.token, &forger.base.network);
    let locked = ProcessLock::acquire(&lock::instance(Role::Forger, token, network, paths));
    let _lock = locked.map_err(|error| ForgerError::Locked(error.to_string()))?;
    let logger = options::logger(&forger.logger, &forger.base.env).map_err(config)?;
    let stem = format!("{}-{}-forger", forger.base.token, forger.base.network);
    let _logs = heartwood_logger::init(&logger, &paths.log, &stem).map_err(component)?;
    heartwood_kernel::info!("Heartwood Core {VERSION}: a forger of {token} on {network}");
    forger
        .base
        .warnings
        .iter()
        .for_each(|w| heartwood_kernel::warning!("{w}"));
    // One thread, as the reference's forger has one; the signing record's writes go to the
    // runtime's blocking pool.
    let runtime = Builder::new_current_thread().enable_all().build();
    let result = runtime
        .map_err(component)
        .and_then(|runtime| runtime.block_on(serve(forger)));
    if let Err(error) = &result {
        heartwood_kernel::error!("{error}");
    }
    result
}

fn component(error: impl Display) -> ForgerError {
    ForgerError::Component(error.to_string())
}

/// Read the key file, the options and the signing record of `forger`, each checked before
/// anything connects; boot the forger component and run its loop and attester until a signal,
/// or until the version floor at the first round refuses this version.
async fn serve(forger: ForgerConfig) -> Result<(), ForgerError> {
    let (base, entry, crypto) = (forger.base, forger.forger, forger.crypto);
    let keys = load_keys(&base.paths.config).map_err(config)?;
    let env = |name: &str| base.env.get(name).map(str::to_owned);
    let options = Options::from_json(&entry, &env).map_err(config)?;
    let (milestones, network) = (crypto.milestones, crypto.network);
    let path = signing_record::path(&base.paths.data);
    let record = SigningRecord::open(&path, &network.nethash).map_err(component)?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let link = Arc::new(Client::new(ClientOptions {
        hosts: options.hosts.clone(),
        local_address: options.local_address,
        version: VERSION.to_owned(),
    }));
    let delegates = keys.delegates(network.pub_key_hash);
    let signer = Signer::new(delegates, record, options.aux(), milestones.clone());
    let declared = keys.declared();
    let (nethash, milestones_) = (network.nethash.clone(), milestones.clone());
    let attester = Attester::new(Arc::new(keys), nethash, milestones_, clock.clone());
    let service = ForgerService::new(ForgerParts {
        link: Arc::clone(&link),
        signer,
        delegates: declared,
        options,
        milestones: milestones.clone(),
        clock,
    });
    let (trigger, shutdown) = ShutdownSignal::new();
    let (trigger, refused) = (Arc::new(trigger), Arc::new(Mutex::new(None)));
    let (stop, refusal) = (Arc::clone(&trigger), Arc::clone(&refused));
    let defaults = P2pOptions::default().minimum_versions;
    service.on_first_round(move |height| {
        if let Err(error) = super::check_floor(&milestones, &network.name, height, &defaults) {
            *refusal.lock().unwrap_or_else(PoisonError::into_inner) = Some(error.to_string());
            stop.trigger();
        }
    });
    let signal = Arc::clone(&trigger);
    tokio::spawn(async move {
        signals::shutdown_requested("forger").await;
        signal.trigger();
    });
    let mut component = Forger::new(service);
    component
        .boot()
        .await
        .map_err(|error| ForgerError::Component(error.to_string()))?;
    let attesting = attester.run(&*link, shutdown.clone());
    tokio::join!(component.service().run(shutdown), attesting);
    component.dispose().await;
    let refused = refused
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    refused.map_or(Ok(()), |refusal: String| Err(ForgerError::Config(refusal)))
}
