//! A transaction and its fields.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use std::fmt;

use indexmap::IndexMap;

use crate::crypto::sig::Signature;
use crate::errors::{DecodeError, HexError};
use crate::identities::{Address, PublicKey, PublicKeyBytes};
use crate::managers::Params;
use crate::transactions::TransactionType;
use crate::utils::hex;

pub mod core;
pub mod solar;

/// A transaction id: the SHA-256 of the transaction's serialised bytes, signatures included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransactionId([u8; 32]);

impl TransactionId {
    /// The id with these 32 bytes.
    pub const fn from_array(bytes: [u8; 32]) -> TransactionId {
        TransactionId(bytes)
    }

    /// The id in the hex `text` (64 hex digits).
    pub fn from_hex(text: &str) -> Result<TransactionId, HexError> {
        hex::decode_array(text).map(TransactionId)
    }

    /// The 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, as the reference writes ids.
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A memo: at most 255 bytes of UTF-8. An empty memo and no memo are the same on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Memo(String);

impl Memo {
    /// The memo `text`, if it is 1 to 255 bytes long.
    pub fn new(text: &str) -> Result<Memo, DecodeError> {
        todo!()
    }

    /// The memo text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One item of a transfer: an amount to a recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferItem {
    /// Amount in base units.
    pub amount: u64,
    /// The recipient.
    pub recipient: Address,
}

/// The resignation kinds of a delegate resignation, numbered as the reference's delegate status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResignationType {
    /// 0: a temporary resignation. It is written as no byte at all.
    TemporaryResign,
    /// 1: a permanent resignation.
    PermanentResign,
    /// 2: the end of a temporary resignation.
    NotResigned,
}

/// A vote asset: delegate names and their shares in basis points (10,000 is 100 %), in the order
/// they appear on the wire.
pub type Votes = IndexMap<String, u16>;

/// The type-specific part of a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asset {
    /// 1/6: one or more transfer items.
    Transfer {
        /// The items, in wire order.
        transfers: Vec<TransferItem>,
    },
    /// 1/1: the second public key to register.
    SecondSignature {
        /// The key, as the reference stores it (not checked to be a curve point).
        public_key: PublicKeyBytes,
    },
    /// 1/2: the delegate name to register.
    DelegateRegistration {
        /// The delegate name.
        username: String,
    },
    /// 1/7: the kind of resignation; `None` when the wire has no resignation byte.
    DelegateResignation {
        /// The kind of resignation.
        resignation_type: Option<ResignationType>,
    },
    /// 2/0: the amount to burn.
    Burn {
        /// Amount in base units.
        amount: u64,
    },
    /// 2/2: the votes; empty to remove all votes.
    Vote {
        /// The votes, in wire order.
        votes: Votes,
    },
}

/// A transaction's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionData {
    /// The version byte as it was on the wire: 3, or 0, which the reference reads as version 3.
    pub version: u8,
    /// The network byte.
    pub network: u8,
    /// The transaction's type (type group and type).
    pub kind: TransactionType,
    /// The sender's nonce.
    pub nonce: u64,
    /// The sender's public key.
    pub sender_public_key: PublicKey,
    /// The fee in base units.
    pub fee: u64,
    /// The memo, if any.
    pub memo: Option<Memo>,
    /// The type-specific part.
    pub asset: Asset,
    /// The sender's signature.
    pub signature: Option<Signature>,
    /// The second signature, for senders with a registered second key.
    pub second_signature: Option<Signature>,
}

impl TransactionData {
    /// The sender's address on the network of `params`, as the reference derives it from the
    /// sender key.
    pub fn sender_address(&self, params: &Params) -> Address {
        todo!()
    }

    /// The top-level amount: the burned amount of a burn, 0 for every other type. A block's total
    /// amount sums this value; transfer items do not count.
    pub fn amount(&self) -> u64 {
        todo!()
    }
}

/// A decoded or built transaction: its fields, its id and its serialised bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub(crate) data: TransactionData,
    pub(crate) id: TransactionId,
    pub(crate) bytes: Vec<u8>,
    pub(crate) verified: bool,
}

impl Transaction {
    /// The fields.
    pub fn data(&self) -> &TransactionData {
        &self.data
    }

    /// The id.
    pub fn id(&self) -> &TransactionId {
        &self.id
    }

    /// The serialised bytes, as decoded or as built.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The type.
    pub fn kind(&self) -> TransactionType {
        self.data.kind
    }

    /// The fee in base units.
    pub fn fee(&self) -> u64 {
        self.data.fee
    }

    /// The top-level amount (see [`TransactionData::amount`]).
    pub fn amount(&self) -> u64 {
        self.data.amount()
    }

    /// Whether the sender's signature verified when the transaction was decoded. A transaction from
    /// [`Transaction::decode_trusted`] counts as verified, as in the reference.
    pub fn is_verified(&self) -> bool {
        self.verified
    }

    /// The transaction as the reference's `toJson` writes it: amounts as decimal strings, bytes as
    /// lowercase hex, `typeGroup` left out for type group 1.
    pub fn to_json(&self, params: &Params) -> serde_json::Value {
        todo!()
    }
}
