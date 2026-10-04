//! The instance index: one file per started instance, `<data-root>/instances/<name>.json`, where
//! `<data-root>` is the token's data root (the parent of every network's data directory) and
//! `<name>` is [`Instance::name`]. The supervisor writes an instance's entry at its start and
//! removes it at its stop, through the writer that refuses a link in its place; a node a service
//! manager starts writes its own, which stays after it stops; `top` lists the token's entries. An
//! entry carries no pid: the supervisor's pid and its liveness come from its held lock (a managed
//! node's from its own lock and record), so an entry whose process is gone is ignored, never
//! trusted.
//!
//! An entry is a JSON object: `name`, `token`, `network`, `role` and `paths` (the instance's
//! `config`, `data`, `cache`, `log` and `temp` directories, absolute), so that `top` can read the
//! instance's lock and state as `status` does. It is written through a temporary file and a
//! rename in the index directory held without following a link ([`RunDir`]), and read the same
//! way: a link in an entry's place is refused on a read and replaced, never written through, on a
//! write.

use std::fs::{self, DirBuilder};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use heartwood_kernel::paths::{Paths, resolve_paths};
use nix::fcntl::OFlag;
use serde_json::{Value, json};
use thiserror::Error;

use crate::errors::ProcessError;
use crate::instance::pick;
use crate::seams::{Instance, Role};
use crate::supervisor::RunDir;

/// The directory of the index under the token's data root.
pub const INDEX_DIR: &str = "instances";

/// The index could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IndexError {
    /// An entry or the index's directory could not be read or written, or a link is in its place.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// An entry is not valid.
    #[error("{} is not a valid index entry: {reason}", path.display())]
    Entry {
        /// The entry.
        path: PathBuf,
        /// Why.
        reason: String,
    },
}

impl From<IndexError> for ProcessError {
    fn from(error: IndexError) -> ProcessError {
        match error {
            IndexError::Io { path, message } => ProcessError::Io { path, message },
            IndexError::Entry { path, reason } => ProcessError::State { path, reason },
        }
    }
}

impl From<ProcessError> for IndexError {
    fn from(error: ProcessError) -> IndexError {
        match error {
            ProcessError::Io { path, message } => IndexError::Io { path, message },
            other => io(Path::new(""), other),
        }
    }
}

fn io(path: &Path, error: impl ToString) -> IndexError {
    let message = error.to_string();
    IndexError::Io {
        path: path.to_path_buf(),
        message,
    }
}

/// The index's directory under `data_root`.
pub fn index_dir(data_root: &Path) -> PathBuf {
    data_root.join(INDEX_DIR)
}

/// The entry of `instance` under `data_root`.
pub fn entry_path(data_root: &Path, instance: &Instance) -> PathBuf {
    index_dir(data_root).join(format!("{}.json", instance.name()))
}

/// The data root of `token`: the directory that holds the data directory of each of its
/// networks, with `env` giving the environment and `home` the home directory. It is where the
/// data directories are by default, so `CORE_PATH_DATA`, which moves one network's data, does not
/// move it: every network's instances stay in one place for `top`.
pub fn data_root(token: &str, env: &dyn Fn(&str) -> Option<String>, home: &Path) -> PathBuf {
    let env = |name: &str| env(name).filter(|_| name != "CORE_PATH_DATA");
    resolve_paths(token, "", &env, home, Path::new("/")).data
}

/// [`data_root`] for this process's environment and home directory.
pub fn process_data_root(token: &str) -> PathBuf {
    let home = std::env::home_dir().unwrap_or_default();
    data_root(token, &|name| std::env::var(name).ok(), &home)
}

fn text(path: &Path) -> Result<&str, IndexError> {
    path.to_str().ok_or_else(|| io(path, "not UTF-8"))
}

/// Write the entry of `instance` under `data_root`, at the supervisor's start.
pub fn write(data_root: &Path, instance: &Instance) -> Result<(), IndexError> {
    let dir = index_dir(data_root);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| io(&dir, e))?;
    let path = entry_path(data_root, instance);
    let held = RunDir::beside(&path)?;
    let paths = &instance.paths;
    let value = json!({
        "name": instance.name(),
        "token": instance.token,
        "network": instance.network,
        "role": instance.role.name(),
        "paths": {
            "config": text(&paths.config)?,
            "data": text(&paths.data)?,
            "cache": text(&paths.cache)?,
            "log": text(&paths.log)?,
            "temp": text(&paths.temp)?,
        },
    });
    let temp = path.with_extension("json.tmp");
    let flags = OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC;
    let file = held.file(&temp, flags, 0o600).map_err(|e| io(&temp, e))?;
    (&file)
        .write_all(format!("{value:#}\n").as_bytes())
        .map_err(|e| io(&temp, e))?;
    held.rename(&temp, &path).map_err(|e| io(&path, e))
}

/// Remove the entry of `instance` under `data_root`, at the supervisor's stop; none is no error.
pub fn remove(data_root: &Path, instance: &Instance) -> Result<(), IndexError> {
    let path = entry_path(data_root, instance);
    let Some(held) = RunDir::beside_if_there(&path)? else {
        return Ok(());
    };
    match held.remove(&path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        removed => removed.map_err(|e| io(&path, e)),
    }
}

/// `token`'s instances under `data_root`, across its networks, in name order. An entry that is a
/// link, is not a regular file or is not valid is an error, naming it.
pub fn list(data_root: &Path, token: &str) -> Result<Vec<Instance>, IndexError> {
    let dir = index_dir(data_root);
    let Some(held) = RunDir::beside_if_there(&dir.join("entry"))? else {
        return Ok(Vec::new());
    };
    let names = match fs::read_dir(&dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        names => names.map_err(|e| io(&dir, e))?,
    };
    let prefix = format!("{token}-");
    let mut names: Vec<String> = names
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix) && name.ends_with(".json"))
        .collect();
    names.sort_unstable();
    let mut found = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let text = held.read(&path).map_err(|e| io(&path, e))?;
        let instance = parse(&text, &path)?;
        if instance.token == token {
            found.push(instance);
        }
    }
    Ok(found)
}

/// The instance the entry `text`, read from `path`, names.
fn parse(text: &str, path: &Path) -> Result<Instance, IndexError> {
    let invalid = |reason: String| IndexError::Entry {
        path: path.to_path_buf(),
        reason,
    };
    let value: Value = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
    let role: String = pick!(value, "role").map_err(invalid)?;
    let (token, network) = (pick!(value, "token"), pick!(value, "network"));
    let paths: Value = value.get("paths").cloned().unwrap_or_default();
    let dir = |key: &str| -> Result<PathBuf, IndexError> {
        let dir: PathBuf = pick!(paths, key).map_err(|e| invalid(format!("paths: {e}")))?;
        let absolute = dir.is_absolute();
        absolute
            .then_some(dir)
            .ok_or_else(|| invalid(format!("paths: {key} is not absolute")))
    };
    let instance = Instance {
        token: token.map_err(invalid)?,
        network: network.map_err(invalid)?,
        role: role.parse::<Role>().map_err(invalid)?,
        paths: Paths {
            config: dir("config")?,
            data: dir("data")?,
            cache: dir("cache")?,
            log: dir("log")?,
            temp: dir("temp")?,
        },
    };
    let name: String = pick!(value, "name").map_err(invalid)?;
    let file = path.file_name().and_then(|file| file.to_str());
    if name != instance.name() || file != Some(&format!("{name}.json")) {
        return Err(invalid("it names another instance".into()));
    }
    Ok(instance)
}
