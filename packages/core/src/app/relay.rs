//! The relay's composition root: the configuration ([`crate::config`]), the process lock, the
//! logger, then the components in rank order: the store and the state (restored, with the
//! version floor at the stored tip), the pool, the node thread, the peer-to-peer client, the
//! blockchain service, the peer-to-peer server and, once the blockchain has started, the API. It
//! links the node's views ([`crate::views`]) and services ([`crate::node_services`]) to them and
//! runs until a signal or a fatal error, then shuts down in reverse: the forger's sessions are
//! closed first, the P2P server and the API stop accepting connections, the blockchain service
//! stops the queue after its job, and the node thread flushes the pool, writes the online record,
//! and closes the store.
//!
//! The P2P server ([`serve_p2p`]) binds `server.hostname:server.port` before the blockchain boots,
//! and answers every route with "App is not ready" until it has; a listener that cannot bind stops
//! the relay, as the reference ends its application when its server cannot start. The forger's
//! attestation session gives the delegates on this node to the node thread's online record, the
//! network monitor's fork check and the network state.
//!
//! The reference's marker files are honoured at the start, when they are the relay's user's alone
//! ([`crate::markers`]): `clear-saved-states.lock` in the temp directory restores the state by a
//! replay (and is removed once it is done), and `clear-pool.lock` clears the pool's storage when
//! the pool opens. So are its reset variables:
//! `CORE_RESET_DATABASE` empties the store (the start is a first start) and the pool, and
//! `CORE_RESET_POOL` empties the pool, at every start while they are set.

use std::fmt::Display;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZero;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_api::{
    Api, ApiConfig, ApiContext, NodeInfo, NodeStatus, PeerList, StoreProductivity,
};
use heartwood_blockchain::state_machine::MachineState;
use heartwood_blockchain::{
    BlockProcessor, Blockchain, BlockchainParts, ChainState, ChainStore, Node, NodeError,
    NodeHandle, NodeParts, PoolTask, RestoreMode, RestorePath, Restored, RollbackDepth, restore,
};
use heartwood_cli::Role;
use heartwood_database::{Readers, Settings};
use heartwood_kernel::bus::EventBus;
use heartwood_kernel::config::Options;
use heartwood_kernel::errors::BootError;
use heartwood_kernel::lifecycle::Component;
use heartwood_kernel::{Clock, SystemClock};
use heartwood_p2p::attestation_session::{AttestationSession, DelegatesOnThisNode};
use heartwood_p2p::diversity::Sampler;
use heartwood_p2p::processor::ProcessorParts;
use heartwood_p2p::seeds::HttpProbe;
use heartwood_p2p::server::controllers::{ControllerParts, Controllers};
use heartwood_p2p::status_server::StatusServer;
use heartwood_p2p::{
    Communicator, CommunicatorParts, Connector, MonitorParts, NetworkMonitor, NodeServices,
    P2pOptions, P2pServer, PeerSide, ServerOptions, ServerParts,
};
use heartwood_p2p::{PeerProcessor, PeerRepository, PeerVerifier};
use heartwood_p2p_wire::replies::PeerAddress;
use heartwood_p2p_wire::status::{NetworkConfig, PeerConfig, PluginConfig, TokenConfig};
use heartwood_pool::{Options as PoolOptions, Pool};
use heartwood_transactions::Handlers;
use thiserror::Error;
use tokio::runtime::{Builder, Handle};
use tokio::sync::{mpsc, oneshot, watch};

use crate::commands::relay::RelayRun;
use crate::config::{Package, RelayConfig};
use crate::lock::{self, ProcessLock};
use crate::network::Monitored;
use crate::node_services::{MonitorSide, RelayServices, queue_state};
use crate::signals::Signals;
use crate::views::{BusEvents, Links, Listener, NodeView, OnThisNode, Submit};
use crate::{VERSION, options};

/// The relay could not start, or stopped on a fatal error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RelayError {
    /// The configuration was refused.
    #[error("{0}")]
    Config(String),
    /// Another relay of the instance holds the process lock.
    #[error("{0}")]
    Locked(String),
    /// A component failed.
    #[error("{0}")]
    Component(String),
    /// The node stopped on a fatal error.
    #[error("{0}")]
    Fatal(String),
}

/// The peer-to-peer rules of this build: the client's, with the harness's seed under `difftest`.
#[cfg(not(feature = "difftest"))]
type Rules = heartwood_p2p::utils::rules::Rules;
/// The peer-to-peer rules of this build: the client's, with the harness's seed under `difftest`.
#[cfg(feature = "difftest")]
type Rules = crate::harness::SeedRules;
type Client = Communicator<Connector, PeerVerifier, Rules, HttpProbe>;
type Acceptor = PeerProcessor<Client, Rules>;

/// The read-only connections of the P2P server's store reads (getBlocks, getCommonBlocks).
const SERVER_READERS: usize = 4;

/// Why the relay stops, or what it waits for.
enum Stop {
    /// A SIGINT or SIGTERM.
    Signal,
    /// The blockchain service booted, or failed to.
    Booted(Result<(), BootError>),
    /// The node thread ended.
    Node(Result<(), NodeError>),
    /// The machine reached its exit state.
    Exit,
    /// The component that runs beside the relay failed.
    Companion(String),
}

/// What runs beside the relay in one process: the core role's forger.
#[derive(Default)]
pub(crate) struct Beside {
    /// Called once the peer-to-peer listener is bound.
    pub started: Option<Box<dyn FnOnce() + Send>>,
    /// The companion's failure, which stops the relay.
    pub failed: Option<oneshot::Receiver<String>>,
    /// Called on a blocking thread after the blockchain service is disposed and before the node
    /// stops.
    pub stopped: Option<Box<dyn FnOnce() + Send>>,
}

fn component(error: impl Display) -> RelayError {
    RelayError::Component(error.to_string())
}

fn config(error: impl Display) -> RelayError {
    RelayError::Config(error.to_string())
}

/// Run a relay with the flags `run` until a signal or a fatal error, with the process's
/// environment, home directory and working directory.
pub fn run_relay(run: &RelayRun) -> Result<(), RelayError> {
    let (process, home, cwd) = process_context().map_err(config)?;
    start(RelayConfig::load(run, &process, &home, &cwd)?, run)
}

/// A process's variables (those with text names and values), its home directory and its working
/// directory.
pub type ProcessContext = (
    Vec<(String, String)>,
    std::path::PathBuf,
    std::path::PathBuf,
);

/// This process's variables, home directory and working directory.
pub fn process_context() -> Result<ProcessContext, String> {
    let vars = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)));
    let process: Vec<(String, String)> = vars.collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let home = home.ok_or("HOME is not set")?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok((process, home, cwd))
}

/// Run the relay of `relay`, whose flags are `run`.
pub fn start(relay: RelayConfig, run: &RelayRun) -> Result<(), RelayError> {
    launch(Role::Relay, relay, run, Beside::default())
}

/// Run the relay of `relay` as the process of `role` (the relay, or the core role, which also
/// holds the locks of the roles it contains), with what runs `beside` it.
pub(crate) fn launch(
    role: Role,
    relay: RelayConfig,
    run: &RelayRun,
    beside: Beside,
) -> Result<(), RelayError> {
    let paths = &relay.paths;
    // The temp directory is not made: the relay only reads the reference's markers there.
    for dir in [&paths.config, &paths.data, &paths.cache, &paths.log] {
        fs::create_dir_all(dir).map_err(|error| config(format!("{}: {error}", dir.display())))?;
    }
    // The signals are caught before the pid file is written.
    let runtime = Builder::new_multi_thread().enable_all().build();
    let runtime = runtime.map_err(component)?;
    let signals = Signals::listen_in(&runtime, role.name());
    let instance = lock::instance(role, &relay.token, &relay.network, paths);
    let locked = ProcessLock::acquire(&instance);
    let _lock = locked.map_err(|error| RelayError::Locked(error.to_string()))?;
    let _also =
        lock::hold_also(&instance).map_err(|error| RelayError::Locked(error.to_string()))?;
    let logger = options::logger(&relay.entry(Package::Logger), &relay.env).map_err(config)?;
    let stem = format!("{}-{}-{}", relay.token, relay.network, role.name());
    let _logs = heartwood_logger::init(&logger, &paths.log, &stem).map_err(component)?;
    let (token, network) = (&relay.token, &relay.network);
    match role {
        Role::Core => {
            heartwood_kernel::info!("Heartwood Core {VERSION}: a core node of {token} on {network}")
        }
        _ => heartwood_kernel::info!("Heartwood Core {VERSION}: a relay of {token} on {network}"),
    }
    for warning in &relay.warnings {
        heartwood_kernel::warning!("{warning}");
    }
    let api = (Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("heartwood-api")
        .enable_all()
        .build())
    .map_err(component)?;
    let result = runtime.block_on(serve(relay, run, api.handle().clone(), signals, beside));
    if let Err(error) = &result {
        heartwood_kernel::error!("{error}");
    }
    api.shutdown_timeout(Duration::from_secs(5));
    runtime.shutdown_timeout(Duration::from_secs(5));
    result
}

/// Start the components of `relay` and run them until a stop.
pub(crate) async fn serve(
    relay: RelayConfig,
    run: &RelayRun,
    api_runtime: Handle,
    signals: Signals,
    mut beside: Beside,
) -> Result<(), RelayError> {
    let (stops, mut stop) = mpsc::unbounded_channel();
    if let Some(failed) = beside.failed.take() {
        let failure = stops.clone();
        tokio::spawn(async move {
            let message = failed.await;
            let message = message.unwrap_or_else(|_| "the forger stopped".into());
            let _ = failure.send(Stop::Companion(message));
        });
    }
    let signal = stops.clone();
    tokio::spawn(async move {
        signals.requested().await;
        let _ = signal.send(Stop::Signal);
    });
    // Every option first, so a refusal stops the start before anything runs.
    let cpus = std::thread::available_parallelism().map_or(1, NonZero::get);
    let pool_options = options::pool(
        &relay.entry(Package::Pool),
        &relay.env,
        &relay.paths.data,
        cpus,
    )
    .map_err(config)?;
    let api_timeout = options::database(&relay.entry(Package::Database)).map_err(config)?;
    let mut p2p = options::p2p(&relay.entry(Package::P2p), &relay.env).map_err(config)?;
    p2p.version = VERSION.to_owned();
    p2p.seeds = (relay.peers.list.iter())
        .map(|peer| PeerAddress {
            ip: peer.ip.clone(),
            port: peer.port,
        })
        .collect();
    p2p.sources.clone_from(&relay.peers.sources);
    p2p.max_transactions_per_request = pool_options.max_transactions_per_request;
    if p2p.remote_access.is_empty() {
        heartwood_kernel::warning!(
            "remoteAccess is empty: no forger can reach this relay's internal routes"
        );
    }
    let blockchain_options = options::blockchain(&relay.entry(Package::Blockchain));
    let blockchain_options = Arc::new(blockchain_options.map_err(config)?);
    let mut api_config = api_options(&relay)?;
    if let Some(api) = api_config.as_mut() {
        let lookback = blockchain_options.missed_blocks_lookback;
        api.missed_blocks_lookback = i64::try_from(lookback).unwrap_or(i64::MAX);
    }
    devnet_guard(&p2p, &relay.crypto.network.name, api_config.as_ref())?;
    let peer_config = peer_config(&relay, api_config.as_ref());
    // The store, the state and the version floor at the stored tip.
    let settings = Settings::in_data_dir(&relay.paths.data);
    let (store, restored) = restored(&relay)?;
    // A signal during a long restore (a replay) stops the start here, before anything runs.
    if stop.try_recv().is_ok() {
        return Ok(());
    }
    let files = &relay.crypto;
    version_floor(&relay, restored.state.height(), &p2p.minimum_versions)?;
    let pool = open_pool(&relay, pool_options)?;
    // The node thread.
    let (milestones, network) = (
        Arc::new(files.milestones.clone()),
        Arc::new(files.network.clone()),
    );
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let (links, bus) = (Arc::new(Links::default()), EventBus::new());
    let chain = Arc::new(Mutex::new(ChainState::at(Arc::new(
        restored.state.last_block().clone(),
    ))));
    let first_start = restored.path == RestorePath::FirstStart;
    #[cfg(feature = "difftest")]
    let (observer, record, await_record, attestation_record) =
        crate::harness::files(&relay, run, &milestones, first_start).map_err(config)?;
    #[cfg(feature = "difftest")]
    let stall = (relay.env.get(crate::harness::DIR_VARIABLE))
        .filter(|dir| !dir.is_empty())
        .map(|dir| heartwood_blockchain::difftest::StallControl::new(dir.into()));
    #[cfg(not(feature = "difftest"))]
    let observer = (None, first_start).0;
    let (node, handle) = Node::spawn(NodeParts {
        state: restored.state,
        store,
        pool: PoolTask::new(pool, restored.online),
        chain: Arc::clone(&chain),
        processor: Arc::new(BlockProcessor),
        handlers: Handlers,
        milestones: Arc::clone(&milestones),
        network: Arc::clone(&network),
        options: Arc::clone(&blockchain_options),
        clock: Arc::clone(&clock),
        listener: Arc::new(Listener {
            bus: bus.clone(),
            links: Arc::clone(&links),
        }),
        observer,
        broadcast: links.clone(),
        own_keys: links.clone(),
        version: VERSION.to_owned(),
        #[cfg(feature = "difftest")]
        stall,
    })
    .map_err(component)?;
    let ended = stops.clone();
    tokio::task::spawn_blocking(move || ended.send(Stop::Node(node.join())));
    // The peer-to-peer client and the blockchain service. The client's store reads run on the
    // runtime's workers, so there is a read-only connection for each worker.
    let view = Arc::new(NodeView {
        handle: handle.clone(),
        chain: Arc::clone(&chain),
        milestones: Arc::clone(&milestones),
        network,
        history: Arc::new(Readers::open(&settings, cpus.max(2)).map_err(component)?),
        links: Arc::clone(&links),
    });
    let (p2p, seed) = seeded(p2p, run)?;
    let p2p = Arc::new(p2p);
    let peers = Arc::new(client(&p2p, seed, view.clone(), &bus, &links, &clock)?);
    let _ = links.peers.set(Arc::new(Arc::clone(&peers.monitor)));
    let blockchain = Blockchain::new(BlockchainParts {
        node: handle.clone(),
        chain,
        network: Monitored(peers.monitor.clone(), links.clone(), clock.clone()),
        options: blockchain_options,
        milestones,
        clock: Arc::clone(&clock),
        rollback: RollbackDepth::Random,
        runtime: Handle::current(),
    });
    let _ = links.machine.set(Arc::new(Arc::clone(&blockchain)));
    let (exit, mut machine) = (stops.clone(), blockchain.status());
    tokio::spawn(async move {
        while machine.changed().await.is_ok() {
            if machine.borrow().state == MachineState::Exit {
                let _ = exit.send(Stop::Exit);
            }
        }
    });
    // The peer-to-peer server, on read-only connections of its own, before the blockchain boots.
    let services = RelayServices {
        handle: handle.clone(),
        view,
        history: Arc::new(Readers::open(&settings, SERVER_READERS).map_err(component)?),
        links: Arc::clone(&links),
        queue: queue_state(&blockchain),
        blockchain: Arc::clone(&blockchain),
        peer_config,
        bus: bus.clone(),
        clock: Arc::clone(&clock),
        #[cfg(feature = "difftest")]
        record,
        #[cfg(feature = "difftest")]
        await_record,
        #[cfg(feature = "difftest")]
        attestation_record,
    };
    // A listener that cannot bind stops the relay, through the graceful shutdown.
    let (p2p, mut unbound) = match serve_p2p(p2p, Arc::new(services), peers, &links, clock).await {
        Ok(p2p) => (Some(p2p), None),
        Err(error) => (None, Some(error)),
    };
    let (booted, service) = (stops, Arc::clone(&blockchain));
    if p2p.is_some() {
        if let Some(start) = beside.started.take() {
            start();
        }
        tokio::spawn(async move { booted.send(Stop::Booted(service.boot().await)) });
    }
    let status = node_status(&links);
    // Serve until a stop; the API once the blockchain has started, as the reference boots it.
    let (mut api, mut node_ended) = (None, false);
    let outcome = loop {
        if let Some(error) = unbound.take() {
            break Err(error);
        }
        match stop.recv().await {
            Some(Stop::Booted(Ok(()))) => {
                links.booted.store(true, Ordering::Release);
                let Some(config) = api_config.take() else {
                    continue;
                };
                let node = NodeInfo {
                    crypto: relay.crypto.clone(),
                    version: VERSION.to_owned(),
                };
                let served = async {
                    let timeout = Duration::from_millis(api_timeout);
                    let history = Readers::with_timeout(&settings, 10, timeout);
                    let history = history.map_err(component)?;
                    let runtime = api_runtime.clone();
                    serve_api(config, node, &handle, &links, &status, history, runtime).await
                };
                // A failure to serve stops the relay, as the reference's server ends its
                // application when it cannot start.
                match served.await {
                    Ok(served) => api = served,
                    Err(error) => break Err(error),
                }
            }
            Some(Stop::Booted(Err(error))) => break Err(component(error)),
            Some(Stop::Signal) | None => break Ok(()),
            Some(Stop::Node(result)) => {
                node_ended = true;
                break Err(RelayError::Fatal(
                    result
                        .err()
                        .map_or("the node thread stopped".into(), |e| e.to_string()),
                ));
            }
            Some(Stop::Exit) => break Err(RelayError::Fatal("the blockchain stopped".into())),
            Some(Stop::Companion(message)) => break Err(RelayError::Fatal(message)),
        }
    };
    // The graceful shutdown, in reverse: the forger's sessions first.
    if let Some(p2p) = p2p {
        p2p.stop().await;
    }
    if let Some(mut api) = api {
        api.dispose().await;
    }
    blockchain.dispose().await;
    if let Some(stopped) = beside.stopped.take() {
        let _ = tokio::task::spawn_blocking(stopped).await;
    }
    handle.stop().await;
    while !node_ended {
        match stop.recv().await {
            Some(Stop::Node(Err(error))) => return Err(RelayError::Fatal(error.to_string())),
            Some(Stop::Node(Ok(()))) | None => node_ended = true,
            Some(_) => {}
        }
    }
    heartwood_kernel::info!("The node has stopped: its pool is flushed and its store is closed");
    outcome
}

/// The relay's P2P server, bound and serving, with the served status's timer.
pub struct P2p {
    server: Arc<P2pServer>,
    shutdown: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl P2p {
    /// The first step of the graceful shutdown: close the forger's sessions, then stop accepting
    /// and close every socket, and wait for the server's tasks.
    pub async fn stop(self) {
        self.server.close_forger_sessions();
        let _ = self.shutdown.send(true);
        for task in self.tasks {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }
}

/// The P2P server of `options` over the node's services `node` and the peer side `peers`: the
/// forger's attestation session (linked into `links` as the delegates on this node), the served
/// status, the controllers and the listener, bound; a listener that cannot bind is the error.
pub async fn serve_p2p(
    options: Arc<P2pOptions>,
    node: Arc<dyn NodeServices>,
    peers: Arc<dyn PeerSide>,
    links: &Links,
    clock: Arc<dyn Clock>,
) -> Result<P2p, RelayError> {
    let attestation = Arc::new(AttestationSession::new(node.clone(), clock.clone()));
    let _ = links.attested.set(attestation.clone());
    let on_this_node = DelegatesOnThisNode(attestation.clone());
    let _ = links.on_this_node.set(Arc::new(on_this_node));
    let server_options = ServerOptions::from_p2p(&options);
    let status = StatusServer::new(node.clone(), attestation.clone(), clock.clone());
    let status = Arc::new(status.with_wait(server_options.attestation_wait));
    let (attested, served, options_) = (attestation.clone(), status.clone(), options.clone());
    let handlers = Arc::new(Controllers::new(ControllerParts {
        node: node.clone(),
        peers: peers.clone(),
        attestation: attested,
        status: served,
        clock,
        options: options_,
    }));
    let server = P2pServer::bind(ServerParts {
        options: server_options,
        node,
        peers,
        handlers,
        events: attestation,
    })
    .await
    .map_err(component)?;
    let (server, (shutdown, signal)) = (Arc::new(server), watch::channel(false));
    let serving = (Arc::clone(&server), signal.clone());
    let tasks = vec![
        tokio::spawn(async move { serving.0.serve(serving.1).await }),
        tokio::spawn(async move { status.run(signal).await }),
    ];
    Ok(P2p {
        server,
        shutdown,
        tasks,
    })
}

/// The runtime guards of a devnet node (see [`heartwood_p2p::devnet::guard`]): the peer server's,
/// and, with devnet addressing, that the API, when it is served, listens on loopback.
fn devnet_guard(
    p2p: &P2pOptions,
    network: &str,
    api: Option<&ApiConfig>,
) -> Result<(), RelayError> {
    heartwood_p2p::devnet::guard(p2p, network).map_err(config)?;
    match api.filter(|api| api.enabled) {
        Some(api) => {
            let api =
                heartwood_p2p::devnet::guard_listener(&p2p.addressing, "the API", api.address.ip());
            api.map_err(config)
        }
        None => Ok(()),
    }
}

/// The peer configuration a relay's status carries (the reference's `getPeerConfig`): the node's
/// version, the network, and the components with an HTTP server's port: the API (when the
/// profile lists it and its options were read), and the webhooks entry although it is never
/// started; the peer-to-peer component is left out.
pub fn peer_config(relay: &RelayConfig, api: Option<&ApiConfig>) -> PeerConfig {
    let network = &relay.crypto.network;
    let mut plugins = Vec::new();
    if let Some(api) = api.filter(|_| relay.components.contains(&Package::Api)) {
        let plugin = PluginConfig {
            enabled: true,
            port: api.address.port().into(),
            estimate_total_count: Some(api.estimate_total_count),
        };
        plugins.push((Package::Api.name(), plugin));
    }
    if let Some(webhooks) = relay.entries.get(&Package::Webhooks) {
        let variable = |name: &str| relay.env.get(name).filter(|value| !value.is_empty());
        let port = (webhooks
            .pointer("/server/http/port")
            .map(ToString::to_string))
        .or_else(|| variable("CORE_WEBHOOKS_PORT").map(str::to_owned));
        let enabled = webhooks.get("enabled").and_then(serde_json::Value::as_bool);
        let plugin = PluginConfig {
            enabled: enabled.unwrap_or_else(|| variable("CORE_WEBHOOKS_ENABLED").is_some()),
            port: port
                .and_then(|port| port.trim_matches('"').parse().ok())
                .unwrap_or(4004),
            estimate_total_count: None,
        };
        plugins.push((Package::Webhooks.name(), plugin));
    }
    PeerConfig {
        version: VERSION.to_owned(),
        network: NetworkConfig {
            version: network.pub_key_hash,
            name: network.name.clone(),
            nethash: network.nethash.clone(),
            explorer: network.explorer.clone(),
            token: TokenConfig {
                name: network.token.clone(),
                symbol: network.symbol.clone(),
            },
        },
        plugins,
    }
}

/// The API's options: `None` when the relay's profile leaves the API out, or when one of them is
/// refused, which is logged and leaves the relay running without an API, as the reference's API
/// is not a required component. A host that does not resolve to an address stops the start, as
/// the reference's server cannot listen on it, and so does `CORE_API_SSL`: this relay serves no
/// HTTPS.
pub fn api_options(relay: &RelayConfig) -> Result<Option<ApiConfig>, RelayError> {
    if !relay.components.contains(&Package::Api) {
        return Ok(None);
    }
    match ApiConfig::from_options(&relay.entry(Package::Api), &relay.env) {
        Ok(api) if api.https => Err(config(
            "CORE_API_SSL is set, but this relay serves no HTTPS: serve its API through a reverse proxy that terminates TLS",
        )),
        Ok(api) => Ok(Some(api)),
        Err(error) if error.key == "server.http.host" && error.reason.contains("listen") => {
            Err(config(error))
        }
        Err(error) => {
            heartwood_kernel::error!("{error}: the API is not started");
            Ok(None)
        }
    }
}

/// The API of `config` over the node's snapshots and status, the monitor's peers and the store's
/// read-only connections `history`, served on `runtime` unless the configuration disables it.
async fn serve_api(
    config: ApiConfig,
    node: NodeInfo,
    handle: &NodeHandle,
    links: &Arc<Links>,
    status: &watch::Receiver<NodeStatus>,
    history: Readers,
    runtime: Handle,
) -> Result<Option<Api>, RelayError> {
    let history = Arc::new(history);
    let lookback = config.missed_blocks_lookback;
    let productivity = StoreProductivity::new(Arc::clone(&history), lookback)
        .with_epoch(node.crypto.milestones.at(1).epoch());
    let context = ApiContext {
        state: handle.state(),
        pool: handle.pool(),
        status: status.clone(),
        peers: Arc::clone(links) as Arc<dyn PeerList>,
        history: Arc::clone(&history),
        submit: Arc::new(Submit(handle.clone())),
        productivity: Arc::new(productivity),
        node: Arc::new(node),
        config: Arc::new(config),
    };
    let mut api = Api::new(Arc::new(context), runtime);
    if !api.enabled() {
        heartwood_kernel::info!("The API is disabled");
        return Ok(None);
    }
    api.boot().await.map_err(component)?;
    Ok(Some(api))
}

/// The store of `relay` and its restored state: by a replay when the reference's
/// `clear-saved-states.lock` asks for one, which is removed once the replay is done; from the
/// genesis block, in an emptied store, while `CORE_RESET_DATABASE` is set.
pub fn restored(relay: &RelayConfig) -> Result<(ChainStore, Restored), RelayError> {
    let mut store =
        ChainStore::open(&Settings::in_data_dir(&relay.paths.data)).map_err(component)?;
    if relay.env.is_set("CORE_RESET_DATABASE") {
        heartwood_kernel::warning!(
            "CORE_RESET_DATABASE is set: the store starts again from the genesis block"
        );
        store.database_mut().reset().map_err(component)?;
    }
    let marker = relay.replay_marker();
    let mode = if crate::markers::honoured(&marker, &relay.paths.data) {
        RestoreMode::Replay
    } else {
        RestoreMode::Auto
    };
    let restored = restore(&mut store, &relay.crypto, &Handlers, mode).map_err(component)?;
    if mode == RestoreMode::Replay {
        let _ = fs::remove_file(&marker);
    }
    Ok((store, restored))
}

/// The pool of `options`, its storage cleared by the reference's `clear-pool.lock`, and emptied
/// while `CORE_RESET_DATABASE` or `CORE_RESET_POOL` is set, as the reference's pool flushes itself
/// at its boot.
pub fn open_pool(relay: &RelayConfig, options: PoolOptions) -> Result<Pool, RelayError> {
    let marker =
        Some(relay.pool_marker()).filter(|m| crate::markers::honoured(m, &relay.paths.data));
    let mut pool = Pool::open(options, marker.as_deref()).map_err(component)?;
    if ["CORE_RESET_DATABASE", "CORE_RESET_POOL"]
        .iter()
        .any(|name| relay.env.is_set(name))
    {
        pool.flush().map_err(component)?;
    }
    Ok(pool)
}

/// The version floor of `relay`'s milestones at the stored tip `height`, with the peers'
/// version gate and the p2p option's minimum versions `configured` ([`super::check_floor`]).
pub fn version_floor(
    relay: &RelayConfig,
    height: u32,
    configured: &[String],
) -> Result<(), RelayError> {
    let files = &relay.crypto;
    super::check_floor(&files.milestones, &files.network.name, height, configured).map_err(config)
}

/// The peer-to-peer client of `options` (with the harness's `seed`) over the node's `view`,
/// reporting its peer events on `bus`, with the delegates on this node of `links` as the fork
/// check's local peer: the server's peer side over its monitor, its peers and its acceptance.
fn client(
    options: &Arc<P2pOptions>,
    seed: Option<IpAddr>,
    view: Arc<NodeView>,
    bus: &EventBus,
    links: &Arc<Links>,
    clock: &Arc<dyn Clock>,
) -> Result<MonitorSide<Client, Acceptor, Rules>, RelayError> {
    let rules = Arc::new(rules(options, seed));
    let repository = Arc::new(PeerRepository::new());
    let events: Arc<dyn heartwood_p2p::PeerEvents> = Arc::new(BusEvents(bus.clone()));
    let chain: Arc<dyn heartwood_p2p::ChainView> = view.clone();
    let runtime = Handle::current();
    let client = Arc::new(
        Communicator::new(CommunicatorParts {
            transport: Arc::new(Connector::for_addressing(&options.addressing)),
            verifier: Arc::new(PeerVerifier::new(
                Arc::clone(&chain),
                view.clone(),
                Arc::clone(clock),
            )),
            rules: Arc::clone(&rules),
            probe: Arc::new(HttpProbe {
                bind: options.addressing.bind_address(),
                ..HttpProbe::default()
            }),
            repository: Arc::clone(&repository),
            chain: Arc::clone(&chain),
            online: view,
            events: Arc::clone(&events),
            options: Arc::clone(options),
            clock: Arc::clone(clock),
            runtime: runtime.clone(),
        })
        .map_err(component)?,
    );
    let acceptor = Arc::new(PeerProcessor::new(ProcessorParts {
        client: Arc::clone(&client),
        rules: Arc::clone(&rules),
        repository: Arc::clone(&repository),
        events: Arc::clone(&events),
        options: Arc::clone(options),
        interfaces: interfaces(),
        clock: Arc::clone(clock),
    }));
    let sampler = Sampler::from_os().map_err(component)?;
    let parts = MonitorParts {
        client,
        acceptor: Arc::clone(&acceptor),
        rules,
        repository: Arc::clone(&repository),
        chain,
        events,
        options: Arc::clone(options),
        clock: Arc::clone(clock),
        sampler,
        runtime,
        attested: Arc::new(OnThisNode(Arc::clone(links))),
    };
    let (monitor, clock) = (Arc::new(NetworkMonitor::new(parts)), Arc::clone(clock));
    let runtime = Handle::current();
    Ok(MonitorSide {
        monitor,
        repository,
        acceptor,
        clock,
        runtime,
    })
}

/// The node's status for the API, refreshed every 250 ms and whenever it may have changed.
fn node_status(links: &Arc<Links>) -> watch::Receiver<NodeStatus> {
    let (sender, receiver) = watch::channel(NodeStatus::default());
    let links = Arc::clone(links);
    tokio::spawn(async move {
        while !sender.is_closed() {
            let network_height = links.peers.get().map_or(0, |peers| peers.network_height());
            let status = NodeStatus {
                synced: links.synced(),
                network_height,
            };
            sender.send_if_modified(|now| std::mem::replace(now, status) != status);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    receiver
}

/// The host's own addresses, which are never peers: Linux's local IPv4 routes and its IPv6
/// interface addresses (none elsewhere).
fn interfaces() -> Vec<IpAddr> {
    let read = |path: &str| fs::read_to_string(path).unwrap_or_default();
    let (mut addresses, mut last) = (Vec::new(), None);
    for line in read("/proc/net/fib_trie").lines() {
        let line = line.trim().trim_start_matches(['|', '+', '-', ' ']);
        match line.parse::<Ipv4Addr>() {
            Ok(address) => last = Some(address),
            Err(_) if line.contains("/32 host LOCAL") => addresses.extend(last.map(IpAddr::V4)),
            Err(_) => {}
        }
    }
    let hex = |line: &str| u128::from_str_radix(line.split_whitespace().next()?, 16).ok();
    addresses.extend(
        read("/proc/net/if_inet6")
            .lines()
            .filter_map(hex)
            .map(|bits| IpAddr::V6(Ipv6Addr::from(bits))),
    );
    addresses.sort();
    addresses.dedup();
    addresses
}

/// The client's options and the harness's seed, which `--difftest-seed` adds to the seeds.
fn seeded(
    mut options: P2pOptions,
    run: &RelayRun,
) -> Result<(P2pOptions, Option<IpAddr>), RelayError> {
    #[cfg(feature = "difftest")]
    if let Some(seed) = &run.difftest_seed {
        let address: std::net::SocketAddr =
            (seed.parse()).map_err(|_| config(format!("--difftest-seed {seed} is not ip:port")))?;
        let (ip, port) = (address.ip().to_string(), address.port());
        options.seeds.push(PeerAddress { ip, port });
        return Ok((options, Some(address.ip())));
    }
    let _ = (run, &mut options);
    Ok((options, None))
}

/// The client's rules of `options` (a devnet node's addressing among them), letting `seed`
/// through the address rules in a harness build.
fn rules(options: &P2pOptions, seed: Option<IpAddr>) -> Rules {
    let rules = heartwood_p2p::utils::rules::Rules::from_options(options);
    #[cfg(feature = "difftest")]
    let rules = crate::harness::SeedRules::new(rules, seed);
    #[cfg(not(feature = "difftest"))]
    let _ = seed;
    rules
}

#[cfg(test)]
mod tests {
    use heartwood_kernel::config::Env;
    use heartwood_p2p::devnet::Addressing;
    use serde_json::json;

    use super::*;

    fn node() -> P2pOptions {
        let address: IpAddr = "127.16.0.11".parse().unwrap();
        P2pOptions {
            host: address,
            addressing: Addressing::Alias { address },
            remote_access: vec!["127.16.0.11".to_owned()],
            ..P2pOptions::default()
        }
    }

    fn api(host: &str, enabled: bool) -> ApiConfig {
        let options = json!({ "server": { "http": { "host": host, "enabled": enabled } } });
        ApiConfig::from_options(&options, &Env::default()).unwrap()
    }

    #[test]
    fn a_devnet_node_serves_its_api_on_loopback_only() {
        let guard = |api: &ApiConfig| devnet_guard(&node(), "devnet", Some(api));
        assert!(guard(&api("127.16.0.11", true)).is_ok());
        assert!(guard(&api("127.0.0.1", true)).is_ok());
        for host in ["0.0.0.0", "10.0.0.5"] {
            let refused = guard(&api(host, true)).unwrap_err().to_string();
            assert!(
                refused.contains("the API") && refused.contains(host),
                "{refused}"
            );
        }
        assert!(
            guard(&api("0.0.0.0", false)).is_ok(),
            "an API that is off listens on nothing"
        );
        assert!(devnet_guard(&node(), "devnet", None).is_ok());
        let elsewhere = P2pOptions::default();
        assert!(devnet_guard(&elsewhere, "testnet", Some(&api("0.0.0.0", true))).is_ok());
    }
}
