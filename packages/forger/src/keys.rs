//! The forger's key file, `delegates.json` in its configuration directory.
//!
//! - The file must exist (the reference's "The ... file does not exist") and hold at least one key
//!   ("Unable to detect any private keys"); the forger refuses a file on which its group or others
//!   have any permission, as SSH refuses a private key file.
//! - `{"keys": [64 hex digits, ...]}`: each a secp256k1 secret key. An empty `secrets` array is
//!   accepted; a non-empty one is refused, since the forger does not read passphrases. Any other
//!   key of the object is refused, as the reference's schema refuses it. The file is never
//!   rewritten.
//! - Forging uses one delegate per public key, in file order. Attestation declares every key in
//!   file order, duplicates included, except in a file with a `secrets` key (even an empty one),
//!   whose keys the reference rewrites through a set of their texts, so each text is declared once.
//! - The keys are wiped from memory when dropped and never logged.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use heartwood_crypto::{KeyPair, PublicKeyBytes, SecretKey};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::delegate::Delegate;
use crate::errors::KeysError;

/// The key file's name in the configuration directory.
pub const FILE: &str = "delegates.json";

/// The key file's path in the configuration directory `config_dir`.
pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// The keys of a key file, in file order.
#[derive(Debug, Clone)]
pub struct DelegateKeys {
    keys: Vec<Arc<KeyPair>>,
    /// The public keys the attester declares, in file order.
    declared: Vec<PublicKeyBytes>,
}

impl DelegateKeys {
    /// Keys given directly, in order, their duplicates declared as often as they appear.
    pub fn from_keys(keys: Vec<KeyPair>) -> DelegateKeys {
        let keys: Vec<_> = keys.into_iter().map(Arc::new).collect();
        let declared = (keys.iter())
            .map(|k| k.public_key().to_compressed())
            .collect();
        DelegateKeys { keys, declared }
    }

    /// The delegates to forge for: one per public key, in file order, their addresses on the
    /// network with address byte `network`.
    pub fn delegates(&self, network: u8) -> Vec<Delegate> {
        let mut delegates: Vec<Delegate> = Vec::new();
        for key in &self.keys {
            let public_key = key.public_key().to_compressed();
            if !(delegates.iter()).any(|d| *d.public_key() == public_key) {
                delegates.push(Delegate::new(key.clone(), network));
            }
        }
        delegates
    }

    /// The keys the attester declares to its relay, in file order.
    pub fn declared(&self) -> Vec<PublicKeyBytes> {
        self.declared.clone()
    }

    /// The key pair of `public_key`, if the file holds it.
    pub fn key(&self, public_key: &PublicKeyBytes) -> Option<&KeyPair> {
        (self.keys.iter())
            .find(|key| key.public_key().to_compressed() == *public_key)
            .map(|key| &**key)
    }
}

/// Read and check `delegates.json` in `config_dir`.
pub fn load_keys(config_dir: &Path) -> Result<DelegateKeys, KeysError> {
    let path = path(config_dir);
    let io = |error: std::io::Error| match error.kind() {
        ErrorKind::NotFound => KeysError::Missing { path: path.clone() },
        _ => KeysError::Io {
            path: path.clone(),
            message: error.to_string(),
        },
    };
    let metadata = std::fs::metadata(&path).map_err(io)?;
    #[cfg(unix)]
    let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o777;
    // Elsewhere the file has no group or others.
    #[cfg(not(unix))]
    let mode = 0o600;
    if mode & 0o077 != 0 {
        return Err(KeysError::TooOpen { path, mode });
    }
    let text = Zeroizing::new(std::fs::read_to_string(&path).map_err(io)?);
    let invalid = |reason: &str| KeysError::Invalid {
        path: path.clone(),
        reason: reason.to_owned(),
    };
    let mut value: Value = serde_json::from_str(&text).map_err(|_| invalid("it is not JSON"))?;
    let result = read(&value, &path, &invalid);
    wipe(&mut value);
    result
}

/// The keys of the file's JSON `value`, whose key texts the caller wipes. The checks run in the
/// reference's order: no key at all first, then the schema.
fn read(
    value: &Value,
    path: &Path,
    invalid: &dyn Fn(&str) -> KeysError,
) -> Result<DelegateKeys, KeysError> {
    let Some(object) = value.as_object() else {
        return Err(KeysError::NoKeys);
    };
    let list = |key: &str| match object.get(key) {
        None => Ok(&[][..]),
        Some(Value::Array(items)) => Ok(items.as_slice()),
        Some(_) => Err(invalid(&format!("`{key}` is not a list"))),
    };
    let (keys, secrets) = (list("keys")?, list("secrets")?);
    if keys.is_empty() && secrets.is_empty() {
        return Err(KeysError::NoKeys);
    }
    if object.keys().any(|key| key != "keys" && key != "secrets") {
        return Err(invalid("it may hold only `keys` and `secrets`"));
    }
    if !secrets.is_empty() {
        return Err(KeysError::Secrets {
            path: path.to_owned(),
        });
    }
    let mut texts: Vec<&str> = Vec::new();
    let mut pairs = Vec::new();
    let mut declared = Vec::new();
    for (index, item) in keys.iter().enumerate() {
        let text = (item.as_str())
            .filter(|text| text.len() == 64)
            .ok_or_else(|| invalid(&format!("key {index} is not 64 hex digits")))?;
        let secret = SecretKey::from_hex(text)
            .map_err(|_| invalid(&format!("key {index} is not a valid secret key")))?;
        let pair = KeyPair::from_secret_key(secret);
        // A file with `secrets` is rewritten by the reference through a set of the key texts.
        if !object.contains_key("secrets") || !texts.contains(&text) {
            declared.push(pair.public_key().to_compressed());
        }
        if (pairs.iter()).any(|p: &Arc<KeyPair>| p.public_key() == pair.public_key()) {
            let key = pair.public_key().to_hex();
            heartwood_kernel::warning!("{FILE} lists the key of delegate {key} twice");
        }
        texts.push(text);
        pairs.push(Arc::new(pair));
    }
    let keys = pairs;
    Ok(DelegateKeys { keys, declared })
}

/// Overwrite every string of `value`.
fn wipe(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe),
        Value::Object(object) => object.values_mut().for_each(wipe),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}
