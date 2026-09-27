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

use std::path::{Path, PathBuf};

use heartwood_crypto::PublicKeyBytes;

use crate::errors::RecordError;
use crate::seams::Signed;

/// The record's directory under the node's data directory.
pub const DIRECTORY: &str = "forger";
/// The record's file name.
pub const FILE: &str = "signing-record.json";

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
    #[expect(dead_code, reason = "the record's methods read it")]
    path: PathBuf,
    #[expect(dead_code, reason = "the record's methods read it")]
    nethash: String,
}

impl SigningRecord {
    /// Open the record at `path` for the network `nethash`, creating it when it does not exist.
    pub fn open(path: &Path, nethash: &str) -> Result<SigningRecord, RecordError> {
        let _ = (path, nethash);
        todo!("the signing record's open")
    }

    /// What the record allows for `key` at `slot`, `own_slot` being the slot of the forger's
    /// clock.
    pub fn decide(&self, key: &PublicKeyBytes, slot: i64, own_slot: i64) -> Decision {
        let _ = (key, slot, own_slot);
        todo!("the signing record's decision")
    }

    /// Record `signed` as `key`'s last block, durably, before it is sent.
    pub fn record(&mut self, key: &PublicKeyBytes, signed: Signed) -> Result<(), RecordError> {
        let _ = (key, signed);
        todo!("the signing record's write")
    }

    /// The block last recorded for `key`.
    pub fn last(&self, key: &PublicKeyBytes) -> Option<&Signed> {
        let _ = key;
        todo!("the signing record's entry")
    }
}
