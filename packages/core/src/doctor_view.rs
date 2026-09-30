//! [`CoreNodeView`]: the doctor's view of the node through the node's own code: the relay's and
//! the forger's configuration as their loaders compute it, the version floor with the P2P crate's
//! version gate, and the relay's store read through a read-only connection while the relay runs.
//! Only typed facts and variable names cross the seam, never a `.env` value, and nothing here
//! reads the key file.
//!
//! The loaders read the instance the doctor examines: the process's variables are given the
//! instance's five directories. The API's host is resolved only when it is an address: a host name
//! would need a name lookup, which the doctor never makes, so the API then counts as not served.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use heartwood_cli::Instance;
use heartwood_cli::doctor::seams::{
    AccessEntry, BuildFacts, ChainFacts, FloorFacts, ForgerFacts, LoadRefusal, MissedSlot,
    NodeView, NodeViewError, RelayFacts, StoreWindow, StoredBlock,
};
use heartwood_cli::supervisor::signals::is_locked;
use heartwood_crypto::Params;
use heartwood_crypto::crypto::slots;
use heartwood_database::Settings;
use heartwood_database::reader_ro::{ReadOnlyError, ReadOnlyStore};
use heartwood_kernel::config::{self, CryptoFiles, dot_env, floor};
use heartwood_logger::Level;
use heartwood_p2p::P2pOptions;
use heartwood_p2p::utils::is_valid_version::is_valid_version;

use crate::commands::{forger::ForgerRun, relay::RelayRun};
use crate::config::{ForgerConfig, Package, RelayConfig};
use crate::options;

/// How long a statement of the store window may run.
pub const STORE_TIMEOUT: Duration = Duration::from_secs(1);

/// The node's view for the doctor, with the environment, home directory and working directory the
/// node's loaders read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreNodeView {
    /// The process's variables, as a role's start reads them.
    pub process: Vec<(String, String)>,
    /// The home directory.
    pub home: PathBuf,
    /// The working directory.
    pub cwd: PathBuf,
}

impl CoreNodeView {
    /// The view with this process's variables, home and working directory, as a role's start
    /// reads them.
    pub fn from_process() -> CoreNodeView {
        let (process, home, cwd) = crate::app::relay::process_context().unwrap_or_default();
        CoreNodeView { process, home, cwd }
    }

    /// The home directory; a start without one is refused, as the roles' starts refuse it.
    fn home(&self) -> Result<&Path, NodeViewError> {
        let (kind, file) = (LoadRefusal::Other, None);
        let home = Some(self.home.as_path()).filter(|home| !home.as_os_str().is_empty());
        home.ok_or(NodeViewError::Refused { kind, file })
    }

    /// The process's variables with `instance`'s five directories as its `CORE_PATH_*`.
    fn vars(&self, instance: &Instance) -> Vec<(String, String)> {
        let p = &instance.paths;
        let dirs = [("CONFIG", &p.config), ("DATA", &p.data)].into_iter();
        let dirs = dirs.chain([("CACHE", &p.cache), ("LOG", &p.log), ("TEMP", &p.temp)]);
        let dirs = dirs.map(|(kind, dir)| (format!("CORE_PATH_{kind}"), dir.display().to_string()));
        let own = self.process.iter();
        let own = own.filter(|(k, _)| !k.starts_with("CORE_PATH_"));
        own.cloned().chain(dirs).collect()
    }

    /// Why `instance`'s loader refused: the first of the steps it takes, in its order, that fails.
    fn refusal(&self, instance: &Instance, role: config::Role) -> NodeViewError {
        let dir = &instance.paths.config;
        let env = config::env::load(dir, &self.home, &instance.token, self.vars(instance));
        let peers = || {
            let peers = config::peers::load(dir).map_err(drop)?;
            heartwood_p2p::seeds::check_sources(&peers.sources).map_err(drop)
        };
        let (relay, app) = (
            role == config::Role::Relay,
            config::app::load(dir, role).is_err(),
        );
        let crypto = config::crypto::load(&dir.join("crypto")).is_err();
        let steps = [
            (LoadRefusal::EnvFile, ".env", env.is_err()),
            (LoadRefusal::AppProfile, "app.json", app),
            (LoadRefusal::Peers, "peers.json", relay && peers().is_err()),
            (LoadRefusal::Crypto, "crypto", crypto),
        ];
        let failed = steps.into_iter().find(|(_, _, failed)| *failed);
        let (kind, file, _) = failed.unwrap_or((LoadRefusal::AppProfile, "app.json", true));
        let file = Some(env.err().map_or_else(|| dir.join(file), |error| error.path));
        NodeViewError::Refused { kind, file }
    }

    /// The relay's configuration and its peer-to-peer options, as its start reads them.
    fn load_relay(&self, relay: &Instance) -> Result<(RelayConfig, P2pOptions), NodeViewError> {
        let mut run = RelayRun::default();
        (run.token, run.network) = (Some(relay.token.clone()), Some(relay.network.clone()));
        let loaded = RelayConfig::load(&run, &self.vars(relay), self.home()?, &self.cwd);
        let config = loaded.map_err(|_| self.refusal(relay, config::Role::Relay))?;
        let p2p = options::p2p(&config.entry(Package::P2p), &config.env);
        let p2p = p2p.map_err(|_| options_refused(&config.paths.config))?;
        Ok((config, p2p))
    }
}

/// A refusal of a component's options, which live in `app.json`.
fn options_refused(config: &Path) -> NodeViewError {
    let (kind, file) = (LoadRefusal::Options, Some(config.join("app.json")));
    NodeViewError::Refused { kind, file }
}

/// The chain's facts: the epoch, and the latest milestone's block time and active delegates.
fn chain(crypto: &CryptoFiles) -> ChainFacts {
    let milestones = &crypto.milestones;
    let latest = milestones.all().last().unwrap_or(milestones.at(1));
    ChainFacts {
        nethash: crypto.network.nethash.clone(),
        epoch_ms: slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap_or_default(),
        block_time_s: u32::try_from(latest.block_time()).unwrap_or(u32::MAX),
        active_delegates: u32::try_from(latest.active_delegates()).unwrap_or(u32::MAX),
    }
}

/// A `remoteAccess` entry as the relay matches it.
fn access(entry: &str) -> AccessEntry {
    let prefix = |(address, bits): (&str, &str)| {
        let (address, bits) = (address.parse().ok()?, bits.parse().ok()?);
        Some(AccessEntry::Prefix { address, bits })
    };
    let other = || entry.split_once('/').and_then(prefix);
    let parsed = entry.parse().map(AccessEntry::Address).ok();
    parsed.or_else(other).unwrap_or(AccessEntry::Other)
}

/// The names the `.env` files of `config` and the home directory set, `[A-Z0-9_]` only: their
/// values are parsed by the node's own parser and dropped.
fn env_names(config: &Path, home: &Path, token: &str) -> Vec<String> {
    let files = [config.join(".env"), home.join(format!(".{token}/.env"))];
    let texts = files.iter().filter_map(|file| std::fs::read(file).ok());
    let keys = texts.flat_map(|text| dot_env::parse(&String::from_utf8_lossy(&text)).into_keys());
    let plain = |b: u8| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_';
    let named = |name: &String| !name.is_empty() && name.bytes().all(plain);
    let names: std::collections::BTreeSet<String> = keys.filter(named).collect();
    names.into_iter().collect()
}

/// The store window's error as the seam gives it.
fn store_error(error: ReadOnlyError) -> NodeViewError {
    match error {
        ReadOnlyError::NoWriter => NodeViewError::RelayNotRunning,
        ReadOnlyError::TimedOut => NodeViewError::StoreTimedOut,
        ReadOnlyError::Database(_) => NodeViewError::Store,
    }
}

impl NodeView for CoreNodeView {
    fn build(&self) -> BuildFacts {
        let on = [cfg!(feature = "devnet"), cfg!(feature = "difftest")];
        let names = ["devnet", "difftest"].into_iter().zip(on);
        let features = names.filter_map(|(name, on)| on.then_some(name)).collect();
        let version = crate::VERSION;
        BuildFacts { version, features }
    }

    fn relay(&self, relay: &Instance) -> Result<RelayFacts, NodeViewError> {
        let (config, p2p) = self.load_relay(relay)?;
        let refused = || options_refused(&config.paths.config);
        let logger = options::logger(&config.entry(Package::Logger), &config.env);
        let logger = logger.map_err(|_| refused())?;
        // A host name would be looked up: the API is then left out rather than resolved.
        let api = config.entry(Package::Api);
        let host = api.pointer("/server/http/host").map(|h| h.as_str());
        let host = host.unwrap_or(config.env.get("CORE_API_HOST").filter(|h| !h.is_empty()));
        let api = match host.is_some_and(|host| host.parse::<IpAddr>().is_err()) {
            true => None,
            false => crate::app::api_options(&config).map_err(|_| refused())?,
        };
        Ok(RelayFacts {
            p2p: SocketAddr::new(p2p.host, p2p.port),
            api: api.filter(|api| api.enabled).map(|api| api.address),
            remote_access: p2p.remote_access.iter().map(|e| access(e)).collect(),
            minimum_network_reach: u32::try_from(p2p.minimum_network_reach).unwrap_or(u32::MAX),
            file_log_level: logger.file.unwrap_or(Level::Emergency),
            warnings: config.warnings.clone(),
            env_names: env_names(&config.paths.config, &self.home, &config.token),
            chain: chain(&config.crypto),
            store_file: Settings::in_data_dir(&config.paths.data).path,
        })
    }

    fn forger(&self, forger: &Instance) -> Result<ForgerFacts, NodeViewError> {
        let mut run = ForgerRun::default();
        (run.token, run.network) = (Some(forger.token.clone()), Some(forger.network.clone()));
        let loaded = ForgerConfig::load(&run, &self.vars(forger), self.home()?, &self.cwd);
        let config = loaded.map_err(|_| self.refusal(forger, config::Role::Forger))?;
        let paths = &config.base.paths;
        let env = |name: &str| config.base.env.get(name).map(str::to_owned);
        let options = heartwood_forger::Options::from_json(&config.forger, &env);
        let options = options.map_err(|_| options_refused(&paths.config))?;
        let logger = options::logger(&config.logger, &config.base.env);
        let logger = logger.map_err(|_| options_refused(&paths.config))?;
        let hosts = options.hosts.iter();
        Ok(ForgerFacts {
            hosts: hosts.map(|h| SocketAddr::new(h.address, h.port)).collect(),
            local_address: options.local_address,
            aux_set: options.aux.is_some(),
            file_log_level: logger.file.unwrap_or(Level::Emergency),
            key_file: heartwood_forger::keys::path(&paths.config),
            signing_record: heartwood_forger::signing_record::path(&paths.data),
            chain: chain(&config.crypto),
        })
    }

    fn floor(
        &self,
        relay: &Instance,
        version: &str,
        height: u32,
    ) -> Result<FloorFacts, NodeViewError> {
        let (config, p2p) = self.load_relay(relay)?;
        let (milestones, network) = (&config.crypto.milestones, &config.crypto.network.name);
        let gate = |version: &str, params: &Params, network: &str| {
            is_valid_version(Some(version), params, &p2p.minimum_versions, network)
        };
        // The later milestones that refuse the version, as the floor's own warnings list them.
        let later = |params: &&Params| params.height() > height && !gate(version, params, network);
        let refused_now = floor::check(milestones, version, network, height, &gate).is_err();
        let refused_from = milestones.all().iter().filter(later).map(Params::height);
        let refused_from = refused_from.collect();
        Ok(FloorFacts {
            refused_now,
            refused_from,
        })
    }

    fn store_window(
        &self,
        relay: &Instance,
        usernames: &[String],
        from_height: u32,
    ) -> Result<StoreWindow, NodeViewError> {
        // The lock's momentary shared probe, as `status` takes it: no writer, no open.
        if !is_locked(&relay.lock_path()).unwrap_or(false) {
            return Err(NodeViewError::RelayNotRunning);
        }
        let settings = Settings::in_data_dir(&relay.paths.data);
        let store = ReadOnlyStore::open(&settings, STORE_TIMEOUT).map_err(store_error)?;
        // A relay that stopped while the store was opened is not read.
        if !is_locked(&relay.lock_path()).unwrap_or(false) {
            return Err(NodeViewError::RelayNotRunning);
        }
        let rows = store.window(usernames, from_height).map_err(store_error)?;
        let missed = rows.missed.into_iter().map(|row| MissedSlot {
            timestamp: row.timestamp,
            height: row.height,
            username: row.username,
        });
        let blocks = rows.blocks.into_iter().map(|row| StoredBlock {
            height: row.height,
            timestamp: row.timestamp,
            id: row.id,
            generator_public_key: row.generator_public_key,
            username: row.username,
        });
        let (missed, blocks, capped) = (missed.collect(), blocks.collect(), rows.capped);
        Ok(StoreWindow {
            missed,
            blocks,
            capped,
        })
    }
}
