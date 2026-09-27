//! The manifest `devnet.json`, the crypto files' hashes, and the file helpers of the devnet: JSON
//! written with 4-space indentation and a final newline, as the devnet tooling writes it, and
//! the manifest written last by an atomic rename, so that its presence marks a complete devnet.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use serde_json::{Map, Value, json};

use crate::devnet::{CRYPTO_FILES, MANIFEST, MAX_NODES, Manifest, ManifestNode, NETWORK};
use crate::devnet::{Result, plan};
use crate::errors::DevnetError;
use crate::seams::ProcessOps;

/// The manifest's format.
const FORMAT: u64 = 1;

/// The error for `path` from an I/O error.
pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> DevnetError + '_ {
    |error| {
        let message = error.to_string();
        DevnetError::Io {
            path: path.to_path_buf(),
            message,
        }
    }
}

/// `value` as JSON with 4-space indentation and a final newline.
pub(crate) fn pretty(value: &Value) -> String {
    // serde_json indents by two spaces and escapes every line break inside a string, so the
    // leading spaces of each line are its indentation alone.
    let two = serde_json::to_string_pretty(value).unwrap_or_default();
    let mut out = String::with_capacity(two.len().saturating_mul(2));
    for line in two.lines() {
        let body = line.trim_start_matches(' ');
        let depth = line.len().saturating_sub(body.len());
        out.extend(std::iter::repeat_n(' ', depth.saturating_mul(2)));
        out.extend([body, "\n"]);
    }
    out
}

/// Write `bytes` to `path`.
pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(io(path))
}

/// Write `bytes` to `path` with mode 0600, so that no group or other user can read it.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    let options = options.write(true).create(true).truncate(true).mode(0o600);
    let mut file = options.open(path).map_err(io(path))?;
    let private = file.set_permissions(fs::Permissions::from_mode(0o600));
    private
        .and_then(|()| file.write_all(bytes))
        .map_err(io(path))
}

/// Remove `path`, a file or a directory tree, when it exists; a symbolic link is removed, not
/// followed.
pub(crate) fn remove(path: &Path) -> Result<()> {
    let removed = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    };
    removed.map_err(io(path))
}

/// The entries of `root` a devnet writes that exist: the manifest and its temporary file,
/// `genesis/` and every `node<k>/`.
fn entries(root: &Path) -> Vec<PathBuf> {
    let names = [
        MANIFEST.to_owned(),
        format!("{MANIFEST}.tmp"),
        "genesis".into(),
    ];
    let nodes = (1..=MAX_NODES).map(|k| format!("node{k}"));
    let paths = names.into_iter().chain(nodes).map(|name| root.join(name));
    paths
        .filter(|path| fs::symlink_metadata(path).is_ok())
        .collect()
}

/// Make room for a new devnet in `root`: nothing to do when it holds no devnet's files; with
/// `force`, stop the processes the old manifest names and remove the files; otherwise refuse.
pub(crate) fn clear(root: &Path, force: bool, ops: &dyn ProcessOps) -> Result<()> {
    let existing = entries(root);
    if existing.is_empty() {
        return Ok(());
    }
    if !force {
        let root = root.to_path_buf();
        return Err(DevnetError::Exists { root });
    }
    if let Ok(old) = Manifest::read(root) {
        let every: Vec<u32> = old.nodes.iter().map(|node| node.index).collect();
        crate::devnet::control::stop_nodes(&old, &every, None, false, ops)?;
    }
    existing.iter().try_for_each(|path| remove(path))
}

/// Remove the devnet's files from `root` after a failed `init`, as far as that works.
pub(crate) fn remove_all(root: &Path) {
    for path in entries(root) {
        let _ = remove(&path);
    }
}

/// The four crypto files of the `crypto` directory `dir`: name and bytes, in [`CRYPTO_FILES`]
/// order.
pub(crate) fn crypto_files(dir: &Path) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let read = |name: &'static str| {
        let path = dir.join(name);
        fs::read(&path)
            .map(|bytes| (name, bytes))
            .map_err(io(&path))
    };
    CRYPTO_FILES.into_iter().map(read).collect()
}

/// The SHA-256 of each file, in hex, by name.
pub(crate) fn hashes(files: &[(&str, Vec<u8>)]) -> Vec<(String, String)> {
    let hash = |(name, bytes): &(&str, Vec<u8>)| ((*name).to_owned(), hex::encode(&sha256(bytes)));
    files.iter().map(hash).collect()
}

impl Manifest {
    /// The manifest of the devnet in `root`.
    pub fn read(root: &Path) -> Result<Manifest> {
        let root = std::path::absolute(root).map_err(io(root))?;
        let path = root.join(MANIFEST);
        let text = match fs::read(&path) {
            Err(e) if e.kind() == ErrorKind::NotFound => Err(format!("it has no {MANIFEST}")),
            text => Ok(text.map_err(io(&path))?),
        };
        let value = text.and_then(|text| {
            let value = serde_json::from_slice::<Value>(&text);
            value.map_err(|error| error.to_string())
        });
        let manifest = value.and_then(|value| parse(&root, &value));
        manifest.map_err(|reason| DevnetError::Manifest { root, reason })
    }

    /// Write the manifest into `root`, atomically.
    pub(crate) fn write(&self, root: &Path) -> Result<()> {
        let [a, b, c] = self.address_prefix;
        let failover: Vec<String> = self.failover.iter().map(ToString::to_string).collect();
        let hashes = self.crypto_hashes.iter();
        let crypto: Map<String, Value> = hashes
            .map(|(n, h)| (n.clone(), h.as_str().into()))
            .collect();
        let node = |node: &ManifestNode| {
            let hosts: Vec<String> = node.forger_hosts.iter().map(ToString::to_string).collect();
            let forger = node.forger_address.map(|address| address.to_string());
            let (index, address) = (node.index, node.address.to_string());
            json!({ "node": index, "address": address, "forgerAddress": forger,
                    "forgerHosts": hosts, "delegates": node.delegates })
        };
        let value = json!({
            "format": FORMAT, "version": env!("CARGO_PKG_VERSION"), "network": NETWORK,
            "seed": self.seed, "addressPrefix": format!("{a}.{b}.{c}"),
            "validators": self.validators, "relays": self.relays, "failover": failover,
            "epochDelay": self.epoch_delay, "epoch": self.epoch, "genesisId": self.genesis_id,
            "nethash": self.nethash, "crypto": crypto,
            "nodes": self.nodes.iter().map(node).collect::<Vec<_>>(),
        });
        let (temp, path) = (root.join(format!("{MANIFEST}.tmp")), root.join(MANIFEST));
        write(&temp, pretty(&value).as_bytes())?;
        fs::rename(&temp, &path).map_err(io(&path))
    }
}

/// The manifest in `value`; the nodes are planned again from the layout it records.
fn parse(root: &Path, value: &Value) -> Result<Manifest, String> {
    let get = |key: &str| value.get(key).ok_or_else(|| format!("{key} is missing"));
    let invalid = |key: &str| format!("{key} is not valid");
    let text = |key: &str| {
        let text = get(key)?.as_str().map(str::to_owned);
        text.ok_or_else(|| invalid(key))
    };
    let count = |key: &str| {
        let number = get(key)?.as_u64().and_then(|n| u32::try_from(n).ok());
        number.ok_or_else(|| invalid(key))
    };
    let list = |key: &str| get(key)?.as_array().ok_or_else(|| invalid(key));
    if get("format")?.as_u64() != Some(FORMAT) {
        return Err(format!("its format is not {FORMAT}"));
    }
    let prefix = text("addressPrefix")?;
    let bytes: Option<Vec<u8>> = prefix.split('.').map(|part| part.parse().ok()).collect();
    let prefix = bytes.and_then(|bytes| <[u8; 3]>::try_from(bytes).ok());
    let address_prefix = prefix.ok_or_else(|| invalid("addressPrefix"))?;
    let failover = list("failover")?
        .iter()
        .map(|entry| entry.as_str()?.parse().ok());
    let failover: Vec<_> = failover
        .collect::<Option<_>>()
        .ok_or_else(|| invalid("failover"))?;
    let crypto = get("crypto")?;
    let hash = |name: &&str| Some(((*name).to_owned(), crypto.get(name)?.as_str()?.to_owned()));
    let crypto_hashes = CRYPTO_FILES.iter().map(hash).collect::<Option<Vec<_>>>();
    let crypto_hashes = crypto_hashes.ok_or_else(|| invalid("crypto"))?;
    let (validators, relays) = (count("validators")?, count("relays")?);
    let planned = plan(root, address_prefix, validators, relays, &failover);
    let mut nodes = planned.map_err(|error| error.to_string())?;
    let listed = list("nodes")?;
    if listed.len() != nodes.len() {
        return Err(invalid("nodes"));
    }
    for (node, item) in nodes.iter_mut().zip(listed) {
        let names = item.get("delegates").and_then(Value::as_array);
        let names = names
            .into_iter()
            .flatten()
            .map(|name| name.as_str().map(str::to_owned));
        node.delegates = names
            .collect::<Option<_>>()
            .ok_or_else(|| invalid("delegates"))?;
    }
    Ok(Manifest {
        seed: text("seed")?,
        address_prefix,
        validators,
        relays,
        failover,
        epoch_delay: count("epochDelay")?,
        nethash: text("nethash")?,
        genesis_id: text("genesisId")?,
        epoch: text("epoch")?,
        crypto_hashes,
        nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_indented_by_four_spaces() {
        let value =
            json!({ "list": [{ "ip": "127.0.0.11", "port": 6002 }], "none": [], "text": "a\nb" });
        assert_eq!(
            pretty(&value),
            "{\n    \"list\": [\n        {\n            \"ip\": \"127.0.0.11\",\n            \"port\": 6002\n        }\n    ],\n    \"none\": [],\n    \"text\": \"a\\nb\"\n}\n"
        );
    }
}
