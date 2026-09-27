//! The crate's errors.
//!
//! [`RelayError`] carries the reference's two forger errors with their texts, which the loop logs
//! as the reference does. The others refuse a configuration, a key file, a block or a signature.

use std::path::PathBuf;

use heartwood_crypto::errors::{BlockError, SigError};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::errors::PreimageError;
use thiserror::Error;

use crate::options::RelayHost;

/// A call to a relay failed: the reference's `HostNoResponseError` and `RelayCommunicationError`,
/// with their texts.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RelayError {
    /// No host had a ready session after the selection's sweeps.
    #[error("{hosts} didn't respond. Trying again later :sparkler:")]
    HostNoResponse {
        /// The hosts' addresses, joined with commas.
        hosts: String,
    },
    /// A call failed: the session's error, the codec's refusal or the call's deadline.
    #[error("Request to {endpoint} failed, because of '{message}' :bangbang:")]
    Communication {
        /// `<hostname>:<port><<route id>>`.
        endpoint: String,
        /// The failure's message.
        message: String,
    },
}

impl RelayError {
    /// The error of a selection that found no ready host among `hosts`.
    pub fn host_no_response(hosts: &[RelayHost]) -> RelayError {
        let hosts: Vec<String> = hosts.iter().map(|host| host.hostname.to_string()).collect();
        RelayError::HostNoResponse {
            hosts: hosts.join(","),
        }
    }

    /// The error of a call on `route` to `host` that failed with `message`.
    pub fn communication(
        host: &RelayHost,
        route: RouteId,
        message: impl Into<String>,
    ) -> RelayError {
        RelayError::Communication {
            endpoint: format!("{}:{}<{}>", host.hostname, host.port, route.id()),
            message: message.into(),
        }
    }

    /// Whether the relay answered that it is not ready ("App is not ready", a 499 before its
    /// blockchain has booted), after which the loop logs "Waiting for relay to become ready" once.
    pub fn is_app_not_ready(&self) -> bool {
        matches!(self, RelayError::Communication { message, .. } if message.contains("App is not ready"))
    }
}

/// The component's options were refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OptionsError {
    /// An option has the wrong type or is out of range.
    #[error("the forger option {key} is not valid: {reason}")]
    Invalid {
        /// The option's key, such as `hosts[0].port`.
        key: String,
        /// Why.
        reason: String,
    },
}

/// `delegates.json` was refused; the forger does not start.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeysError {
    /// There is no file: the reference's `MissingConfigFile`.
    #[error("The {} file does not exist", path.display())]
    Missing {
        /// The file.
        path: PathBuf,
    },
    /// Neither `keys` nor `secrets` holds an entry: the reference's `KeysNotDetected`.
    #[error("Unable to detect any private keys")]
    NoKeys,
    /// The file can be read by its group or by others.
    #[error(
        "{} can be read by its group or by others (mode {mode:03o}); make it private (chmod 600)",
        path.display()
    )]
    Readable {
        /// The file.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
    },
    /// `secrets` holds passphrases, which the forger does not read.
    #[error(
        "{} lists passphrases in `secrets`, which Heartwood does not read: give the delegates' private keys in `keys`",
        path.display()
    )]
    Secrets {
        /// The file.
        path: PathBuf,
    },
    /// The file is not JSON, has a key other than `keys` and `secrets`, or holds a key that is not
    /// 64 hex digits of a valid secret key.
    #[error("{} is not valid: {reason}", path.display())]
    Invalid {
        /// The file.
        path: PathBuf,
        /// Why, without the key's value.
        reason: String,
    },
    /// The file could not be read.
    #[error("{} cannot be read: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The system's error.
        message: String,
    },
}

/// A block could not be assembled or signed from its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ForgeError {
    /// A sum over the transactions exceeds 64 bits, where the reference's serialiser throws.
    #[error("the block's {field} exceeds 64 bits")]
    TotalOverflow {
        /// `totalAmount` or `totalFee`.
        field: &'static str,
    },
    /// More transactions than a block header can count.
    #[error("{count} transactions do not fit one block")]
    TooManyTransactions {
        /// The number of transactions.
        count: usize,
    },
    /// The previous height is the largest a block can have.
    #[error("no block can follow height {height}")]
    Height {
        /// The previous height.
        height: u32,
    },
    /// The block factory refused the block.
    #[error("the block cannot be made: {0}")]
    Block(#[from] BlockError),
}

/// The signing record could not be opened or written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecordError {
    /// A file operation failed.
    #[error("the signing record {} cannot be written or read: {message}", path.display())]
    Io {
        /// The record's file.
        path: PathBuf,
        /// The system's error.
        message: String,
    },
    /// The file is not a signing record.
    #[error("the signing record {} is not valid: {reason}", path.display())]
    Corrupt {
        /// The record's file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// The record belongs to another network: the forger does not start.
    #[error("the signing record {} is for the network {recorded}, not {expected}", path.display())]
    Network {
        /// The record's file.
        path: PathBuf,
        /// The nethash the record holds.
        recorded: String,
        /// This network's nethash.
        expected: String,
    },
}

/// The signer did not give a block for a slot.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SignError {
    /// The forger holds no key for the delegate.
    #[error("no key for the delegate {public_key}")]
    UnknownKey {
        /// The delegate's public key, in hex.
        public_key: String,
    },
    /// The record holds a later slot for the key.
    #[error("slot {slot} is below slot {recorded_slot}, which this delegate has already signed")]
    Refused {
        /// The slot asked for.
        slot: i64,
        /// The key's recorded slot.
        recorded_slot: i64,
    },
    /// The slot is more than one away from the slot of the forger's own clock.
    #[error("slot {slot} is more than one slot away from this forger's slot {own_slot}")]
    OutOfWindow {
        /// The slot asked for.
        slot: i64,
        /// The slot of the forger's clock.
        own_slot: i64,
    },
    /// The block could not be made.
    #[error(transparent)]
    Forge(#[from] ForgeError),
    /// The record could not be written, so the block is not sent.
    #[error(transparent)]
    Record(#[from] RecordError),
}

/// The attester refused an offered status, or could not sign it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AttestError {
    /// The text is not the canonical form of a status tree.
    #[error("the offered status cannot be read: {0}")]
    Preimage(#[from] PreimageError),
    /// The state's height is not its header's.
    #[error("the offered status's height {state} is not its header's height {header}")]
    Height {
        /// `state.height`.
        state: u32,
        /// `state.header.height`.
        header: u32,
    },
    /// The status's slot is more than one away from the slot of the forger's clock.
    #[error("the offered status's slot {offered} is more than one slot away from slot {own}")]
    Slot {
        /// `state.currentSlot`.
        offered: i64,
        /// The slot of the forger's clock.
        own: i64,
    },
    /// The status is of another network.
    #[error("the offered status is for the network {offered}, not {own}")]
    Network {
        /// The status's nethash.
        offered: String,
        /// This forger's nethash.
        own: String,
    },
    /// The offer names a key this forger does not hold.
    #[error("the offer asks for a signature by {public_key}, which this forger does not hold")]
    ForeignKey {
        /// The key, as offered.
        public_key: String,
    },
    /// A signature could not be made.
    #[error("the status cannot be signed: {0}")]
    Sign(#[from] SigError),
}
