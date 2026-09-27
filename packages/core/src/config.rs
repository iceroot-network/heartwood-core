//! The relay's configuration, read before anything starts: the token and the network (the flags,
//! else `config.json`, which never overrides a flag), the directories (`CORE_PATH_*`), the
//! environment (`.env` files), the relay's profile of `app.json` as compiled-in components, the
//! components' options, `peers.json` and the crypto files.
//!
//! What the relay does not honour, it names in [`RelayConfig::warnings`]: the reference's test
//! and skip variables, a profile's `flags` and an `app.js`, the forger and snapshot components in
//! a relay's profile, enabled webhooks, and option keys that no component knows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_kernel::config::crypto::{self, CryptoFiles};
use heartwood_kernel::config::merge::deep_merge;
use heartwood_kernel::config::{Env, PeersFile, Role, app, env, peers};
use heartwood_kernel::paths::{Paths, resolve_paths};
use serde_json::{Map, Value, json};

use crate::app::RelayError;
use crate::cli::RelayRun;

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

    /// The components a relay cannot run without.
    pub const REQUIRED: [Package; 6] = [
        Package::Database,
        Package::State,
        Package::Transactions,
        Package::Pool,
        Package::P2p,
        Package::Blockchain,
    ];

    /// The package's name and the option keys its defaults and schema know.
    #[rustfmt::skip]
    fn describe(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Package::Database => ("database", &["apiConnectionTimeout", "connection"]),
            Package::State => ("state", &["maxSavedStates", "maxSavedStateAge", "savedStatesPath", "storage", "walletSync"]),
            Package::Transactions => ("transactions", &[]),
            Package::Pool => ("pool", &["enabled", "storage", "maxTransactionsInPool", "maxTransactionsPerSender", "allowedSenders", "maxTransactionsPerRequest", "maxTransactionAge", "maxTransactionBytes", "dynamicFees", "workerPool"]),
            Package::P2p => ("p2p", &["server", "minimumVersions", "minimumNetworkReach", "verifyTimeout", "getBlocksTimeout", "maxSameSubnetPeers", "maxPeerSequentialErrors", "whitelist", "blacklist", "remoteAccess", "dns", "ntp", "rateLimit", "rateLimitPostTransactions", "networkStart", "disableDiscovery", "skipDiscovery", "ignoreMinimumNetworkReach"]),
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

/// The variables of the reference's test and skip paths, which the relay never honours.
const TEST_VARIABLES: [&str; 4] = [
    "CORE_ENV",
    "NODE_ENV",
    "CORE_SKIP_PEER_STATE_VERIFICATION",
    "CORE_SKIP_BLOCKCHAIN_STARTED_CHECK",
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
        let config = |error: String| RelayError::Config(error);
        let lookup = |name: &str| {
            process
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let (token, network) = identity(run, &lookup, home, cwd);
        let network = network.ok_or_else(|| {
            config("no network: give --network, or name it in config.json".into())
        })?;
        let paths = resolve_paths(&token, &network, &lookup, home, cwd);
        // The reference's process has these set when its `.env` files and its components' defaults
        // are read.
        let dirs = [
            ("CONFIG", &paths.config),
            ("DATA", &paths.data),
            ("CACHE", &paths.cache),
            ("LOG", &paths.log),
            ("TEMP", &paths.temp),
        ];
        let mut vars: Vec<(String, String)> = process.to_vec();
        vars.extend(
            dirs.map(|(kind, dir)| (format!("CORE_PATH_{kind}"), dir.display().to_string())),
        );
        vars.extend([
            ("CORE_TOKEN".into(), token.clone()),
            ("CORE_NETWORK_NAME".into(), network.clone()),
        ]);
        vars.push((
            "CORE_ENV".into(),
            run.env.clone().unwrap_or_else(|| "production".into()),
        ));
        let env = env::load(&paths.config, home, &token, vars)
            .map_err(|error| config(error.to_string()))?;
        let app =
            app::load(&paths.config, Role::Relay).map_err(|error| config(error.to_string()))?;
        let mut warnings: Vec<String> = app
            .ignored
            .iter()
            .map(|what| format!("{what} is not read"))
            .collect();
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
            let unknown = plugin
                .options
                .as_object()
                .into_iter()
                .flat_map(Map::keys)
                .filter(|key| !known.contains(&key.as_str()));
            warnings.extend(unknown.map(|key| {
                format!(
                    "the {} option {key} is not known and is not read",
                    plugin.package
                )
            }));
            entries.insert(
                package,
                deep_merge(&cli_options(run, package), &plugin.options),
            );
            if package <= Package::Api && !components.contains(&package) {
                components.push(package);
            }
        }
        components.sort();
        if let Some(missing) = Package::REQUIRED
            .into_iter()
            .find(|package| !components.contains(package))
        {
            return Err(config(format!(
                "the relay profile of app.json has no {}",
                missing.name()
            )));
        }
        for package in [Package::Forger, Package::Snapshots]
            .into_iter()
            .filter(|p| entries.contains_key(p))
        {
            warnings.push(format!("{} is not started by a relay", package.name()));
        }
        let webhooks = entries
            .get(&Package::Webhooks)
            .map(|options| options.get("enabled").is_some_and(|on| on == true));
        if webhooks.is_some_and(|on| on || env.is_set("CORE_WEBHOOKS_ENABLED")) {
            warnings.push("webhooks are not served by this relay".into());
        }
        let testing = |name: &str| {
            env.is_set(name) && (!name.ends_with("_ENV") || env.get(name) == Some("test"))
        };
        for name in TEST_VARIABLES.into_iter().filter(|name| testing(name)) {
            warnings.push(format!("{name} is not honoured"));
        }
        let peers = peers::load(&paths.config).map_err(|error| config(error.to_string()))?;
        heartwood_p2p::seeds::check_sources(&peers.sources)
            .map_err(|error| config(error.to_string()))?;
        let crypto = crypto::load(&paths.config.join("crypto"))
            .map_err(|error| config(error.to_string()))?;
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

/// The token and the network: the flags, each missing one from `config.json` (in
/// `CORE_PATH_CONFIG`, else in the network's configuration directory), the token `solar` by
/// default.
fn identity(
    run: &RelayRun,
    lookup: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    cwd: &Path,
) -> (String, Option<String>) {
    let (mut token, mut network) = (run.token.clone(), run.network.clone());
    if token.is_none() || network.is_none() {
        let named = |dir: PathBuf| std::fs::read(dir.join("config.json")).ok();
        let dir = lookup("CORE_PATH_CONFIG")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
        let text = dir.and_then(named).or_else(|| {
            let token = token.as_deref().unwrap_or("solar");
            let paths = resolve_paths(
                token,
                network.as_deref().unwrap_or_default(),
                &|_| None,
                home,
                cwd,
            );
            named(paths.config)
        });
        let file: Value = text
            .and_then(|text| serde_json::from_slice(&text).ok())
            .unwrap_or_default();
        let field = |key: &str| file.get(key).and_then(Value::as_str).map(str::to_owned);
        token = token.or_else(|| field("token"));
        network = network.or_else(|| field("network"));
    }
    (token.unwrap_or_else(|| "solar".into()), network)
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
