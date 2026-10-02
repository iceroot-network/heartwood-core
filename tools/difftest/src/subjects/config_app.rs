//! The node's configuration: `config.pluginOptions` (`{package, options, env}` to the options after
//! the package's schema and defaults, or the error) and `config.loadApp` (`app.json` text and a
//! role to the profile's components, or the error), answered with `heartwood_kernel::config` and
//! the components' options types.
//!
//! An accepted case's output is `{options}`, the options a relay reads of the package, in the
//! oracle's projection (`tools/oracle/lib/ops/config.js`): the peer-to-peer, pool and database
//! options as the relay reads them (`heartwood_core::options`), the blockchain's and the API's as
//! their components read them (their `Options` implementations). The pool's worker count, which
//! depends on the host's processors, is not in it. A refusal's class is not compared: the
//! reference names its schema's first failing key, Heartwood its own reader's.
//!
//! `config.loadApp` reads the profile with the kernel and maps each package to the relay's known
//! components ([`heartwood_core::config::Package`]); an unknown one refuses the file, as the
//! reference's plugin discovery does.
//!
//! `config.pluginStart` (the same input) answers what the relay does next with the options:
//! `{"next": "runs"}`, `"left out"` (the component is not started and the relay runs without it)
//! or `"start refused"`. The relay's start reads the pool's, the store's, the peer-to-peer
//! network's and the blockchain's options first and refuses the start when one is refused; the
//! API's options go through [`heartwood_core::app::api_options`], over a configuration that lists
//! the API with the case's options and environment.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_api::ApiConfig;
use heartwood_api::server::SemaphoreLevel;
use heartwood_core::app::api_options;
use heartwood_core::config::{Package, RelayConfig};
use heartwood_core::options;
use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_kernel::config::{Env as NodeEnv, Options, OptionsError, PeersFile, Role, app};
use heartwood_kernel::paths::Paths;
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed, text, unclassified};
use crate::subject::{Case, Verdict};

/// The operations of the node's configuration.
pub(super) const OPS: &[(&str, Run)] = &[
    ("config.pluginOptions", plugin_options),
    ("config.loadApp", load_app),
    ("config.pluginStart", plugin_start),
];

/// The processors a case's pool is read for; the worker count they set is not compared.
const CPUS: usize = 2;

/// A case's plugin entry options (`{}` when it has none) and its process environment.
fn entry_and_env(input: &Value) -> Outcome<(Value, Vec<(String, String)>)> {
    let entry = input.get("options").cloned().unwrap_or_else(|| json!({}));
    let vars = input["env"]
        .as_object()
        .ok_or_else(|| failed("input.env is not an object"))?;
    let vars = vars
        .iter()
        .map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())));
    let vars: Vec<(String, String)> = vars
        .collect::<Option<_>>()
        .ok_or_else(|| failed("input.env holds a value that is not text"))?;
    Ok((entry, vars))
}

fn plugin_start(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let (entry, vars) = entry_and_env(input)?;
    let node_env = NodeEnv::from_vars(vars);
    let name = text(input, "package")?;
    let read = match name {
        "@solar-network/p2p" => options::p2p(&entry, &node_env).map(drop),
        "@solar-network/blockchain" => options::blockchain(&entry).map(drop),
        "@solar-network/database" => options::database(&entry).map(drop),
        "@solar-network/pool" => {
            let data = node_env
                .get("CORE_PATH_DATA")
                .ok_or_else(|| failed("input.env has no CORE_PATH_DATA"))?;
            options::pool(&entry, &node_env, Path::new(data), CPUS).map(drop)
        }
        "@solar-network/api" => {
            let devnet = env.devnet()?;
            let data = PathBuf::from(
                node_env
                    .get("CORE_PATH_DATA")
                    .ok_or_else(|| failed("input.env has no CORE_PATH_DATA"))?,
            );
            let relay = RelayConfig {
                token: "solar".into(),
                network: "testnet".into(),
                paths: Paths {
                    config: data.join("config"),
                    cache: data.join("cache"),
                    log: data.join("log"),
                    temp: data.join("temp"),
                    data,
                },
                env: node_env,
                components: vec![Package::Api],
                entries: BTreeMap::from([(Package::Api, entry)]),
                peers: PeersFile {
                    list: Vec::new(),
                    sources: Vec::new(),
                },
                crypto: CryptoFiles {
                    network: devnet.network.clone(),
                    milestones: devnet.milestones.clone(),
                    milestones_text: String::new(),
                    genesis: Value::Null,
                },
                warnings: Vec::new(),
            };
            let next = match api_options(&relay) {
                Ok(Some(_)) => "runs",
                Ok(None) => "left out",
                Err(_) => "start refused",
            };
            return Ok(Verdict::Output(json!({ "next": next })));
        }
        other => {
            return Err(failed(format!(
                "input.package {other} is not a package the relay reads"
            )));
        }
    };
    let next = if read.is_ok() {
        "runs"
    } else {
        "start refused"
    };
    Ok(Verdict::Output(json!({ "next": next })))
}

fn plugin_options(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let (entry, vars) = entry_and_env(input)?;
    let env = NodeEnv::from_vars(vars);
    let projected: Result<Value, OptionsError> = match text(input, "package")? {
        "@solar-network/p2p" => options::p2p(&entry, &env).map(|o| {
            json!({
                "port": o.port,
                "minimumVersions": o.minimum_versions,
                "minimumNetworkReach": o.minimum_network_reach,
                "verifyTimeout": o.verify_timeout.as_millis(),
                "getBlocksTimeout": o.get_blocks_timeout.as_millis(),
                "maxSameSubnetPeers": o.max_same_subnet_peers,
                "maxPeerSequentialErrors": o.max_peer_sequential_errors,
                "whitelist": o.whitelist,
                "blacklist": o.blacklist,
                "rateLimit": o.rate_limit,
                "rateLimitPostTransactions": o.rate_limit_post_transactions,
                "networkStart": o.network_start,
                "disableDiscovery": o.disable_discovery,
                "skipDiscovery": o.skip_discovery,
                "ignoreMinimumNetworkReach": o.ignore_minimum_network_reach,
                "allowPrivateIpRanges": o.allow_private_ip_ranges,
            })
        }),
        "@solar-network/blockchain" => options::blockchain(&entry).map(|o| {
            json!({
                "databaseRollback": {
                    "maxBlockRewind": o.database_rollback.max_block_rewind,
                    "steps": o.database_rollback.steps,
                },
                "missedBlocksLookback": o.missed_blocks_lookback,
                "networkStart": o.network_start,
            })
        }),
        "@solar-network/pool" => {
            let data = env
                .get("CORE_PATH_DATA")
                .ok_or_else(|| failed("input.env has no CORE_PATH_DATA"))?;
            options::pool(&entry, &env, Path::new(data), CPUS).map(|o| {
                let fees = &o.dynamic_fees;
                let addon: Map<String, Value> = (fees.addon_bytes.iter()).map(|(key, bytes)| (key.clone(), (*bytes).into())).collect();
                json!({
                    "enabled": o.enabled,
                    "storage": o.storage.display().to_string(),
                    "maxTransactionsInPool": o.max_transactions_in_pool,
                    "maxTransactionsPerSender": o.max_transactions_per_sender,
                    "allowedSenders": o.allowed_senders.iter().map(|a| a.to_base58()).collect::<Vec<_>>(),
                    "maxTransactionsPerRequest": o.max_transactions_per_request,
                    "maxTransactionAge": o.max_transaction_age,
                    "maxTransactionBytes": o.max_transaction_bytes,
                    "dynamicFees": {
                        "enabled": fees.enabled,
                        "minFeePool": fees.min_fee_pool,
                        "minFeeBroadcast": fees.min_fee_broadcast,
                        "addonBytes": addon,
                    },
                })
            })
        }
        "@solar-network/api" => ApiConfig::from_options(&entry, &env).map(|o| api(&o)),
        "@solar-network/database" => {
            options::database(&entry).map(|timeout| json!({ "apiConnectionTimeout": timeout }))
        }
        other => {
            return Err(failed(format!(
                "input.package {other} is not a package the relay reads"
            )));
        }
    };
    Ok(match projected {
        Ok(options) => Verdict::Output(json!({ "options": options })),
        Err(error) => unclassified(error),
    })
}

/// The API's options in the oracle's projection.
fn api(o: &ApiConfig) -> Value {
    let level = |level: &SemaphoreLevel| {
        let mut out = Map::new();
        out.insert("concurrency".into(), level.concurrency.into());
        out.insert("queueLimit".into(), level.queue_limit.into());
        if let Some(offset) = level.max_offset {
            out.insert("maxOffset".into(), offset.into());
        }
        Value::Object(out)
    };
    let levels = |[one, two]: &[SemaphoreLevel; 2]| json!({ "levelOne": level(one), "levelTwo": level(two) });
    let (rate, semaphore) = (&o.rate_limit, &o.semaphore);
    json!({
        "enabled": o.enabled,
        "host": o.address.ip().to_string(),
        "port": o.address.port(),
        "https": o.https,
        "rateLimit": {
            "enabled": rate.enabled,
            "points": rate.points,
            "duration": rate.duration_s,
            "whitelist": rate.whitelist,
            "blacklist": rate.blacklist,
        },
        "semaphore": {
            "enabled": semaphore.enabled,
            "database": levels(&semaphore.database),
            "memory": levels(&semaphore.memory),
        },
        "socketTimeout": o.socket_timeout_ms,
        "whitelist": o.whitelist,
        "trustProxy": o.trust_proxy,
        "basePath": o.base_path,
        "estimateTotalCount": o.estimate_total_count,
    })
}

fn load_app(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let role = match text(case.input, "role")? {
        "relay" => Role::Relay,
        "forger" => Role::Forger,
        "core" => Role::Core,
        other => return Err(failed(format!("input.role {other} is not a role"))),
    };
    let profile = match app::parse(text(case.input, "app")?, role) {
        Ok(profile) => profile,
        Err(error) => return Ok(unclassified(error)),
    };
    if let Some(unknown) = profile
        .plugins
        .iter()
        .find(|plugin| Package::named(&plugin.package).is_none())
    {
        return Ok(unclassified(format!(
            "unknown package {:?}",
            unknown.package
        )));
    }
    let plugins = profile
        .plugins
        .iter()
        .map(|p| json!({ "package": p.package, "options": p.options }));
    Ok(Verdict::Output(
        json!({ "plugins": plugins.collect::<Vec<_>>() }),
    ))
}
