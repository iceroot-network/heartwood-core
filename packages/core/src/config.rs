//! The configuration of both roles, read before anything starts: the token and the network (the
//! flags, else `config.json`, which never overrides a flag), the directories (`CORE_PATH_*`), the
//! environment (`.env` files), the role's profile of `app.json` as compiled-in components, the
//! components' options and the crypto files; a relay also reads `peers.json`, which a forger
//! needs no more than it needs a store ([`RelayConfig`], [`ForgerConfig`]). A forger's profile may
//! hold only the logger, the transactions and the forger, and must hold the forger.
//!
//! What the relay does not honour, it names in [`RelayConfig::warnings`]: the reference's test
//! and skip variables, the API's cache and request log and the state's wallet table
//! (`CORE_API_CACHE`, `CORE_API_LOG`, `CORE_WALLET_SYNC_ENABLED`), a profile's `flags` and an
//! `app.js`, the forger and snapshot components in a relay's profile, enabled webhooks, the
//! state's options (its saved states, caches and wallet table), and option keys that no component
//! knows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_kernel::config::crypto::{self, CryptoFiles};
use heartwood_kernel::config::merge::deep_merge;
use heartwood_kernel::config::{Env, PeersFile, Role, app, env, peers};
use heartwood_kernel::paths::{Paths, resolve_paths};
use serde_json::{Map, Value, json};

use heartwood_cli::instance::check_name;
use heartwood_forger::PACKAGE;

use crate::app::RelayError;
use crate::commands::forger::ForgerRun;
use crate::commands::relay::RelayRun;

/// A component the relay knows by its package name, in the rank order the relay starts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Package {
    /// `@solar-network/database`.
    Database,
    /// `@solar-network/state`.
    State,
    /// `@solar-network/transactions`.
    Transactions,
    /// `@solar-network/pool`.
    Pool,
    /// `@solar-network/p2p`.
    P2p,
    /// `@solar-network/blockchain`.
    Blockchain,
    /// `@solar-network/api`, which a relay may leave out.
    Api,
    /// `@solar-network/logger`: the relay always logs, with these options when listed.
    Logger,
    /// `@solar-network/webhooks`: never started (a warning when enabled).
    Webhooks,
    /// `@solar-network/forger`: never started by a relay (a warning).
    Forger,
    /// `@solar-network/snapshots`: never started by a relay (a warning).
    Snapshots,
}

impl Package {
    /// Every package, in rank order.
    pub const ALL: [Package; 11] = [
        Package::Database,
        Package::State,
        Package::Transactions,
        Package::Pool,
        Package::P2p,
        Package::Blockchain,
        Package::Api,
        Package::Logger,
        Package::Webhooks,
        Package::Forger,
        Package::Snapshots,
    ];

    /// Whether a relay cannot run without the component: the database, the state, the
    /// transactions, the pool, the peer-to-peer network and the blockchain.
    pub fn required(self) -> bool {
        self <= Package::Blockchain
    }

    /// The package's name and the option keys its defaults and schema know.
    #[rustfmt::skip]
    fn describe(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Package::Database => ("database", &["apiConnectionTimeout", "connection"]),
            Package::State => ("state", &["maxSavedStates", "maxSavedStateAge", "savedStatesPath", "storage", "walletSync"]),
            Package::Transactions => ("transactions", &[]),
            Package::Pool => ("pool", &["enabled", "storage", "maxTransactionsInPool", "maxTransactionsPerSender", "allowedSenders", "maxTransactionsPerRequest", "maxTransactionAge", "maxTransactionBytes", "dynamicFees", "workerPool"]),
            Package::P2p => ("p2p", &["server", "minimumVersions", "minimumNetworkReach", "verifyTimeout", "getBlocksTimeout", "maxSameSubnetPeers", "maxPeerSequentialErrors", "whitelist", "blacklist", "remoteAccess", "dns", "ntp", "rateLimit", "rateLimitPostTransactions", "networkStart", "disableDiscovery", "skipDiscovery", "ignoreMinimumNetworkReach", "minimumDelegateReach"]),
            Package::Blockchain => ("blockchain", &["databaseRollback", "missedBlocksLookback", "networkStart"]),
            Package::Api => ("api", &["server", "plugins", "options", "ws"]),
            Package::Logger => ("logger", &["emojify", "levels", "fileRotator"]),
            Package::Webhooks => ("webhooks", &["enabled", "server", "timeout", "basePath"]),
            Package::Forger => ("forger", &[]),
            Package::Snapshots => ("snapshots", &[]),
        }
    }

    /// The package's name, such as `@solar-network/api`.
    pub fn name(self) -> String {
        format!("@solar-network/{}", self.describe().0)
    }

    /// The package named `name`, if the relay knows it.
    pub fn named(name: &str) -> Option<Package> {
        Package::ALL
            .into_iter()
            .find(|package| package.name() == name)
    }
}

/// The relay's configuration.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// The token of the node's directories.
    pub token: String,
    /// The network's name.
    pub network: String,
    /// The node's directories.
    pub paths: Paths,
    /// The node's environment.
    pub env: Env,
    /// The listed components the relay starts (the logger aside), in rank order.
    pub components: Vec<Package>,
    /// Each listed component's options: the entry's options over the command line's.
    pub entries: BTreeMap<Package, Value>,
    /// `peers.json`.
    pub peers: PeersFile,
    /// The crypto files.
    pub crypto: CryptoFiles,
    /// What the relay does not honour, for the start's warnings.
    pub warnings: Vec<String>,
}

/// The variables of the reference's test and skip paths, and of its API cache, API request log
/// and wallet table, which the relay never honours.
const UNHONOURED: [&str; 7] = [
    "CORE_ENV",
    "NODE_ENV",
    "CORE_SKIP_PEER_STATE_VERIFICATION",
    "CORE_SKIP_BLOCKCHAIN_STARTED_CHECK",
    "CORE_API_CACHE",
    "CORE_API_LOG",
    "CORE_WALLET_SYNC_ENABLED",
];

impl RelayConfig {
    /// The configuration of a relay run with the flags `run`, over the process's variables
    /// `process`, for the user whose home directory is `home`, in the working directory `cwd`.
    pub fn load(
        run: &RelayRun,
        process: &[(String, String)],
        home: &Path,
        cwd: &Path,
    ) -> Result<RelayConfig, RelayError> {
        RelayConfig::load_as(Role::Relay, run, process, home, cwd)
    }

    /// The configuration of the relay side of `role` (the relay, or the core role, whose profile
    /// also lists the forger), run with the flags `run`.
    pub fn load_as(
        role: Role,
        run: &RelayRun,
        process: &[(String, String)],
        home: &Path,
        cwd: &Path,
    ) -> Result<RelayConfig, RelayError> {
        let config = |error: String| RelayError::Config(error);
        let flags = (run.token.as_deref(), run.network.as_deref());
        let base = Base::load(role, flags, run.env.as_deref(), (process, home, cwd));
        let Base {
            token,
            network,
            paths,
            env,
            app,
            mut warnings,
        } = base.map_err(config)?;
        let (mut components, mut entries) = (Vec::new(), BTreeMap::new());
        for plugin in &app.plugins {
            let package = Package::named(&plugin.package);
            let package = package.ok_or_else(|| {
                config(format!(
                    "unknown package {:?} in the relay profile of app.json",
                    plugin.package
                ))
            })?;
            let known = package.describe().1;
            // The core role's forger entry is read by the forger's own options.
            let own = package == Package::Forger && role == Role::Core;
            // A devnet node's addressing is a p2p option of the devnet commands' own.
            let devnet =
                |key: &str| cfg!(feature = "devnet") && package == Package::P2p && key == "devnet";
            for key in plugin.options.as_object().into_iter().flat_map(Map::keys) {
                if own {
                    continue;
                }
                // The state's options are the reference's saved states, caches and wallet table.
                let what = match known.contains(&key.as_str()) || devnet(key) {
                    false => "is not known and is not read",
                    true if package == Package::State => "is not read by this relay",
                    true => continue,
                };
                warnings.push(format!("the {} option {key} {what}", plugin.package));
            }
            entries.insert(
                package,
                deep_merge(&cli_options(run, package), &plugin.options),
            );
            if package <= Package::Api && !components.contains(&package) {
                components.push(package);
            }
        }
        components.sort();
        let mut missing = Package::ALL.into_iter().filter(|p| p.required());
        if let Some(missing) = missing.find(|package| !components.contains(package)) {
            return Err(config(format!(
                "the relay profile of app.json has no {}",
                missing.name()
            )));
        }
        for package in [Package::Forger, Package::Snapshots]
            .into_iter()
            .filter(|p| entries.contains_key(p) && (*p, role) != (Package::Forger, Role::Core))
        {
            warnings.push(format!("{} is not started by a relay", package.name()));
        }
        let webhooks = entries
            .get(&Package::Webhooks)
            .map(|options| options.get("enabled").is_some_and(|on| on == true));
        if webhooks.is_some_and(|on| on || env.is_set("CORE_WEBHOOKS_ENABLED")) {
            warnings.push("webhooks are not served by this relay".into());
        }
        let peers = peers::load(&paths.config).map_err(|error| config(error.to_string()))?;
        heartwood_p2p::seeds::check_sources(&peers.sources)
            .map_err(|error| config(error.to_string()))?;
        let crypto = crypto::load(&paths.config.join("crypto"));
        let crypto = crypto.map_err(|error| config(error.to_string()))?;
        Ok(RelayConfig {
            token,
            network,
            paths,
            env,
            components,
            entries,
            peers,
            crypto,
            warnings,
        })
    }

    /// The forger side of a core role's configuration: its profile's forger entry (none when the
    /// profile lists no forger), the logger's options and the same directories, environment and
    /// crypto files.
    pub fn forger(&self) -> Option<ForgerConfig> {
        let forger = self.entries.get(&Package::Forger)?.clone();
        let app = app::AppConfig {
            plugins: Vec::new(),
            ignored: Vec::new(),
        };
        Some(ForgerConfig {
            base: Base {
                token: self.token.clone(),
                network: self.network.clone(),
                paths: self.paths.clone(),
                env: self.env.clone(),
                app,
                warnings: Vec::new(),
            },
            logger: self.entry(Package::Logger),
            forger,
            crypto: self.crypto.clone(),
        })
    }

    /// The options of `package` as the node gives them to the component: `{}` when unlisted.
    pub fn entry(&self, package: Package) -> Value {
        self.entries
            .get(&package)
            .cloned()
            .unwrap_or_else(|| json!({}))
    }

    /// Whether the state is to be replayed from the genesis block (the reference's
    /// `clear-saved-states.lock` in the temp directory, which a start then removes).
    pub fn replay_marker(&self) -> PathBuf {
        self.paths.temp.join("clear-saved-states.lock")
    }

    /// The marker that clears the pool's storage when the pool opens.
    pub fn pool_marker(&self) -> PathBuf {
        self.paths.temp.join("clear-pool.lock")
    }
}

/// What both roles read first: the token and the network (the flags, each missing one from
/// `config.json` in `CORE_PATH_CONFIG`, else in the network's configuration directory; the token
/// `solar` by default), the directories, the environment, the role's profile of `app.json`, the
/// warnings of what neither role reads or honours.
#[derive(Debug, Clone)]
pub struct Base {
    /// The token of the node's directories.
    pub token: String,
    /// The network's name.
    pub network: String,
    /// The node's directories.
    pub paths: Paths,
    /// The node's environment.
    pub env: Env,
    /// The role's profile of `app.json`.
    pub app: app::AppConfig,
    /// What the process does not read or honour, for the start's warnings.
    pub warnings: Vec<String>,
}

impl Base {
    /// The base of `role` with the flags `(token, network)` and `env_name`, over the process's
    /// variables, home directory and working directory.
    pub fn load(
        role: Role,
        (token, network): (Option<&str>, Option<&str>),
        env_name: Option<&str>,
        (process, home, cwd): (&[(String, String)], &Path, &Path),
    ) -> Result<Base, String> {
        let lookup = |name: &str| process.iter().find(|(key, _)| key == name).map(|(_, v)| v);
        let (mut token, mut network) = (token.map(str::to_owned), network.map(str::to_owned));
        if token.is_none() || network.is_none() {
            let named = |dir: PathBuf| std::fs::read(dir.join("config.json")).ok();
            let dir = lookup("CORE_PATH_CONFIG").filter(|dir| !dir.is_empty());
            let text = dir.map(PathBuf::from).and_then(named).or_else(|| {
                let (token, network) = (token.as_deref(), network.as_deref());
                let empty = |_: &str| None;
                let own = resolve_paths(
                    token.unwrap_or("solar"),
                    network.unwrap_or(""),
                    &empty,
                    home,
                    cwd,
                );
                named(own.config)
            });
            let file: Value = text
                .and_then(|t| serde_json::from_slice(&t).ok())
                .unwrap_or_default();
            let field = |key: &str| file.get(key).and_then(Value::as_str).map(str::to_owned);
            token = token.or_else(|| field("token"));
            network = network.or_else(|| field("network"));
        }
        let token = token.unwrap_or_else(|| "solar".into());
        let network = network.ok_or("no network: give --network, or name it in config.json")?;
        let named = check_name("token", &token).and_then(|()| check_name("network", &network));
        named.map_err(|error| error.to_string())?;
        let paths = resolve_paths(&token, &network, &|name| lookup(name).cloned(), home, cwd);
        // The reference's process has these set when its `.env` files and its components'
        // defaults are read.
        let dirs = [
            ("CONFIG", &paths.config),
            ("DATA", &paths.data),
            ("CACHE", &paths.cache),
        ];
        let dirs = dirs
            .into_iter()
            .chain([("LOG", &paths.log), ("TEMP", &paths.temp)]);
        let mut vars: Vec<(String, String)> = process.to_vec();
        vars.extend(
            dirs.map(|(kind, dir)| (format!("CORE_PATH_{kind}"), dir.display().to_string())),
        );
        let env_name = env_name.unwrap_or("production").to_owned();
        vars.extend([
            ("CORE_TOKEN".into(), token.clone()),
            ("CORE_NETWORK_NAME".into(), network.clone()),
        ]);
        vars.push(("CORE_ENV".into(), env_name));
        let text = |error: &dyn std::fmt::Display| error.to_string();
        let env = env::load(&paths.config, home, &token, vars).map_err(|e| text(&e))?;
        let app = app::load(&paths.config, role).map_err(|e| text(&e))?;
        let ignored = app.ignored.iter().map(|what| format!("{what} is not read"));
        let mut warnings: Vec<String> = ignored.collect();
        // A variable is set as the reference reads it; an environment's name only when `test`.
        let asked = |name: &&str| {
            env.is_set(name) && (!name.ends_with("_ENV") || env.get(name) == Some("test"))
        };
        let unhonoured = UNHONOURED.iter().filter(|name| asked(name));
        warnings.extend(unhonoured.map(|name| format!("{name} is not honoured")));
        Ok(Base {
            token,
            network,
            paths,
            env,
            app,
            warnings,
        })
    }
}

/// The forger's configuration.
#[derive(Debug, Clone)]
pub struct ForgerConfig {
    /// What every role reads.
    pub base: Base,
    /// The logger's options (`{}` when unlisted).
    pub logger: Value,
    /// The forger's options.
    pub forger: Value,
    /// The crypto files.
    pub crypto: CryptoFiles,
}

impl ForgerConfig {
    /// The configuration of a forger run with the flags `run`, over the process's variables
    /// `process`, for the user whose home directory is `home`, in the working directory `cwd`.
    pub fn load(
        run: &ForgerRun,
        process: &[(String, String)],
        home: &Path,
        cwd: &Path,
    ) -> Result<ForgerConfig, String> {
        let flags = (run.token.as_deref(), run.network.as_deref());
        let base = Base::load(
            Role::Forger,
            flags,
            run.env.as_deref(),
            (process, home, cwd),
        )?;
        let entry = |name: &str| {
            let mut plugins = base.app.plugins.iter();
            plugins
                .find(|plugin| plugin.package == name)
                .map(|plugin| plugin.options.clone())
        };
        let known = [
            "@solar-network/logger",
            "@solar-network/transactions",
            PACKAGE,
        ];
        if let Some(other) = (base.app.plugins.iter()).find(|p| !known.contains(&&*p.package)) {
            let other = &other.package;
            return Err(format!(
                "the forger profile of app.json lists {other:?}: a forger runs only the logger, the transactions and the forger"
            ));
        }
        let forger = entry(PACKAGE);
        let forger = forger.ok_or(format!("the forger profile of app.json has no {PACKAGE}"))?;
        let logger = entry("@solar-network/logger").unwrap_or_else(|| json!({}));
        let crypto = crypto::load(&base.paths.config.join("crypto"));
        let crypto = crypto.map_err(|error| error.to_string())?;
        Ok(ForgerConfig {
            base,
            logger,
            forger,
            crypto,
        })
    }
}

/// The options the command line gives `package`, as the reference's `relay:run` gives them:
/// `networkStart` to the blockchain, and the discovery flags (`--launchMode seed` skips the
/// discovery and the minimum reach) to the peer-to-peer network.
fn cli_options(run: &RelayRun, package: Package) -> Value {
    let seed = run.launch_mode.as_deref() == Some("seed");
    let flags: &[(&str, bool)] = match package {
        Package::Blockchain => &[("networkStart", run.network_start)],
        Package::P2p => &[
            ("networkStart", run.network_start),
            ("disableDiscovery", run.disable_discovery),
            ("skipDiscovery", run.skip_discovery || seed),
            (
                "ignoreMinimumNetworkReach",
                run.ignore_minimum_network_reach || seed,
            ),
        ],
        _ => &[],
    };
    let set = flags
        .iter()
        .filter(|(_, on)| *on)
        .map(|(key, _)| ((*key).to_owned(), Value::Bool(true)));
    Value::Object(set.collect())
}
