//! The JSON bodies of the internal routes: what a forger and its relay exchange.
//!
//! The reference's internal codecs are `JSON.stringify` of the object a controller returns and
//! `JSON.parse` of the bytes. Each body here is typed with the reference's field names, JSON types
//! and key order for every field its forger reads; the other fields are written as the reference
//! writes them. [`encode`] writes a body's bytes and [`decode`] reads them, refusing a body whose
//! used fields are missing or of another type.
//!
//! | Route | Request | Response |
//! |---|---|---|
//! | `getCurrentRound` | `{}` | [`CurrentRound`] |
//! | `getSlotNumber` | [`SlotNumberRequest`] | [`SlotNumber`] |
//! | `getNetworkState` | [`NetworkStateRequest`] | [`NetworkState`] |
//! | `syncBlockchain` | `{}` | `{}` |
//! | `emitEvent` | [`EmitEvent`] | `{}` |
//! | `getStatusToSign` (Heartwood only) | [`StatusToSignRequest`] | [`StatusToSignResponse`] |
//! | `postStatusSignatures` (Heartwood only) | [`StatusSignatures`] | `{}` |
//!
//! [`QuorumDetails`] carries the counts of the reference's forging quorum and its arithmetic:
//! [`QuorumDetails::quorum`] and [`QuorumDetails::can_forge`].
//!
//! Every body is untrusted input on the side that reads it: decoding never panics, and a number
//! out of its type's range is refused.

use indexmap::IndexMap;
use serde_json::Value;

use crate::errors::InternalBodyError;
use crate::routes::RouteId;

/// A body of an internal route, written and read as the reference's `JSON.stringify` and
/// `JSON.parse` of its object.
pub trait InternalBody: Sized {
    /// The route whose request or response this body is.
    const ROUTE: RouteId;

    /// The body as the reference's object, keys in its order and numbers in its forms.
    fn to_json(&self) -> Value;

    /// The body read from a parsed value, as the reference's reader uses its fields.
    fn from_json(value: &Value) -> Result<Self, InternalBodyError>;
}

/// The bytes of `body`: `JSON.stringify` of the reference's object.
pub fn encode<T: InternalBody>(body: &T) -> Vec<u8> {
    let _ = body;
    todo!("the internal bodies' writer")
}

/// The body in `bytes`: `JSON.parse`, then [`InternalBody::from_json`].
pub fn decode<T: InternalBody>(bytes: &[u8]) -> Result<T, InternalBodyError> {
    let _ = bytes;
    todo!("the internal bodies' reader")
}

/// The answer to `getCurrentRound`, fields in the reference's order.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentRound {
    /// Every registered delegate (`allByUsername`), each as its wallet data and the delegate
    /// attribute.
    pub all_delegates: Vec<DelegateWallet>,
    /// The forging status of the relay's slot (the forger never reads it).
    pub can_forge: bool,
    /// The round of tip + 1.
    pub current: u64,
    /// The delegate at the current forger index, if the round has one there.
    pub current_forger: Option<DelegateWallet>,
    /// The round's active delegates, in forging order (`getActiveDelegates`).
    pub delegates: Vec<DelegateWallet>,
    /// The tip's block data.
    pub last_block: LastBlockData,
    /// The delegate at the next forger index, if the round has one there.
    pub next_forger: Option<DelegateWallet>,
    /// The reward of a block by the current forger, as decimal text.
    pub reward: String,
    /// The block timestamp of the current slot, in seconds after the epoch (negative before it).
    pub timestamp: i64,
}

/// A delegate's wallet as the round lists it: `{...wallet.getData(), delegate}`.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegateWallet {
    /// The wallet's address.
    pub address: String,
    /// The wallet's public key, in hex.
    pub public_key: String,
    /// The balance, as decimal text.
    pub balance: String,
    /// The nonce, as decimal text.
    pub nonce: String,
    /// The delegate attribute.
    pub delegate: DelegateAttribute,
}

/// A wallet's `delegate` attribute: the username the forger reads, and every key of the
/// attribute in the reference's order, as the relay writes them.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegateAttribute {
    /// The delegate's username.
    pub username: String,
    /// The attribute's keys and values, `username` included, in the reference's order.
    pub fields: IndexMap<String, Value>,
}

/// The tip's block data, as `lastBlock.data` writes it: amounts as decimal text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastBlockData {
    /// The block id, in hex.
    pub id: String,
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The height.
    pub height: u32,
    /// The previous block's id, in hex.
    pub previous_block: String,
    /// The number of transactions.
    pub number_of_transactions: u32,
    /// The sum of the transactions' amounts.
    pub total_amount: String,
    /// The sum of the transactions' fees.
    pub total_fee: String,
    /// The burned part of the fees, when the block data has it.
    pub burned_fee: Option<String>,
    /// The block reward.
    pub reward: String,
    /// The donations by address, when the block data has them.
    pub donations: Option<IndexMap<String, String>>,
    /// The generator's username, when the block data has it.
    pub username: Option<String>,
    /// The payload length.
    pub payload_length: u32,
    /// The payload hash, in hex.
    pub payload_hash: String,
    /// The generator's public key, in hex.
    pub generator_public_key: String,
    /// The block signature, in hex.
    pub block_signature: String,
}

/// The `getSlotNumber` request: the slot of `timestamp`, or of the relay's now when absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotNumberRequest {
    /// Seconds after the epoch.
    pub timestamp: Option<i64>,
}

/// The `getSlotNumber` answer: a slot number, a bare JSON number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotNumber(pub i64);

/// The `getNetworkState` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkStateRequest {
    /// Whether the relay logs the state it computes (the forger's first attempt of a slot).
    pub log: bool,
}

/// The status of a network state, the reference's numeric enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkStateStatus {
    /// 0: the state was computed from the peers.
    Default,
    /// 1: fewer active delegates across the peers than the minimum delegate reach.
    BelowMinimumDelegates,
    /// 2: the reference's test environment (never produced by Heartwood).
    Test,
    /// 3: the relay started with `--networkStart` and has not ended its cold start.
    ColdStart,
    /// 4: no state: the forger's reading of an answer without a status.
    Unknown,
}

/// The answer to `getNetworkState`, as `NetworkState.parse` reads it on the forger's side.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkState {
    /// The status.
    pub status: NetworkStateStatus,
    /// The relay's tip height.
    pub node_height: Option<u32>,
    /// The relay's tip id.
    pub last_block_id: Option<String>,
    /// The tip's generator, by username (the reference's `lastBlock.data.username`).
    pub last_generator: Option<String>,
    /// The tip's slot.
    pub last_slot_number: Option<i64>,
    /// The quorum's counts.
    pub quorum_details: QuorumDetails,
}

/// The counts of the forging quorum, each peer counted once per active delegate it carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuorumDetails {
    /// Delegates on peers at the relay's height and not forked.
    pub has_quorum: u32,
    /// Delegates on peers at another height, or forked.
    pub no_quorum: u32,
    /// Delegates on peers above the relay's height.
    pub over_height: u32,
    /// The headers of the peers above the relay's height, by peer IP, as the reference keys them.
    pub over_height_block_headers: IndexMap<String, Value>,
    /// Delegates on forked peers.
    pub forked: u32,
    /// Delegates on peers in another slot (statistics only).
    pub different_slot: u32,
    /// Delegates on peers whose forging is not allowed (statistics only).
    pub forging_not_allowed: u32,
    /// The delegates' names in each list.
    pub delegates: QuorumDelegates,
}

/// The delegates' names in each list of the quorum.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuorumDelegates {
    /// On peers that count toward the quorum.
    pub has_quorum: Vec<String>,
    /// On peers that count against it.
    pub no_quorum: Vec<String>,
    /// The round's delegates in neither list.
    pub no_response: Vec<String>,
}

impl QuorumDetails {
    /// `hasQuorum / (hasQuorum + noQuorum)`, and 0 when that is not finite.
    pub fn quorum(&self) -> f64 {
        todo!("the quorum")
    }

    /// Whether the quorum reaches `min(1, (floor(active_delegates / 2) + 1) / (hasQuorum +
    /// noQuorum))`, the reference's `canForge`, computed in integers so that it gives the
    /// reference's answer for every count.
    pub fn can_forge(&self, active_delegates: u32) -> bool {
        let _ = active_delegates;
        todo!("the forging quorum")
    }
}

/// The `emitEvent` request: an event name and its body.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitEvent {
    /// The event's name, such as `block.forged`.
    pub event: String,
    /// The event's body.
    pub body: Value,
}

/// The `getStatusToSign` request: the forger's keys, in `delegates.json` order, duplicates kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusToSignRequest {
    /// The public keys, in hex.
    pub public_keys: Vec<String>,
}

/// The `getStatusToSign` answer: a status to attest, or nothing new within the long poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusToSignResponse {
    /// The canonical status text to sign, and the keys to sign it with: the declared keys that are
    /// registered delegates, in the declared order.
    Offer {
        /// The status text, as [`crate::status::signing_preimage`] writes it.
        status: String,
        /// The keys to sign with, in hex.
        public_keys: Vec<String>,
    },
    /// The status did not change within the long poll.
    Unchanged,
}

/// The `postStatusSignatures` request: the attestations of one offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSignatures {
    /// The SHA-256 of the offered status bytes, in hex, naming the offer signed.
    pub offer: String,
    /// One legacy Schnorr signature per offered key, in the offer's order, in hex.
    pub signatures: Vec<String>,
}

impl InternalBody for CurrentRound {
    const ROUTE: RouteId = RouteId::GetCurrentRound;

    fn to_json(&self) -> Value {
        todo!("getCurrentRound's answer")
    }

    fn from_json(value: &Value) -> Result<CurrentRound, InternalBodyError> {
        let _ = value;
        todo!("getCurrentRound's answer")
    }
}

impl InternalBody for SlotNumberRequest {
    const ROUTE: RouteId = RouteId::GetSlotNumber;

    fn to_json(&self) -> Value {
        todo!("getSlotNumber's request")
    }

    fn from_json(value: &Value) -> Result<SlotNumberRequest, InternalBodyError> {
        let _ = value;
        todo!("getSlotNumber's request")
    }
}

impl InternalBody for SlotNumber {
    const ROUTE: RouteId = RouteId::GetSlotNumber;

    fn to_json(&self) -> Value {
        todo!("getSlotNumber's answer")
    }

    fn from_json(value: &Value) -> Result<SlotNumber, InternalBodyError> {
        let _ = value;
        todo!("getSlotNumber's answer")
    }
}

impl InternalBody for NetworkStateRequest {
    const ROUTE: RouteId = RouteId::GetNetworkState;

    fn to_json(&self) -> Value {
        todo!("getNetworkState's request")
    }

    fn from_json(value: &Value) -> Result<NetworkStateRequest, InternalBodyError> {
        let _ = value;
        todo!("getNetworkState's request")
    }
}

impl InternalBody for NetworkState {
    const ROUTE: RouteId = RouteId::GetNetworkState;

    fn to_json(&self) -> Value {
        todo!("getNetworkState's answer")
    }

    fn from_json(value: &Value) -> Result<NetworkState, InternalBodyError> {
        let _ = value;
        todo!("getNetworkState's answer")
    }
}

impl InternalBody for EmitEvent {
    const ROUTE: RouteId = RouteId::EmitEvent;

    fn to_json(&self) -> Value {
        todo!("emitEvent's request")
    }

    fn from_json(value: &Value) -> Result<EmitEvent, InternalBodyError> {
        let _ = value;
        todo!("emitEvent's request")
    }
}

impl InternalBody for StatusToSignRequest {
    const ROUTE: RouteId = RouteId::GetStatusToSign;

    fn to_json(&self) -> Value {
        todo!("getStatusToSign's request")
    }

    fn from_json(value: &Value) -> Result<StatusToSignRequest, InternalBodyError> {
        let _ = value;
        todo!("getStatusToSign's request")
    }
}

impl InternalBody for StatusToSignResponse {
    const ROUTE: RouteId = RouteId::GetStatusToSign;

    fn to_json(&self) -> Value {
        todo!("getStatusToSign's answer")
    }

    fn from_json(value: &Value) -> Result<StatusToSignResponse, InternalBodyError> {
        let _ = value;
        todo!("getStatusToSign's answer")
    }
}

impl InternalBody for StatusSignatures {
    const ROUTE: RouteId = RouteId::PostStatusSignatures;

    fn to_json(&self) -> Value {
        todo!("postStatusSignatures' request")
    }

    fn from_json(value: &Value) -> Result<StatusSignatures, InternalBodyError> {
        let _ = value;
        todo!("postStatusSignatures' request")
    }
}
