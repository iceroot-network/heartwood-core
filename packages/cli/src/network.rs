//! `network generate`: the files of a new network, from a profile and a seed.
//!
//! - The four crypto files of `heartwood_crypto::genesis::build`, byte for byte as the devnet
//!   generator writes them.
//! - `genesis-wallet.json` (mode 0600): every derived key of the chain, as the genesis vectors
//!   record them: `generator`, `genesis` (the genesis wallets), `donations`, `wallets` (the test
//!   wallets, not funded at genesis) and `delegates` (with their private keys).
//! - `delegates.json` (mode 0600): `{"keys": [...]}`, every delegate's private key in genesis
//!   order, the forger's key file.
//! - Unless `--chain-only`, a single-node instance: `.env` with loopback hosts and the testnet
//!   ports (P2P 6002, API 6003), `app.json` (the relay's and the forger's components, and the
//!   combined core role's, the relay's followed by the forger; the relay admitting the loopback
//!   forger, the forger with the seed's aux), `peers.json` naming the node itself, and
//!   `config.json` (token `solar` and the network).
//! - The same seed and flags give the same bytes; the epoch is the clock's current second when
//!   none is given. A directory that is not empty is refused without `--force`, which overwrites
//!   the files written and leaves others.
//! - The devnet profile, which the genesis vectors record, is built in; `--profile FILE` names
//!   another in the same format. `--network`, `--pub-key-hash`, `--token`, `--symbol` and
//!   `--explorer` replace the profile's values.

use std::fs::{self, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use heartwood_crypto::Network;
use heartwood_crypto::errors::GenesisError;
use heartwood_crypto::genesis::{
    self, Allocation, ChainDefinition, DerivedKey, DonationShare, GenesisDelegate, GenesisInput,
    RuleOverride, derive,
};
use heartwood_crypto::utils::{hex, js};
use heartwood_kernel::Clock;
use heartwood_kernel::timestamp::UtcDateTime;
use nix::fcntl::OFlag;
use serde_json::{Map, Value, json};
use zeroize::Zeroizing;

use crate::errors::NetworkError;
use crate::instance::{DEFAULT_TOKEN, pick};
use crate::seams::{ChainGenerator, GenerateOptions, Generated, GeneratedDelegate, Profile};

/// The devnet profile: the chain definition the genesis vectors record.
pub const DEVNET_PROFILE: &str = include_str!("profiles/devnet.json");

/// The single-node instance's peer-to-peer and API ports: the testnet profile's.
const PORTS: (u16, u16) = (6002, 6003);

/// The warning at the top of `genesis-wallet.json`.
const WARNING: &str = "DEVNET ONLY. Derived from a seed: anyone with the seed has these keys. \
                       Never use them for anything of value.";

/// A file to write: its path under the output directory, its text, and whether it holds keys.
type File = (&'static str, Zeroizing<String>, bool);

/// Build and write the network of `options`, the epoch from `clock` when the options give none.
pub fn generate(options: &GenerateOptions, clock: &dyn Clock) -> Result<Generated, NetworkError> {
    let (name, text) = match &options.profile {
        Profile::Devnet => ("devnet".to_owned(), Ok(DEVNET_PROFILE.to_owned())),
        Profile::File(path) => (path.display().to_string(), fs::read_to_string(path)),
    };
    let invalid = |reason: String| NetworkError::Profile {
        name: name.clone(),
        reason,
    };
    let text = text.map_err(|e| invalid(e.to_string()))?;
    let profile: Value = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
    let epoch = match &options.epoch {
        Some(epoch) => normalise_epoch(epoch),
        None => clock_epoch(clock)?,
    };
    let (input, supply) = input_of(&profile, options, epoch).map_err(invalid)?;
    let chain = genesis::build(&input).map_err(|e| match e {
        GenesisError::Epoch(epoch) => NetworkError::Epoch { epoch },
        other => NetworkError::Genesis(other.to_string()),
    })?;
    if chain.summary.issued != supply {
        let issued = chain.summary.issued;
        return Err(invalid(format!(
            "the genesis issues {issued}, but the supply is {supply}"
        )));
    }
    let forger_aux = derive::aux(&input.derivation_tag, "forger", &input.seed);
    let files = files(&chain, &input, options, &forger_aux);
    write_all(&options.out, &files, options.force)?;
    let delegates = input
        .delegates
        .iter()
        .zip(&chain.delegates)
        .map(|(delegate, key)| GeneratedDelegate {
            username: delegate.username.clone(),
            public_key: key.keys.public_key().to_hex(),
            secret_key: key.keys.secret_key().to_hex(),
        })
        .collect();
    Ok(Generated {
        out: options.out.clone(),
        nethash: hex::encode(&chain.summary.nethash),
        genesis_id: chain.summary.genesis_id.to_hex(),
        epoch: input.epoch.clone(),
        delegates,
        forger_aux,
        files: files.iter().map(|(path, ..)| PathBuf::from(path)).collect(),
    })
}

/// The [`ChainGenerator`] over [`generate`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkGenerator;

impl ChainGenerator for NetworkGenerator {
    fn generate(
        &self,
        options: &GenerateOptions,
        clock: &dyn Clock,
    ) -> Result<Generated, NetworkError> {
        generate(options, clock)
    }
}

/// `2026-01-01T00:00:00Z` as `2026-01-01T00:00:00.000Z`; any other text unchanged (the builder
/// refuses what is not a whole second in that form).
fn normalise_epoch(epoch: &str) -> String {
    match epoch.strip_suffix('Z') {
        Some(time) if epoch.len() == 20 && !time.contains('.') => format!("{time}.000Z"),
        _ => epoch.to_owned(),
    }
}

/// The clock's current second, as `Date.prototype.toISOString` writes it.
fn clock_epoch(clock: &dyn Clock) -> Result<String, NetworkError> {
    let now = clock.now_ms();
    now.div_euclid(1000)
        .checked_mul(1000)
        .and_then(UtcDateTime::from_unix_ms)
        .map(|time| time.to_iso_string())
        .ok_or(NetworkError::Epoch {
            epoch: now.to_string(),
        })
}

/// The builder's input for the profile `profile` and `options`, and the profile's supply.
fn input_of(
    profile: &Value,
    options: &GenerateOptions,
    epoch: String,
) -> Result<(GenesisInput, u64), String> {
    let amount = |value: &Value, key: &str| -> Result<u64, String> {
        let digits: String = pick!(value, key)?;
        let canonical = digits.bytes().all(|b| b.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'));
        let parsed = canonical.then(|| digits.parse().ok()).flatten();
        parsed.ok_or(format!("{key} {digits:?} is not an amount"))
    };
    let numbered = |pattern: &Value, key: &str| -> Result<Vec<String>, String> {
        let (count, prefix): (u32, String) = (pick!(pattern, "count")?, pick!(pattern, key)?);
        Ok((1..=count).map(|k| format!("{prefix}{k}")).collect())
    };
    let Some(Value::Object(mut network)) = profile.get("network").cloned() else {
        return Err("network is not an object".into());
    };
    network.insert("name".into(), options.network.clone().into());
    network.insert("nethash".into(), "".into());
    if let Some(byte) = options.pub_key_hash {
        network.insert("pubKeyHash".into(), byte.into());
    }
    let client = [
        ("token", &options.token),
        ("symbol", &options.symbol),
        ("explorer", &options.explorer),
    ];
    if let Some(Value::Object(names)) = network.get_mut("client") {
        let given = client
            .into_iter()
            .filter_map(|(key, value)| Some((key.into(), value.clone()?.into())));
        names.extend(given);
    }
    let network = Network::load(&Value::Object(network).to_string()).map_err(|e| e.to_string())?;
    let delegates = &profile["delegates"];
    let allocations: Vec<Value> = pick!(profile, "allocations")?;
    let donations: Vec<Value> = pick!(profile, "donations")?;
    let overrides: Map<String, Value> = pick!(profile, "overrides")?;
    let input = GenesisInput {
        seed: options.seed.clone(),
        derivation_tag: pick!(profile, "derivationTag")?,
        epoch,
        network,
        rules: pick!(profile, "rules")?,
        overrides: overrides
            .into_iter()
            .map(|(path, value)| RuleOverride { path, value })
            .collect(),
        generator_role: pick!(profile, "generatorRole")?,
        allocations: allocations
            .iter()
            .map(|a| {
                Ok(Allocation {
                    role: pick!(a, "role")?,
                    amount: amount(a, "amount")?,
                })
            })
            .collect::<Result<_, String>>()?,
        delegates: numbered(delegates, "rolePrefix")?
            .into_iter()
            .zip(numbered(delegates, "usernamePrefix")?)
            .map(|(role, username)| GenesisDelegate { role, username })
            .collect(),
        donations: donations
            .iter()
            .map(|d| {
                let (role, basis_points) = (pick!(d, "role")?, pick!(d, "basisPoints")?);
                let purpose = pick!(d, "purpose")?;
                Ok(DonationShare {
                    role,
                    basis_points,
                    purpose,
                })
            })
            .collect::<Result<_, String>>()?,
        donations_from_height: pick!(profile, "donationsFromHeight")?,
        wallet_roles: numbered(&profile["wallets"], "rolePrefix")?,
    };
    Ok((input, amount(profile, "supply")?))
}

/// The files of `chain`, in the order they are written.
fn files(
    chain: &ChainDefinition,
    input: &GenesisInput,
    options: &GenerateOptions,
    forger_aux: &[u8; 32],
) -> Vec<File> {
    let public = |path: &'static str, text: String| (path, Zeroizing::new(text), false);
    let mut files = vec![
        public("crypto/network.json", chain.network_json.clone()),
        public("crypto/milestones.json", chain.milestones_json.clone()),
        public("crypto/genesisBlock.json", chain.genesis_block_json.clone()),
        public("crypto/exceptions.json", chain.exceptions_json.clone()),
        (
            "genesis-wallet.json",
            write_json(&wallets(chain, input)),
            true,
        ),
    ];
    let keys: Vec<Zeroizing<String>> = chain
        .delegates
        .iter()
        .map(|key| key.keys.secret_key().to_hex())
        .collect();
    let keys: Vec<&str> = keys.iter().map(|key| key.as_str()).collect();
    files.push(("delegates.json", write_json(&json!({ "keys": keys })), true));
    if options.chain_only {
        return files;
    }
    let (p2p, api) = PORTS;
    let env = format!(
        "CORE_LOG_LEVEL=info\nCORE_LOG_LEVEL_FILE=info\n\nCORE_P2P_HOST=127.0.0.1\n\
         CORE_P2P_PORT={p2p}\nCORE_P2P_MIN_NETWORK_REACH=0\n\nCORE_API_HOST=127.0.0.1\n\
         CORE_API_PORT={api}\n"
    );
    let plugin = |name: &str| json!({ "package": format!("@solar-network/{name}") });
    let p2p_plugin = json!({
        "package": "@solar-network/p2p",
        "options": { "remoteAccess": ["127.0.0.1", "::ffff:127.0.0.1", "::1"], "dns": [], "ntp": [] },
    });
    let forger_plugin = json!({
        "package": "@solar-network/forger",
        "options": {
            "hosts": [{ "hostname": "127.0.0.1", "port": p2p }],
            "aux": hex::encode(forger_aux),
        },
    });
    let relay = vec![
        plugin("logger"),
        plugin("state"),
        plugin("database"),
        plugin("transactions"),
        plugin("pool"),
        p2p_plugin,
        plugin("blockchain"),
        plugin("api"),
    ];
    let core: Vec<_> = relay
        .iter()
        .cloned()
        .chain([forger_plugin.clone()])
        .collect();
    // Each profile is usable; the command chooses which role runs.
    let app = json!({
        "core": { "plugins": core },
        "relay": { "plugins": relay },
        "forger": { "plugins": [plugin("logger"), plugin("transactions"), forger_plugin] },
    });
    let peers = json!({ "list": [{ "ip": "127.0.0.1", "port": p2p }] });
    let config = json!({ "token": DEFAULT_TOKEN, "network": options.network });
    files.extend([
        public(".env", env),
        ("app.json", write_json(&app), false),
        ("peers.json", write_json(&peers), false),
        ("config.json", write_json(&config), false),
    ]);
    files
}

/// `genesis-wallet.json`: every derived key, with the fields the genesis vectors give.
fn wallets(chain: &ChainDefinition, input: &GenesisInput) -> Value {
    let key = |k: &DerivedKey| {
        json!({
            "role": k.role,
            "address": k.address.to_base58(),
            "publicKey": k.keys.public_key().to_hex(),
            "passphrase": k.passphrase.as_str(),
        })
    };
    let genesis = chain.allocations.iter().zip(&input.allocations);
    let donations = chain.donations.iter().zip(&input.donations);
    let delegates = chain.delegates.iter().zip(&input.delegates);
    json!({
        "warning": WARNING,
        "generator": merge(key(&chain.generator), json!({ "issued": chain.summary.issued.to_string() })),
        "genesis": genesis
            .map(|(k, a)| merge(key(k), json!({ "genesisBalance": a.amount.to_string() })))
            .collect::<Vec<_>>(),
        "donations": donations
            .map(|(k, share)| {
                let extra = json!({
                    "genesisBalance": "0",
                    "basisPoints": share.basis_points,
                    "fromHeight": input.donations_from_height,
                });
                merge(key(k), extra)
            })
            .collect::<Vec<_>>(),
        "wallets": chain.wallets.iter().map(key).collect::<Vec<_>>(),
        "delegates": delegates
            .map(|(k, d)| {
                json!({
                    "username": d.username,
                    "role": k.role,
                    "address": k.address.to_base58(),
                    "publicKey": k.keys.public_key().to_hex(),
                    "privateKey": k.keys.secret_key().to_hex().as_str(),
                    "passphrase": k.passphrase.as_str(),
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// `value` with the keys of `extra` added after its own.
fn merge(mut value: Value, extra: Value) -> Value {
    if let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) {
        map.extend(extra);
    }
    value
}

/// `JSON.stringify(value, null, 4)` and a newline.
fn write_json(value: &Value) -> Zeroizing<String> {
    let mut text = js::stringify(value, 4);
    text.push('\n');
    Zeroizing::new(text)
}

/// Write `files` under `out`, refusing a directory that is not empty unless `force`.
fn write_all(out: &Path, files: &[File], force: bool) -> Result<(), NetworkError> {
    let failed = |path: &Path| {
        let path = path.to_path_buf();
        move |e: std::io::Error| NetworkError::Io {
            path,
            message: e.to_string(),
        }
    };
    if !force && fs::read_dir(out).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(NetworkError::Exists {
            path: out.to_path_buf(),
        });
    }
    let crypto = out.join("crypto");
    fs::create_dir_all(&crypto).map_err(failed(&crypto))?;
    for (name, text, secret) in files {
        let path = out.join(name);
        let mode = if *secret { 0o600 } else { 0o644 };
        let mut options = OpenOptions::new();
        let file = options
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .custom_flags(OFlag::O_NOFOLLOW.bits())
            .open(&path);
        let mut file = file.map_err(failed(&path))?;
        // The mode above applies only to a new file.
        let permissions = file.set_permissions(Permissions::from_mode(mode));
        permissions
            .and_then(|()| file.write_all(text.as_bytes()))
            .map_err(failed(&path))?;
    }
    Ok(())
}
