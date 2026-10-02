//! `config publish` and `config forger`: an instance's configuration files from the built-in
//! template (for any network name; an existing directory refused without `--force`), and the
//! forger's key written into the existing `delegates.json` (its `keys` replaced by the one key,
//! every other key kept, four-space indentation and a final newline, 0600 through a temporary
//! file, fsync and rename; an absent file refused, as the reference's `require` fails).
//!
//! The template holds no crypto files: they come from `network generate --chain-only` or a copy.
//! `--force` writes the template's files over the directory's and leaves every other file.

use std::fmt;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use heartwood_crypto::utils::js;
use nix::fcntl::OFlag;
use serde_json::{Value, json};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::instance::InstanceFlags;
use crate::seams::Instance;

/// The template's files and their modes: the key file is private.
const TEMPLATE: [(&str, &str, u32); 4] = [
    (".env", include_str!("templates/env"), 0o644),
    ("app.json", include_str!("templates/app.json"), 0o644),
    ("peers.json", include_str!("templates/peers.json"), 0o644),
    (
        "delegates.json",
        include_str!("templates/delegates.json"),
        0o600,
    ),
];

/// The most a key file may hold: the file is read whole.
const KEY_FILE_LIMIT: u64 = 1 << 20;

/// What `config publish` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// The configuration directory.
    pub dir: PathBuf,
    /// The files written, in the order written.
    pub files: Vec<PathBuf>,
}

impl fmt::Display for Published {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dir = self.dir.display();
        writeln!(f, "Published the configuration to {dir}")?;
        writeln!(
            f,
            "Its crypto files are not part of the template: write them with"
        )?;
        writeln!(
            f,
            "`network generate --chain-only`, or copy them, into {dir}/crypto"
        )
    }
}

/// A configuration command failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// The configuration directory holds files and `--force` was not given.
    #[error("Please use the --force flag if you wish to overwrite the existing configuration")]
    Exists {
        /// The directory.
        dir: PathBuf,
    },
    /// The key is missing or is not 64 hexadecimal characters.
    #[error("the private key (--privateKey) must be 64 hexadecimal characters")]
    Key,
    /// There is no `delegates.json` to put the key into.
    #[error("{} does not exist: run config publish first, or put your delegates.json there", .0.display())]
    Absent(PathBuf),
    /// A file could not be read or written, or a link is in its place.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// `--method` names a method that is not available.
    #[error("--method {0} is not available: privateKey is the only method (bip39 is deferred)")]
    Method(String),
}

/// `error` as a failure of the file at `path`.
pub(crate) fn io(path: &Path, error: impl fmt::Display) -> ConfigError {
    let (path, message) = (path.to_path_buf(), error.to_string());
    ConfigError::Io { path, message }
}

/// The flags of `first`, the other's where it gives none: `config forger private-key` takes the
/// instance's flags before or after its own name.
pub fn merged(first: InstanceFlags, other: InstanceFlags) -> InstanceFlags {
    let (token, network) = (first.token.or(other.token), first.network.or(other.network));
    InstanceFlags { token, network }
}

/// Write `instance`'s configuration files from the template; with `force`, over an existing
/// directory's.
pub fn publish(instance: &Instance, force: bool) -> Result<Published, ConfigError> {
    let dir = &instance.paths.config;
    if !force && fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(ConfigError::Exists { dir: dir.clone() });
    }
    fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let config = json!({ "token": instance.token, "network": instance.network });
    let config = format!("{}\n", js::stringify(&config, 4));
    let files = TEMPLATE
        .into_iter()
        .chain([("config.json", config.as_str(), 0o644)]);
    let mut written = Vec::new();
    for (name, text, mode) in files {
        write_atomic(&dir.join(name), text.as_bytes(), mode)?;
        written.push(dir.join(name));
    }
    Ok(Published {
        dir: dir.clone(),
        files: written,
    })
}

/// `config forger`: the method `method` (`privateKey`, or none), with `key`; gives the key file.
pub fn forger(
    instance: &Instance,
    method: Option<&str>,
    key: Option<&str>,
) -> Result<PathBuf, ConfigError> {
    if let Some(other) = method.filter(|method| *method != "privateKey") {
        return Err(ConfigError::Method(other.to_owned()));
    }
    forger_private_key(instance, key.unwrap_or_default())?;
    Ok(instance.paths.config.join("delegates.json"))
}

/// Write the forger's private key `key` (64 hex characters, lower-cased) into `instance`'s
/// `delegates.json`.
pub fn forger_private_key(instance: &Instance, key: &str) -> Result<(), ConfigError> {
    let path = instance.paths.config.join("delegates.json");
    // The key is checked before the file is read, as the reference validates its flags first.
    with_key("{}", key)?;
    let text = match read_file(&path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(ConfigError::Absent(path)),
        read => read.map_err(|e| io(&path, e))?,
    };
    write_atomic(&path, with_key(&text, key)?.as_bytes(), 0o600)
}

/// The text of a key file `existing` with `keys` replaced by `[key]`, `key` lower-cased, every
/// other key kept in its place, as `writeJsonSync(file, delegates, {spaces: 4})` writes it.
pub fn with_key(existing: &str, key: &str) -> Result<Zeroizing<String>, ConfigError> {
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfigError::Key);
    }
    let invalid = |e: String| io(Path::new("delegates.json"), e);
    let existing = existing.strip_prefix('\u{feff}').unwrap_or(existing);
    let mut delegates: Value =
        serde_json::from_str(existing).map_err(|e| invalid(e.to_string()))?;
    let Some(object) = delegates.as_object_mut() else {
        return Err(invalid("not a JSON object".into()));
    };
    object.insert("keys".into(), json!([key.to_ascii_lowercase()]));
    Ok(Zeroizing::new(js::stringify(&delegates, 4) + "\n"))
}

/// The text of the file at `path`, opened without following a link.
fn read_file(path: &Path) -> std::io::Result<Zeroizing<String>> {
    if fs::symlink_metadata(path)?.is_symlink() {
        return Err(std::io::Error::other("is a symbolic link"));
    }
    // A FIFO in the file's place reads as empty instead of waiting for a writer.
    let flags = (OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)?;
    let mut text = Zeroizing::new(String::new());
    file.take(KEY_FILE_LIMIT + 1).read_to_string(&mut text)?;
    match u64::try_from(text.len()) {
        Ok(len) if len <= KEY_FILE_LIMIT => Ok(text),
        _ => Err(std::io::Error::other("too large")),
    }
}

/// Write `bytes` to `path` with `mode`, never wider, through a new file beside it that is synced
/// and renamed into place; a link at `path` is refused.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), ConfigError> {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_symlink()) {
        return Err(io(path, "is a symbolic link"));
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temp)?;
        file.set_permissions(Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&temp);
        io(path, e)
    })?;
    // The rename is done; making it durable is best effort.
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    let _ = File::open(dir.unwrap_or(Path::new("."))).and_then(|dir| dir.sync_all());
    Ok(())
}
