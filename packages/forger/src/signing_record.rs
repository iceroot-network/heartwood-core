//! The signing record: per key, the last block the forger signed, so that it never signs two
//! different blocks for one slot and a retry sends the same bytes again.
//!
//! - One JSON file, `<data>/forger/signing-record.json` (mode 0600, directory 0700), holding the
//!   network's nethash and, per key, the height, slot, id and bytes of its last signed block. A
//!   record of another network stops the forger's start.
//! - A write goes to a temporary file, is synced, renamed over the record and the directory is
//!   synced, all before the block leaves the forger; a torn temporary file is ignored at the next
//!   open.
//! - [`SigningRecord::decide`] checks the slot against the forger's own clock first: a slot more
//!   than one away is refused and not recorded. Then a slot below the key's recorded slot is
//!   refused, the recorded slot is re-sent as recorded, and a later slot is signed.
//! - A write slower than 250 ms is logged.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use heartwood_crypto::utils::hex;
use heartwood_crypto::{BlockId, PublicKeyBytes};
use serde_json::{Map, Value, json};

use crate::errors::RecordError;
use crate::seams::Signed;

/// The record's directory under the node's data directory.
pub const DIRECTORY: &str = "forger";
/// The record's file name.
pub const FILE: &str = "signing-record.json";
/// The format the file names itself with.
const FORMAT: &str = "heartwood-signing-record/1";
/// A write that takes longer is logged.
const SLOW_WRITE: Duration = Duration::from_millis(250);

/// The record's path under the data directory `data_dir`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(DIRECTORY).join(FILE)
}

/// What the record allows for a key and a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Sign a new block for the slot.
    SignNew,
    /// Send the block recorded for this slot again.
    Resend(Signed),
    /// Refuse: the key has signed a later slot.
    Refuse {
        /// The key's recorded slot.
        recorded_slot: i64,
    },
    /// Refuse: the slot is more than one away from the forger's own slot.
    OutOfWindow {
        /// The slot of the forger's clock.
        own_slot: i64,
    },
}

/// The signing record of one forger.
#[derive(Debug)]
pub struct SigningRecord {
    path: PathBuf,
    nethash: String,
    entries: BTreeMap<PublicKeyBytes, Signed>,
}

impl SigningRecord {
    /// Open the record at `path` for the network `nethash`, creating it when it does not exist.
    pub fn open(path: &Path, nethash: &str) -> Result<SigningRecord, RecordError> {
        let mut record = SigningRecord {
            path: path.to_owned(),
            nethash: nethash.to_owned(),
            entries: BTreeMap::new(),
        };
        let io = |error| io_error(path, error);
        let corrupt = |reason: &str| RecordError::Corrupt {
            path: path.to_owned(),
            reason: reason.to_owned(),
        };
        // A temporary file that was never renamed is a write that did not finish.
        match fs::remove_file(temporary(path)) {
            Err(error) if error.kind() != ErrorKind::NotFound => return Err(io(error)),
            _ => {}
        }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let directory = path.parent().unwrap_or(Path::new("."));
                fs::create_dir_all(directory).map_err(io)?;
                restrict(directory, 0o700).map_err(io)?;
                record.write().map_err(io)?;
                return Ok(record);
            }
            Err(error) => return Err(io(error)),
        };
        let value: Value = serde_json::from_str(&text).map_err(|_| corrupt("not JSON"))?;
        if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err(corrupt("not a signing record"));
        }
        let recorded = value
            .get("nethash")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if recorded != nethash {
            return Err(RecordError::Network {
                path: path.to_owned(),
                recorded: recorded.to_owned(),
                expected: nethash.to_owned(),
            });
        }
        let keys =
            (value.get("keys").and_then(Value::as_object)).ok_or_else(|| corrupt("no keys"))?;
        for (key, entry) in keys {
            let parsed = PublicKeyBytes::from_hex(key).ok().zip(entry_of(entry));
            let (key, signed) = parsed.ok_or_else(|| corrupt("an entry is not valid"))?;
            record.entries.insert(key, signed);
        }
        Ok(record)
    }

    /// What the record allows for `key` at `slot`, `own_slot` being the slot of the forger's
    /// clock.
    pub fn decide(&self, key: &PublicKeyBytes, slot: i64, own_slot: i64) -> Decision {
        if slot.abs_diff(own_slot) > 1 {
            return Decision::OutOfWindow { own_slot };
        }
        match self.entries.get(key) {
            Some(last) if slot < last.slot => Decision::Refuse {
                recorded_slot: last.slot,
            },
            Some(last) if slot == last.slot => Decision::Resend(last.clone()),
            _ => Decision::SignNew,
        }
    }

    /// Record `signed` as `key`'s last block, durably, before it is sent.
    pub fn record(&mut self, key: &PublicKeyBytes, signed: Signed) -> Result<(), RecordError> {
        let previous = self.entries.insert(*key, signed);
        let started = tokio::time::Instant::now();
        if let Err(error) = self.write() {
            match previous {
                Some(previous) => self.entries.insert(*key, previous),
                None => self.entries.remove(key),
            };
            return Err(io_error(&self.path, error));
        }
        let took = started.elapsed();
        if took > SLOW_WRITE {
            heartwood_kernel::warning!("Writing the signing record took {} ms", took.as_millis());
        }
        Ok(())
    }

    /// The block last recorded for `key`.
    pub fn last(&self, key: &PublicKeyBytes) -> Option<&Signed> {
        self.entries.get(key)
    }

    /// Write the whole record: a temporary file, synced, renamed over the record, then the
    /// directory synced.
    fn write(&self) -> std::io::Result<()> {
        let keys: Map<String, Value> = (self.entries.iter())
            .map(|(key, signed)| {
                let entry = json!({
                    "height": signed.height, "slot": signed.slot,
                    "id": signed.id.to_hex(), "bytes": hex::encode(&signed.bytes),
                });
                (key.to_hex(), entry)
            })
            .collect();
        let text = json!({"format": FORMAT, "nethash": self.nethash, "keys": keys}).to_string();
        let temporary = temporary(&self.path);
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &self.path)?;
        match self.path.parent() {
            Some(directory) if !directory.as_os_str().is_empty() => {
                File::open(directory)?.sync_all()
            }
            _ => Ok(()),
        }
    }
}

fn io_error(path: &Path, error: std::io::Error) -> RecordError {
    RecordError::Io {
        path: path.to_owned(),
        message: error.to_string(),
    }
}

/// The temporary file a write goes through.
fn temporary(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// An entry of the file.
fn entry_of(entry: &Value) -> Option<Signed> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    Some(Signed {
        height: u32::try_from(entry.get("height")?.as_u64()?).ok()?,
        slot: entry.get("slot")?.as_i64()?,
        id: BlockId::from_hex(text("id")?).ok()?,
        bytes: hex::decode(text("bytes")?).ok()?,
    })
}

#[cfg(unix)]
fn restrict(directory: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(directory, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict(_: &Path, _: u32) -> std::io::Result<()> {
    Ok(())
}
