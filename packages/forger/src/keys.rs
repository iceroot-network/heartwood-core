//! The forger's key file, `delegates.json` in its configuration directory.
//!
//! - The file must exist (the reference's "The ... file does not exist") and hold at least one key
//!   ("Unable to detect any private keys"); the forger refuses a file its group or others can read.
//! - `{"keys": [64 hex digits, ...]}`: each a secp256k1 secret key. An empty `secrets` array is
//!   accepted; a non-empty one is refused, since the forger does not read passphrases. Any other
//!   key of the object is refused, as the reference's schema refuses it. The file is never
//!   rewritten.
//! - Forging uses one delegate per public key, in file order. Attestation declares every key in
//!   file order, duplicates included, except in a file with a `secrets` key (even an empty one),
//!   whose keys the reference rewrites through a set, so they are declared once each.
//! - The keys are wiped from memory when dropped and never logged.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use heartwood_crypto::{KeyPair, PublicKeyBytes};

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
    /// Whether the declared keys are deduplicated (a file with a `secrets` key).
    once_each: bool,
}

impl DelegateKeys {
    /// Keys given directly, in order, their duplicates declared as often as they appear.
    pub fn from_keys(keys: Vec<KeyPair>) -> DelegateKeys {
        DelegateKeys {
            keys: keys.into_iter().map(Arc::new).collect(),
            once_each: false,
        }
    }

    /// The delegates to forge for: one per public key, in file order, their addresses on the
    /// network with address byte `network`.
    pub fn delegates(&self, network: u8) -> Vec<Delegate> {
        let _ = (network, &self.keys);
        todo!("the delegates of the key file")
    }

    /// The keys the attester declares to its relay, in file order.
    pub fn declared(&self) -> Vec<PublicKeyBytes> {
        let _ = self.once_each;
        todo!("the declared keys")
    }

    /// The key pair of `public_key`, if the file holds it.
    pub fn key(&self, public_key: &PublicKeyBytes) -> Option<&KeyPair> {
        let _ = public_key;
        todo!("a key by its public key")
    }
}

/// Read and check `delegates.json` in `config_dir`.
pub fn load_keys(config_dir: &Path) -> Result<DelegateKeys, KeysError> {
    let _ = config_dir;
    todo!("the key file")
}
