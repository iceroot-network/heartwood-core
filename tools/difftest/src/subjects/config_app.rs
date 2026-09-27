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

use std::path::Path;

use heartwood_api::ApiConfig;
use heartwood_api::server::SemaphoreLevel;
use heartwood_core::config::Package;
use heartwood_core::options;
use heartwood_kernel::config::{Env as NodeEnv, Options, OptionsError, Role, app};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed, text, unclassified};
use crate::subject::{Case, Verdict};

/// The operations of the node's configuration.
pub(super) const OPS: &[(&str, Run)] = &[
    ("config.pluginOptions", plugin_options),
    ("config.loadApp", load_app),
];

/// The processors a case's pool is read for; the worker count they set is not compared.
const CPUS: usize = 2;

fn plugin_options(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
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
