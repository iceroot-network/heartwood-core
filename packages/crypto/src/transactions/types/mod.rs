//! A transaction and its fields.

use std::fmt;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::crypto::sig::Signature;
use crate::errors::{HexError, MemoError};
use crate::identities::{Address, PublicKey, PublicKeyBytes};
use crate::managers::Params;
use crate::transactions::TransactionType;
use crate::transactions::registry::TYPE_GROUP_CORE;
use crate::utils::amounts::format_amount;
use crate::utils::hex;

pub mod core;
pub mod group2;

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

/// A memo: 1 to 255 bytes of UTF-8. An empty memo and no memo are the same on the wire, so an
/// empty memo is written as `None`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Memo(String);

impl Memo {
    /// The most bytes a memo may have.
    pub const MAX_BYTES: usize = 255;

    /// The memo `text`, if it is 1 to 255 bytes long.
    pub fn new(text: &str) -> Result<Memo, MemoError> {
        match text.len() {
            0 => Err(MemoError::Empty),
            length if length > Memo::MAX_BYTES => Err(MemoError::TooLong { length }),
            _ => Ok(Memo(text.to_owned())),
        }
    }

    /// A memo read from the wire, which holds 1 to 255 bytes.
    pub(crate) fn from_wire(text: &str) -> Memo {
        Memo(text.to_owned())
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

impl ResignationType {
    /// The type with the reference's number `value` (0, 1 or 2).
    pub const fn from_number(value: u8) -> Option<ResignationType> {
        match value {
            0 => Some(ResignationType::TemporaryResign),
            1 => Some(ResignationType::PermanentResign),
            2 => Some(ResignationType::NotResigned),
            _ => None,
        }
    }

    /// The reference's number of the type.
    pub const fn number(self) -> u8 {
        match self {
            ResignationType::TemporaryResign => 0,
            ResignationType::PermanentResign => 1,
            ResignationType::NotResigned => 2,
        }
    }
}

/// A vote asset: delegate names and their shares in basis points (10,000 is 100 %), in the order
/// of the reference's vote object (see [`crate::utils::sort_votes`]).
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
    /// 1/7: the kind of resignation; `None` when the wire has no resignation byte. A temporary
    /// resignation is written as no byte, so `Some(TemporaryResign)` comes only from a wire byte
    /// `0xff`, which the reference decodes and writes back as no byte.
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

impl Asset {
    /// The transaction type this asset belongs to.
    pub const fn kind(&self) -> TransactionType {
        match self {
            Asset::Transfer { .. } => TransactionType::Transfer,
            Asset::SecondSignature { .. } => TransactionType::SecondSignature,
            Asset::DelegateRegistration { .. } => TransactionType::DelegateRegistration,
            Asset::DelegateResignation { .. } => TransactionType::DelegateResignation,
            Asset::Burn { .. } => TransactionType::Burn,
            Asset::Vote { .. } => TransactionType::Vote,
        }
    }
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
        Address::from_public_key(&self.sender_public_key, params.network())
    }

    /// The top-level amount: the burned amount of a burn, 0 for every other type. A block's total
    /// amount sums this value; transfer items do not count.
    pub fn amount(&self) -> u64 {
        match &self.asset {
            Asset::Burn { amount } => *amount,
            Asset::Transfer { .. }
            | Asset::SecondSignature { .. }
            | Asset::DelegateRegistration { .. }
            | Asset::DelegateResignation { .. }
            | Asset::Vote { .. } => 0,
        }
    }

    /// The version the reference reads: the wire byte 0 is version 3.
    pub fn effective_version(&self) -> u8 {
        if self.version == 0 { 3 } else { self.version }
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

    /// The reference's key for the type (`transfer`, `secondSignature`, `delegateRegistration`,
    /// `delegateResignation`, `burn` or `vote`), which the milestones' fee tables use.
    pub fn key(&self) -> &'static str {
        self.data.kind.key()
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
    /// lowercase hex, `typeGroup` left out for type group 1, and the keys in the reference's order.
    /// Vote shares are percentages, written as integers when they are whole.
    pub fn to_json(&self, params: &Params) -> Value {
        let data = &self.data;
        let mut out = Map::new();
        out.insert("headerType".into(), Value::from(0));
        out.insert("version".into(), Value::from(data.effective_version()));
        out.insert("network".into(), Value::from(data.network));
        if data.kind.type_group() != TYPE_GROUP_CORE {
            out.insert("typeGroup".into(), Value::from(data.kind.type_group()));
        }
        out.insert("type".into(), Value::from(data.kind.type_id()));
        out.insert("nonce".into(), Value::from(format_amount(data.nonce)));
        out.insert(
            "senderPublicKey".into(),
            Value::from(data.sender_public_key.to_hex()),
        );
        out.insert(
            "senderId".into(),
            Value::from(data.sender_address(params).to_base58()),
        );
        out.insert("fee".into(), Value::from(format_amount(data.fee)));
        if let Some(memo) = &data.memo {
            out.insert("memo".into(), Value::from(memo.as_str()));
        }
        match &data.asset {
            Asset::Burn { amount } => {
                out.insert("amount".into(), Value::from(format_amount(*amount)));
            }
            Asset::DelegateResignation {
                resignation_type: None,
            } => {}
            asset => {
                out.insert("asset".into(), asset_json(asset));
            }
        }
        if let Some(signature) = &data.signature {
            out.insert("signature".into(), Value::from(signature.to_hex()));
        }
        if let Some(signature) = &data.second_signature {
            out.insert("secondSignature".into(), Value::from(signature.to_hex()));
        }
        out.insert("id".into(), Value::from(self.id.to_hex()));
        Value::Object(out)
    }
}

impl Transaction {
    /// The transaction as the reference's `toString` writes it in its log lines and pool
    /// messages: `<senderId>#<nonce> <last 8 hex digits of the id> <Key> v<version>`, the type key
    /// capitalised (`Transfer`, `SecondSignature`, `DelegateRegistration`,
    /// `DelegateResignation`, `Burn`, `Vote`). The sender's address is derived with `network`,
    /// the node's address byte, as the reference derives it, whatever the transaction's own
    /// network byte.
    pub fn display(&self, network: u8) -> TransactionDisplay<'_> {
        TransactionDisplay {
            transaction: self,
            network,
        }
    }
}

/// A transaction written as the reference's `toString` writes it (see [`Transaction::display`]).
#[derive(Debug, Clone, Copy)]
pub struct TransactionDisplay<'a> {
    transaction: &'a Transaction,
    network: u8,
}

impl fmt::Display for TransactionDisplay<'_> {
    /// The version is the one the reference reads: a wire byte 0 is `v3`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = &self.transaction.data;
        let sender = Address::from_public_key(&data.sender_public_key, self.network);
        let id = self.transaction.id.to_hex();
        let key = data.kind.key();
        let (first, rest) = key.split_at(key.len().min(1));
        write!(
            f,
            "{}#{} {} {}{rest} v{}",
            sender.to_base58(),
            data.nonce,
            id.get(id.len().saturating_sub(8)..).unwrap_or_default(),
            first.to_ascii_uppercase(),
            data.effective_version()
        )
    }
}

/// The `asset` object of the reference's JSON form.
fn asset_json(asset: &Asset) -> Value {
    let mut out = Map::new();
    match asset {
        Asset::Transfer { transfers } => {
            let items = transfers
                .iter()
                .map(|item| {
                    let mut entry = Map::new();
                    entry.insert("amount".into(), Value::from(format_amount(item.amount)));
                    entry.insert(
                        "recipientId".into(),
                        Value::from(item.recipient.to_base58()),
                    );
                    Value::Object(entry)
                })
                .collect();
            out.insert("transfers".into(), Value::Array(items));
        }
        Asset::SecondSignature { public_key } => {
            let mut signature = Map::new();
            signature.insert("publicKey".into(), Value::from(public_key.to_hex()));
            out.insert("signature".into(), Value::Object(signature));
        }
        Asset::DelegateRegistration { username } => {
            let mut delegate = Map::new();
            delegate.insert("username".into(), Value::from(username.as_str()));
            out.insert("delegate".into(), Value::Object(delegate));
        }
        Asset::DelegateResignation { resignation_type } => {
            if let Some(kind) = resignation_type {
                out.insert("resignationType".into(), Value::from(kind.number()));
            }
        }
        Asset::Burn { amount } => {
            out.insert("amount".into(), Value::from(format_amount(*amount)));
        }
        Asset::Vote { votes } => {
            let shares = votes
                .iter()
                .map(|(name, &basis_points)| (name.clone(), percent_value(basis_points)))
                .collect();
            out.insert("votes".into(), Value::Object(shares));
        }
    }
    Value::Object(out)
}

/// A vote share as the reference holds it: the JavaScript number `basis_points / 100`, which is an
/// integer when the share is whole.
pub(crate) fn percent_value(basis_points: u16) -> Value {
    if basis_points.is_multiple_of(100) {
        Value::from(basis_points / 100)
    } else {
        Value::from(f64::from(basis_points) / 100.0)
    }
}
