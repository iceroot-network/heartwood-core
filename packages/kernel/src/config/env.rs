//! The node's environment, as the reference's local config loader sets it: the process's own
//! variables, then the keys of `<config>/.env` that the process does not have, then every key of
//! `~/.<token>/.env` when that file exists, over both. The reference refuses to start without the
//! home file; a Heartwood relay does not need it.
//!
//! A switch (`CORE_API_DISABLED`, `CORE_API_RATE_LIMIT_DISABLED`, ...) is on when its variable is
//! set to any non-empty value ([`Env::is_set`]), as the reference's `!!process.env.X` reads it.
//!
//! A file's values become text as the reference's process environment stores them: a number as
//! JavaScript prints it (`0042` is `42`), `true` and `false` as themselves. A key whose dotted
//! parts name `__proto__`, `prototype` or `constructor` is skipped, and any other key with a dot
//! refuses the file, as the reference's nested assignment into the environment fails on it.

use std::path::{Path, PathBuf};

use heartwood_crypto::utils::js;
use indexmap::IndexMap;
use thiserror::Error;

use crate::config::dot_env::{self, EnvValue};

/// The node's environment: every variable, after the `.env` files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    vars: IndexMap<String, String>,
}

/// An `.env` file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("cannot read {}: {detail}", path.display())]
pub struct EnvError {
    /// The file.
    pub path: PathBuf,
    /// Why.
    pub detail: String,
}

impl Env {
    /// An environment of exactly `vars`, for tests and for a node without `.env` files.
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Env {
        Env {
            vars: vars.into_iter().collect(),
        }
    }

    /// The value of `name`, if it is set.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// Whether `name` is set to a non-empty value.
    pub fn is_set(&self, name: &str) -> bool {
        self.get(name).is_some_and(|value| !value.is_empty())
    }

    /// The environment as a lookup function, as the components' defaults read it.
    pub fn lookup(&self) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| self.get(name).map(str::to_owned)
    }
}

/// The environment of a node whose configuration directory is `config_dir`, for the user whose
/// home directory is `home`, with the token `token`, over the process's variables `process`
/// (see the module documentation for the precedence).
pub fn load(
    config_dir: &Path,
    home: &Path,
    token: &str,
    process: impl IntoIterator<Item = (String, String)>,
) -> Result<Env, EnvError> {
    let mut vars: IndexMap<String, String> = process.into_iter().collect();
    let global = home.join(format!(".{token}")).join(".env");
    let global = global.exists().then_some((global, true));
    for (path, overrides) in [(config_dir.join(".env"), false)].into_iter().chain(global) {
        let fail = |detail: String| EnvError {
            path: path.clone(),
            detail,
        };
        let text = std::fs::read(&path).map_err(|error| fail(error.to_string()))?;
        for (key, value) in dot_env::parse(&String::from_utf8_lossy(&text)) {
            let parts: Vec<&str> = key.split('.').collect();
            let prototype = parts
                .iter()
                .any(|part| ["__proto__", "prototype", "constructor"].contains(part));
            if prototype || (!overrides && vars.contains_key(&key)) {
                continue;
            } else if parts.len() > 1 {
                return Err(fail(format!(
                    "the key {key} cannot be set in the environment"
                )));
            }
            let text = match value {
                EnvValue::Number(number) => js::number_to_string(number),
                EnvValue::Bool(flag) => flag.to_string(),
                EnvValue::Text(text) => text,
            };
            vars.insert(key, text);
        }
    }
    Ok(Env { vars })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A fresh directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("heartwood-env-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn the_process_wins_over_the_config_file_and_the_home_file_over_both() {
        let dir = scratch("precedence");
        let (config, home) = (dir.join("config"), dir.join("home"));
        fs::create_dir_all(home.join(".solar")).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join(".env"),
            "CORE_P2P_PORT=0042\nCORE_API_PORT=4003\nCORE_LOG_LEVEL=debug\nFLAG=true\nA=x\n",
        )
        .unwrap();
        fs::write(
            home.join(".solar/.env"),
            "CORE_LOG_LEVEL=info\nCORE_API_PORT=5003\n",
        )
        .unwrap();
        let process = vars(&[("CORE_API_PORT", "7003"), ("CORE_P2P_PORT", "9"), ("A", "")]);
        let env = load(&config, &home, "solar", process).unwrap();
        // The process keeps its own, even an empty one; the home file overrides the process.
        assert_eq!(env.get("CORE_P2P_PORT"), Some("9"));
        assert_eq!(env.get("A"), Some(""));
        assert!(!env.is_set("A"));
        assert_eq!(env.get("CORE_API_PORT"), Some("5003"));
        assert_eq!(env.get("CORE_LOG_LEVEL"), Some("info"));
        assert_eq!(env.get("FLAG"), Some("true"));
        assert!(env.is_set("FLAG"));
        // Without the home file (another token), the config file fills what the process lacks,
        // with a number as JavaScript prints it.
        let env = load(&config, &home, "other", vars(&[])).unwrap();
        assert_eq!(env.get("CORE_P2P_PORT"), Some("42"));
        assert_eq!(env.get("CORE_LOG_LEVEL"), Some("debug"));
        assert_eq!(env.lookup()("CORE_API_PORT"), Some("4003".to_owned()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_config_file_is_required_and_the_home_file_is_not() {
        let dir = scratch("required");
        let error = load(&dir, &dir, "solar", vars(&[])).unwrap_err();
        assert_eq!(error.path, dir.join(".env"));
        fs::write(dir.join(".env"), "").unwrap();
        assert_eq!(load(&dir, &dir, "solar", vars(&[])), Ok(Env::default()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dotted_keys_are_skipped_or_refused_as_the_reference_assigns_them() {
        let dir = scratch("dotted");
        fs::write(dir.join(".env"), "a.constructor=1\nprototype.b=2\nOK=1\n").unwrap();
        let env = load(&dir, &dir, "solar", vars(&[])).unwrap();
        assert_eq!(env, Env::from_vars(vars(&[("OK", "1")])));
        // A dotted key the process already has is never assigned, so it is no error.
        fs::write(dir.join(".env"), "a.b=1\n").unwrap();
        assert!(load(&dir, &dir, "solar", vars(&[("a.b", "x")])).is_ok());
        let error = load(&dir, &dir, "solar", vars(&[])).unwrap_err();
        assert!(error.to_string().contains("a.b"), "{error}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
