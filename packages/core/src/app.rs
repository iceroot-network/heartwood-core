//! The relay's composition root: the configuration ([`crate::config`]), the process lock, the
//! logger, then the components in rank order: the store and the state (restored, with the
//! version floor at the stored tip), the pool, the node thread, the peer-to-peer client, the
//! blockchain service and, once the blockchain has started, the API. It links the node's views
//! ([`crate::views`]) to them and runs until a signal or a fatal error, then shuts down in reverse:
//! the API stops accepting connections, the blockchain service stops the queue after its job, and
//! the node thread flushes the pool, writes the online record, and closes the store.
//!
//! The reference's marker files are honoured at the start: `clear-saved-states.lock` in the temp
//! directory restores the state by a replay (and is removed once it is done), and
//! `clear-pool.lock` clears the pool's storage when the pool opens.

use std::fmt::Display;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_api::{Api, ApiConfig, ApiContext, NodeInfo, NodeStatus, PeerList};
use heartwood_blockchain::state_machine::MachineState;
use heartwood_blockchain::{
    BlockProcessor, Blockchain, BlockchainParts, ChainState, ChainStore, Node, NodeError,
    NodeHandle, NodeParts, PoolTask, RestoreMode, RestorePath, Restored, RollbackDepth, restore,
};
use heartwood_crypto::Params;
use heartwood_database::{Readers, Settings};
use heartwood_kernel::bus::EventBus;
use heartwood_kernel::config::Options;
use heartwood_kernel::config::floor;
use heartwood_kernel::errors::BootError;
use heartwood_kernel::lifecycle::Component;
use heartwood_kernel::{Clock, SystemClock};
use heartwood_p2p::diversity::Sampler;
use heartwood_p2p::processor::ProcessorParts;
use heartwood_p2p::seeds::HttpProbe;
use heartwood_p2p::utils::is_valid_version::is_valid_version;
use heartwood_p2p::{
    Communicator, CommunicatorParts, Connector, MonitorParts, NetworkMonitor, P2pOptions,
};
use heartwood_p2p::{PeerProcessor, PeerRepository, PeerVerifier};
use heartwood_p2p_wire::replies::PeerAddress;
use heartwood_pool::Pool;
use heartwood_transactions::Handlers;
use thiserror::Error;
use tokio::runtime::{Builder, Handle};
use tokio::sync::{mpsc, watch};

use crate::cli::RelayRun;
use crate::config::{Package, RelayConfig};
use crate::lock::ProcessLock;
use crate::network::Monitored;
use crate::views::{BusEvents, Links, Listener, NodeView, Submit};
use crate::{VERSION, options, signals};

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
type Monitor = NetworkMonitor<Client, PeerProcessor<Client, Rules>, Rules>;

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
    let vars = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)));
    let process: Vec<(String, String)> = vars.collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let home = home.ok_or_else(|| config("HOME is not set"))?;
    let cwd = std::env::current_dir().map_err(config)?;
    start(RelayConfig::load(run, &process, &home, &cwd)?, run)
}

/// Run the relay of `relay`, whose flags are `run`.
pub fn start(relay: RelayConfig, run: &RelayRun) -> Result<(), RelayError> {
    let paths = &relay.paths;
    for dir in [
        &paths.config,
        &paths.data,
        &paths.cache,
        &paths.log,
        &paths.temp,
    ] {
        fs::create_dir_all(dir).map_err(|error| config(format!("{}: {error}", dir.display())))?;
    }
    let _lock = ProcessLock::acquire(&paths.data)?;
    let logger = options::logger(&relay.entry(Package::Logger), &relay.env).map_err(config)?;
    let stem = format!("{}-{}-relay", relay.token, relay.network);
    let _logs = heartwood_logger::init(&logger, &paths.log, &stem).map_err(component)?;
    heartwood_kernel::info!(
        "Heartwood Core {VERSION}: a relay of {} on {}",
        relay.token,
        relay.network
    );
    for warning in &relay.warnings {
        heartwood_kernel::warning!("{warning}");
    }
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(component)?;
    let api = (Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("heartwood-api")
        .enable_all()
        .build())
    .map_err(component)?;
    let result = runtime.block_on(serve(relay, run, api.handle().clone()));
    if let Err(error) = &result {
        heartwood_kernel::error!("{error}");
    }
    api.shutdown_timeout(Duration::from_secs(5));
    runtime.shutdown_timeout(Duration::from_secs(5));
    result
}

/// Start the components of `relay` and run them until a stop.
async fn serve(relay: RelayConfig, run: &RelayRun, api_runtime: Handle) -> Result<(), RelayError> {
    let (stops, mut stop) = mpsc::unbounded_channel();
    let signal = stops.clone();
    tokio::spawn(async move {
        signals::shutdown_requested().await;
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
    options::database(&relay.entry(Package::Database)).map_err(config)?;
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
    let blockchain_options = options::blockchain(&relay.entry(Package::Blockchain));
    let blockchain_options = Arc::new(blockchain_options.map_err(config)?);
    let api_config = relay
        .components
        .contains(&Package::Api)
        .then(|| ApiConfig::from_options(&relay.entry(Package::Api), &relay.env));
    let mut api_config = api_config.transpose().map_err(config)?;
    if api_config.as_ref().is_some_and(|api| api.https) {
        return Err(config(
            "CORE_API_SSL is set, but this relay serves no HTTPS: serve its API through a reverse proxy that terminates TLS",
        ));
    }
    // The store, the state and the version floor at the stored tip.
    let settings = Settings::in_data_dir(&relay.paths.data);
    let (store, restored) = restored(&relay)?;
    let files = &relay.crypto;
    version_floor(&relay, restored.state.height(), &p2p.minimum_versions)?;
    let pool = Pool::open(pool_options, Some(&relay.pool_marker())).map_err(component)?;
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
        observer: recorder(run, &milestones, first_start)?,
        broadcast: links.clone(),
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
    let monitor = client(seeded(p2p, run)?, view, &bus, &clock)?;
    let _ = links.peers.set(Arc::new(Arc::clone(&monitor)));
    let blockchain = Blockchain::new(BlockchainParts {
        node: handle.clone(),
        chain,
        network: Monitored(monitor),
        options: blockchain_options,
        milestones,
        clock,
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
    let (booted, service) = (stops, Arc::clone(&blockchain));
    tokio::spawn(async move { booted.send(Stop::Booted(service.boot().await)) });
    let status = node_status(&links);
    // Serve until a stop; the API once the blockchain has started, as the reference boots it.
    let (mut api, mut node_ended) = (None, false);
    let outcome = loop {
        match stop.recv().await {
            Some(Stop::Booted(Ok(()))) => {
                let Some(config) = api_config.take() else {
                    continue;
                };
                let node = NodeInfo {
                    crypto: relay.crypto.clone(),
                    version: VERSION.to_owned(),
                };
                // A failure to serve stops the relay, as the reference's server ends its
                // application when it cannot start.
                match serve_api(
                    config,
                    node,
                    &handle,
                    &links,
                    &status,
                    &settings,
                    api_runtime.clone(),
                )
                .await
                {
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
        }
    };
    // The graceful shutdown, in reverse.
    if let Some(mut api) = api {
        api.dispose().await;
    }
    blockchain.dispose().await;
    handle.stop().await;
    while !node_ended {
        match stop.recv().await {
            Some(Stop::Node(Err(error))) => return Err(RelayError::Fatal(error.to_string())),
            Some(Stop::Node(Ok(()))) | None => node_ended = true,
            Some(_) => {}
        }
    }
    outcome
}

/// The API of `config` over the node's snapshots and status, the monitor's peers and the store's
/// read-only connections, served on `runtime` unless the configuration disables it.
async fn serve_api(
    config: ApiConfig,
    node: NodeInfo,
    handle: &NodeHandle,
    links: &Arc<Links>,
    status: &watch::Receiver<NodeStatus>,
    settings: &Settings,
    runtime: Handle,
) -> Result<Option<Api>, RelayError> {
    let context = ApiContext {
        state: handle.state(),
        pool: handle.pool(),
        status: status.clone(),
        peers: Arc::clone(links) as Arc<dyn PeerList>,
        history: Arc::new(Readers::open(settings, 10).map_err(component)?),
        submit: Arc::new(Submit(handle.clone())),
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
/// `clear-saved-states.lock` asks for one, which is removed once the replay is done.
pub fn restored(relay: &RelayConfig) -> Result<(ChainStore, Restored), RelayError> {
    let mut store =
        ChainStore::open(&Settings::in_data_dir(&relay.paths.data)).map_err(component)?;
    let marker = relay.replay_marker();
    let mode = if marker.exists() {
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

/// The version floor of `relay`'s milestones at the stored tip `height`, with the peers' version
/// gate and the p2p option's minimum versions `configured`: refused when the floor in force
/// refuses this node's version; each later floor that refuses it is logged.
pub fn version_floor(
    relay: &RelayConfig,
    height: u32,
    configured: &[String],
) -> Result<(), RelayError> {
    let files = &relay.crypto;
    let gate = |version: &str, params: &Params, network: &str| {
        is_valid_version(Some(version), params, configured, network)
    };
    let later = floor::check(
        &files.milestones,
        VERSION,
        &files.network.name,
        height,
        &gate,
    )
    .map_err(config)?;
    for floor in later {
        let (height, ranges) = (floor.height, floor.ranges.join(", "));
        heartwood_kernel::warning!(
            "From height {height} the minimum versions are {ranges}, which this node's version {VERSION} does not satisfy"
        );
    }
    Ok(())
}

/// The peer-to-peer client of `options` (with the harness's seed) over the node's `view`,
/// reporting its peer events on `bus`.
fn client(
    (options, seed): (P2pOptions, Option<IpAddr>),
    view: Arc<NodeView>,
    bus: &EventBus,
    clock: &Arc<dyn Clock>,
) -> Result<Arc<Monitor>, RelayError> {
    let options = Arc::new(options);
    let rules = Arc::new(rules(&options, seed));
    let repository = Arc::new(PeerRepository::new());
    let events: Arc<dyn heartwood_p2p::PeerEvents> = Arc::new(BusEvents(bus.clone()));
    let chain: Arc<dyn heartwood_p2p::ChainView> = view.clone();
    let runtime = Handle::current();
    let client = Arc::new(
        Communicator::new(CommunicatorParts {
            transport: Arc::new(Connector::new()),
            verifier: Arc::new(PeerVerifier::new(
                Arc::clone(&chain),
                view.clone(),
                Arc::clone(clock),
            )),
            rules: Arc::clone(&rules),
            probe: Arc::new(HttpProbe::default()),
            repository: Arc::clone(&repository),
            chain: Arc::clone(&chain),
            online: view,
            events: Arc::clone(&events),
            options: Arc::clone(&options),
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
        options: Arc::clone(&options),
        interfaces: interfaces(),
        clock: Arc::clone(clock),
    }));
    let sampler = Sampler::from_os().map_err(component)?;
    let parts = MonitorParts {
        client,
        acceptor,
        rules,
        repository,
        chain,
        events,
        options,
        clock: Arc::clone(clock),
        sampler,
        runtime,
    };
    Ok(Arc::new(NetworkMonitor::new(parts)))
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
#[cfg(feature = "difftest")]
fn seeded(
    mut options: P2pOptions,
    run: &RelayRun,
) -> Result<(P2pOptions, Option<IpAddr>), RelayError> {
    let Some(seed) = &run.difftest_seed else {
        return Ok((options, None));
    };
    let address: std::net::SocketAddr = seed
        .parse()
        .map_err(|_| config(format!("--difftest-seed {seed} is not ip:port")))?;
    options.seeds.push(PeerAddress {
        ip: address.ip().to_string(),
        port: address.port(),
    });
    Ok((options, Some(address.ip())))
}

/// The client's options.
#[cfg(not(feature = "difftest"))]
fn seeded(
    options: P2pOptions,
    _run: &RelayRun,
) -> Result<(P2pOptions, Option<IpAddr>), RelayError> {
    Ok((options, None))
}

/// The client's rules of `options`, letting `seed` through the address rules.
#[cfg(feature = "difftest")]
fn rules(options: &P2pOptions, seed: Option<IpAddr>) -> Rules {
    let rules = heartwood_p2p::utils::rules::Rules::new(
        options.rate_limit,
        options.rate_limit_post_transactions,
    );
    crate::harness::SeedRules::new(rules, seed)
}

/// The client's rules of `options`.
#[cfg(not(feature = "difftest"))]
fn rules(options: &P2pOptions, _seed: Option<IpAddr>) -> Rules {
    Rules::new(options.rate_limit, options.rate_limit_post_transactions)
}

/// The harness's recorder of `--difftest-digest` and `--difftest-blocks`.
#[cfg(feature = "difftest")]
fn recorder(
    run: &RelayRun,
    milestones: &Arc<heartwood_crypto::Milestones>,
    first_start: bool,
) -> Result<Option<Box<dyn heartwood_blockchain::events::CommitObserver>>, RelayError> {
    let (digests, blocks) = (
        run.difftest_digest.as_deref(),
        run.difftest_blocks.as_deref(),
    );
    if digests.is_none() && blocks.is_none() {
        return Ok(None);
    }
    let recorder =
        crate::harness::Recorder::open(Arc::clone(milestones), digests, blocks, first_start);
    Ok(Some(Box::new(recorder.map_err(config)?)))
}

/// No recorder without the harness.
#[cfg(not(feature = "difftest"))]
fn recorder(
    _run: &RelayRun,
    _milestones: &Arc<heartwood_crypto::Milestones>,
    _first_start: bool,
) -> Result<Option<Box<dyn heartwood_blockchain::events::CommitObserver>>, RelayError> {
    Ok(None)
}
