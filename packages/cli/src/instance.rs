//! Instance resolution: which token, network and role a command is about, and its directories.
//!
//! - `--token` and `--network` name the instance; a missing one is filled from `config.json`,
//!   which never overrides a flag that was given. The token is `solar` when neither names it; the
//!   network, when neither names it, is the one directory under `<token>-core`'s config directory
//!   that holds a complete chain definition (its four `crypto/*.json` files).
//! - `config.json` is read, as the reference reads it, from `CORE_PATH_CONFIG` when that variable
//!   is set, else from the config directory of `<token>-core` and the `--network` given (the
//!   directory itself when none is given). A file that exists but is not a JSON object with text
//!   values is refused, where the reference would ignore it.
//! - The five directories come from the kernel's `paths` for `<token>-core/<network>`, each
//!   replaced by its `CORE_PATH_DATA`, `CORE_PATH_CONFIG`, `CORE_PATH_CACHE`, `CORE_PATH_LOG` or
//!   `CORE_PATH_TEMP` variable when set.
//! - The instance's name is `<token>-<network>-<role>`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use heartwood_kernel::paths::resolve_paths;
use serde_json::Value;

use crate::errors::InstanceError;
pub use crate::seams::{Instance, Role};

/// The token when neither `--token` nor `config.json` names one.
pub const DEFAULT_TOKEN: &str = "solar";

/// The four files of a complete chain definition, under `crypto/`.
pub const CRYPTO_FILES: [&str; 4] = [
    "network.json",
    "milestones.json",
    "genesisBlock.json",
    "exceptions.json",
];

/// The value of the key `$key` of the JSON value `$value`, read as the type the context asks
/// for (a missing key reads as `null`); the error text names the key.
macro_rules! pick {
    ($value:expr, $key:expr) => {
        serde_json::from_value($value.get($key).cloned().unwrap_or_default())
            .map_err(|e| format!("{}: {e}", $key))
    };
}
pub(crate) use pick;

/// The flags that name an instance, which every role command takes.
#[derive(Debug, Clone, Default, PartialEq, Eq, clap::Args)]
pub struct InstanceFlags {
    /// The token's name, the first part of the instance's directories and name.
    #[arg(long)]
    pub token: Option<String>,
    /// The network's name.
    #[arg(long)]
    pub network: Option<String>,
}

/// The places `config.json` is read from, in order, for `token` and `network` as the flags give
/// them, with `env` giving the environment, `home` the home directory and `cwd` the working one.
pub fn config_paths(
    token: &str,
    network: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    cwd: &Path,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if env("CORE_PATH_CONFIG").is_some_and(|value| !value.is_empty()) {
        dirs.push(resolve_paths(token, "", env, home, cwd).config);
    }
    let own = resolve_paths(
        token,
        network.unwrap_or(""),
        &without_config(env),
        home,
        cwd,
    );
    dirs.push(own.config);
    dirs.into_iter()
        .map(|dir| dir.join("config.json"))
        .collect()
}

/// The instance of `role` that `flags` name, with `env` giving the environment, `home` the
/// user's home directory and `cwd` the working directory.
pub fn resolve(
    role: Role,
    flags: &InstanceFlags,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    cwd: &Path,
) -> Result<Instance, InstanceError> {
    let first = flags.token.as_deref().unwrap_or(DEFAULT_TOKEN);
    let candidates = config_paths(first, flags.network.as_deref(), env, home, cwd);
    let (token, network) = read_config(&candidates)?;
    let token = flags.token.clone().or(token);
    let token = token.unwrap_or_else(|| DEFAULT_TOKEN.to_owned());
    let network = flags.network.clone().or(network);
    let network = network
        .or_else(|| discover_network(&token, env, home, cwd))
        .ok_or_else(|| InstanceError::MissingNetwork {
            config: candidates.last().cloned().unwrap_or_default(),
        })?;
    check_name("token", &token)?;
    check_name("network", &network)?;
    let paths = resolve_paths(&token, &network, env, home, cwd);
    Ok(Instance {
        token,
        network,
        role,
        paths,
    })
}

/// Refuse a token or network name (`what`) that is empty or holds anything but letters, digits,
/// `-` and `_`: a name becomes part of the instance's directories and file names. `relay run` and
/// `forger run` check theirs with it too.
pub fn check_name(what: &'static str, value: &str) -> Result<(), InstanceError> {
    let valid = (value.bytes()).all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if value.is_empty() || !valid {
        let value = value.to_owned();
        return Err(InstanceError::InvalidName { what, value });
    }
    Ok(())
}

/// The token and network of the first `config.json` of `candidates` that exists.
fn read_config(candidates: &[PathBuf]) -> Result<(Option<String>, Option<String>), InstanceError> {
    for path in candidates {
        let invalid = |reason: String| InstanceError::Config {
            path: path.clone(),
            reason,
        };
        let text = match fs::read_to_string(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            read => read.map_err(|e| invalid(e.to_string()))?,
        };
        let value: Value = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
        if !value.is_object() {
            return Err(invalid("not a JSON object".into()));
        }
        return Ok((
            pick!(value, "token").map_err(invalid)?,
            pick!(value, "network").map_err(invalid)?,
        ));
    }
    Ok((None, None))
}

/// The one network under `<token>-core`'s config directory with a complete chain definition.
fn discover_network(
    token: &str,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    cwd: &Path,
) -> Option<String> {
    let root = resolve_paths(token, "", &without_config(env), home, cwd).config;
    let mut found = fs::read_dir(root).ok()?.filter_map(|entry| {
        let entry = entry.ok()?;
        let crypto = entry.path().join("crypto");
        let complete = CRYPTO_FILES.iter().all(|file| crypto.join(file).is_file());
        complete.then(|| entry.file_name().into_string().ok())?
    });
    let network = found.next()?;
    found.next().is_none().then_some(network)
}

/// `env` without `CORE_PATH_CONFIG`, for the directories of `<token>-core` itself.
fn without_config(env: &dyn Fn(&str) -> Option<String>) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| env(name).filter(|_| name != "CORE_PATH_CONFIG")
}
