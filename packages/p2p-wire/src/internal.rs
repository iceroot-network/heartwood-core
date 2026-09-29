//! The JSON bodies of the internal routes: what a forger and its relay exchange.
//!
//! The reference's internal codecs are `JSON.stringify` of the object a controller returns and
//! `JSON.parse` of the bytes, read as UTF-8 text. Each body here is typed with the reference's
//! field names, JSON types and key order for every field its forger reads; the other fields are
//! written as the reference writes them where that is cheap. [`encode`] writes a body's bytes and
//! [`decode`] reads them, refusing a body whose used fields are missing or of another type, where
//! the reference's forger fails later with a `TypeError` or computes with the odd value. A field
//! the forger does not use is read when it has its type and left empty otherwise.
//!
//! | Route | Request | Response |
//! |---|---|---|
//! | `getCurrentRound` | `{}` | [`CurrentRound`] |
//! | `getSlotNumber` | [`SlotNumberRequest`] | [`SlotNumber`] |
//! | `getNetworkState` | [`NetworkStateRequest`] | [`NetworkState`] |
//! | `syncBlockchain` | `{}` | `true` |
//! | `emitEvent` | [`EmitEvent`] | `true` |
//! | `getStatusToSign` (Heartwood only) | [`StatusToSignRequest`] | [`StatusToSignResponse`] |
//! | `postStatusSignatures` (Heartwood only) | [`StatusSignatures`] | `true` |
//!
//! [`QuorumDetails`] carries the counts of the reference's forging quorum and its arithmetic:
//! [`QuorumDetails::quorum`] and [`QuorumDetails::can_forge`]; [`NetworkState`] adds the
//! reference's test status, which always forges.
//!
//! Every body is untrusted input on the side that reads it: decoding never panics, and a number
//! out of its type's range is refused.

use heartwood_crypto::utils::js;
use serde_json::{Map, Value, json};

use crate::errors::InternalBodyError;
use crate::routes::RouteId;

#[cfg(test)]
mod tests;

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
    js::stringify(&body.to_json(), 0).into_bytes()
}

/// The body in `bytes`: `JSON.parse`, then [`InternalBody::from_json`].
pub fn decode<T: InternalBody>(bytes: &[u8]) -> Result<T, InternalBodyError> {
    T::from_json(&parse(bytes)?)
}

/// `JSON.parse` of the bytes read as UTF-8 text, a malformed sequence read as U+FFFD as Node's
/// `toString` reads it: the value of a body the reader then ignores, such as `true`.
pub fn parse(bytes: &[u8]) -> Result<Value, InternalBodyError> {
    serde_json::from_str(&String::from_utf8_lossy(bytes))
        .map_err(|error| InternalBodyError::NotJson(error.to_string()))
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

/// A delegate's wallet as the round lists it, `{...wallet.getData(), delegate}`: its fields as the
/// relay writes them, and the two the forger reads.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegateWallet {
    /// The wallet's public key, in hex (`publicKey`).
    pub public_key: String,
    /// The delegate's username (`delegate.username`).
    pub username: String,
    /// Every field in the relay's order, `publicKey` and `delegate` included: what is written.
    pub fields: Map<String, Value>,
}

impl DelegateWallet {
    /// The wallet of `fields`, which must hold `publicKey` and `delegate.username` as texts.
    pub fn new(fields: Map<String, Value>) -> Result<DelegateWallet, InternalBodyError> {
        let username = fields
            .get("delegate")
            .and_then(|delegate| delegate.get("username"));
        Ok(DelegateWallet {
            public_key: string(fields.get("publicKey"), "publicKey")?,
            username: string(username, "delegate.username")?,
            fields,
        })
    }
}

/// The tip's block data, as `lastBlock.data` writes it: its fields as the relay writes them
/// (amounts as decimal text), and the height, which the forger reads.
#[derive(Debug, Clone, PartialEq)]
pub struct LastBlockData {
    /// The height.
    pub height: u32,
    /// Every field in the relay's order, `height` included: what is written.
    pub fields: Map<String, Value>,
}

impl LastBlockData {
    /// The block data of `fields`, which must hold the height.
    pub fn new(fields: Map<String, Value>) -> Result<LastBlockData, InternalBodyError> {
        let height = integer(fields.get("height"), "lastBlock.height")?;
        Ok(LastBlockData { height, fields })
    }
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

/// The status of a network state, the reference's numeric enum; unknown by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NetworkStateStatus {
    /// 0: the state was computed from the peers.
    Default = 0,
    /// 1: fewer active delegates across the peers than the minimum delegate reach.
    BelowMinimumDelegates = 1,
    /// 2: the reference's test environment (never produced by Heartwood); it always forges.
    Test = 2,
    /// 3: the relay started with `--networkStart` and has not ended its cold start.
    ColdStart = 3,
    /// 4: no state: the forger's reading of an answer without a status.
    #[default]
    Unknown = 4,
}

/// The answer to `getNetworkState`, as `NetworkState.parse` reads it on the forger's side.
#[derive(Debug, Clone, Default, PartialEq)]
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

impl NetworkState {
    /// The forger's reading of an answer without a status: status unknown, nothing else.
    pub fn unknown() -> NetworkState {
        NetworkState::default()
    }

    /// The reference's `getQuorum`: 1 with the test status, else [`QuorumDetails::quorum`].
    pub fn quorum(&self) -> f64 {
        match self.status {
            NetworkStateStatus::Test => 1.0,
            _ => self.quorum_details.quorum(),
        }
    }

    /// The reference's `canForge`: always with the test status, else
    /// [`QuorumDetails::can_forge`].
    pub fn can_forge(&self, active_delegates: u32) -> bool {
        self.status == NetworkStateStatus::Test || self.quorum_details.can_forge(active_delegates)
    }
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
    pub over_height_block_headers: Map<String, Value>,
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
        let has = f64::from(self.has_quorum);
        let quorum = has / (has + f64::from(self.no_quorum));
        if quorum.is_finite() { quorum } else { 0.0 }
    }

    /// Whether the quorum reaches `min(1, (floor(active_delegates / 2) + 1) / (hasQuorum +
    /// noQuorum))`, the reference's `canForge`, computed in integers so that it gives the
    /// reference's answer for every count.
    ///
    /// With no delegate counted the quorum is 0 and the threshold 1. Otherwise both sides are
    /// fractions of the same total, and two different counts below 2^33 never round to the same
    /// double, so the comparison of the doubles is the comparison of the counts: all of them
    /// when the half plus one is not below the total, else at least the half plus one.
    pub fn can_forge(&self, active_delegates: u32) -> bool {
        let total = u64::from(self.has_quorum).saturating_add(u64::from(self.no_quorum));
        let needed = u64::from(active_delegates / 2).saturating_add(1);
        total > 0 && u64::from(self.has_quorum) >= needed.min(total)
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

/// The `getStatusToSign` answer: a status to attest, `{status, publicKeys, challenge}`, or nothing
/// new within the long poll, `{unchanged: true}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusToSignResponse {
    /// The canonical status text to sign, the keys to sign it with (the declared keys that are
    /// registered delegates, in the declared order), and the session's challenge.
    Offer {
        /// The status text, as [`crate::status::signing_preimage`] writes it.
        status: String,
        /// The keys to sign with, in hex.
        public_keys: Vec<String>,
        /// The session's challenge, which each key signs as [`crate::status::session_proof`]
        /// writes it.
        challenge: String,
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
    /// One legacy Schnorr signature of the session's proof text per offered key, in the same
    /// order, in hex.
    pub proofs: Vec<String>,
}

impl InternalBody for CurrentRound {
    const ROUTE: RouteId = RouteId::GetCurrentRound;

    fn to_json(&self) -> Value {
        let wallet = |wallet: &DelegateWallet| Value::Object(wallet.fields.clone());
        let wallets = |list: &[DelegateWallet]| list.iter().map(wallet).collect::<Vec<_>>();
        defined(json!({
            "allDelegates": wallets(&self.all_delegates), "canForge": self.can_forge,
            "current": self.current, "currentForger": self.current_forger.as_ref().map(wallet),
            "delegates": wallets(&self.delegates), "lastBlock": self.last_block.fields,
            "nextForger": self.next_forger.as_ref().map(wallet), "reward": self.reward,
            "timestamp": self.timestamp,
        }))
    }

    fn from_json(value: &Value) -> Result<CurrentRound, InternalBodyError> {
        let round = object(Some(value), "round")?;
        let wallet = |value: &Value| DelegateWallet::new(object(Some(value), "wallet")?.clone());
        let wallets = |key: &str| match get(round, key) {
            Some(Value::Array(items)) => items.iter().map(wallet).collect(),
            _ => Err(field(key)),
        };
        Ok(CurrentRound {
            all_delegates: wallets("allDelegates")?,
            can_forge: get(round, "canForge").is_some_and(js::is_truthy),
            current: integer(get(round, "current"), "current").unwrap_or_default(),
            current_forger: get(round, "currentForger").map(wallet).transpose()?,
            delegates: wallets("delegates")?,
            last_block: LastBlockData::new(object(get(round, "lastBlock"), "lastBlock")?.clone())?,
            next_forger: get(round, "nextForger").map(wallet).transpose()?,
            reward: string(get(round, "reward"), "reward")?,
            timestamp: integer(get(round, "timestamp"), "timestamp")?,
        })
    }
}

impl InternalBody for SlotNumberRequest {
    const ROUTE: RouteId = RouteId::GetSlotNumber;

    fn to_json(&self) -> Value {
        defined(json!({ "timestamp": self.timestamp }))
    }

    fn from_json(value: &Value) -> Result<SlotNumberRequest, InternalBodyError> {
        let timestamp = optional(object(Some(value), "timestamp")?, "timestamp", integer)?;
        Ok(SlotNumberRequest { timestamp })
    }
}

impl InternalBody for SlotNumber {
    const ROUTE: RouteId = RouteId::GetSlotNumber;

    fn to_json(&self) -> Value {
        self.0.into()
    }

    fn from_json(value: &Value) -> Result<SlotNumber, InternalBodyError> {
        integer(Some(value), "slot").map(SlotNumber)
    }
}

impl InternalBody for NetworkStateRequest {
    const ROUTE: RouteId = RouteId::GetNetworkState;

    fn to_json(&self) -> Value {
        json!({ "log": self.log })
    }

    fn from_json(value: &Value) -> Result<NetworkStateRequest, InternalBodyError> {
        let log = get(object(Some(value), "log")?, "log").is_some_and(js::is_truthy);
        Ok(NetworkStateRequest { log })
    }
}

impl InternalBody for NetworkState {
    const ROUTE: RouteId = RouteId::GetNetworkState;

    /// The relay's `NetworkState` object: its status, its quorum's counts, then the tip's fields
    /// in the order the reference sets them, each only when it is set.
    fn to_json(&self) -> Value {
        let (details, lists) = (&self.quorum_details, &self.quorum_details.delegates);
        defined(json!({
            "status": self.status as u8,
            "quorumDetails": {
                "hasQuorum": details.has_quorum, "noQuorum": details.no_quorum,
                "overHeight": details.over_height,
                "overHeightBlockHeaders": details.over_height_block_headers,
                "forked": details.forked, "differentSlot": details.different_slot,
                "forgingNotAllowed": details.forging_not_allowed,
                "delegates": {
                    "hasQuorum": lists.has_quorum, "noQuorum": lists.no_quorum,
                    "noResponse": lists.no_response,
                },
            },
            "nodeHeight": self.node_height, "lastBlockId": self.last_block_id,
            "lastGenerator": self.last_generator, "lastSlotNumber": self.last_slot_number,
        }))
    }

    /// `NetworkState.parse`: an answer that is not an object or has no status is the unknown
    /// state; the counts the answer leaves out are 0.
    fn from_json(value: &Value) -> Result<NetworkState, InternalBodyError> {
        let (Some(status), Value::Object(state)) = (value.get("status"), value) else {
            return Ok(NetworkState::unknown());
        };
        let status = match js::safe_integer(status) {
            Some(0) => NetworkStateStatus::Default,
            Some(1) => NetworkStateStatus::BelowMinimumDelegates,
            Some(2) => NetworkStateStatus::Test,
            Some(3) => NetworkStateStatus::ColdStart,
            Some(4) => NetworkStateStatus::Unknown,
            _ => return Err(field("status")),
        };
        let empty = Map::new();
        let details = part(state, "quorumDetails", &empty)?;
        let lists = part(details, "delegates", &empty)?;
        let counts = [
            "hasQuorum",
            "noQuorum",
            "overHeight",
            "forked",
            "differentSlot",
            "forgingNotAllowed",
        ];
        let [has, no, over, forked, different, refused] =
            counts.map(|key| optional(details, key, integer).map(Option::unwrap_or_default));
        let names =
            ["hasQuorum", "noQuorum", "noResponse"].map(|key| strings(get(lists, key), key));
        let [has_quorum, no_quorum, no_response] = names;
        let headers = part(details, "overHeightBlockHeaders", &empty)?;
        Ok(NetworkState {
            status,
            node_height: optional(state, "nodeHeight", integer)?,
            last_block_id: optional(state, "lastBlockId", string)?,
            last_generator: optional(state, "lastGenerator", string)?,
            last_slot_number: optional(state, "lastSlotNumber", integer)?,
            quorum_details: QuorumDetails {
                has_quorum: has?,
                no_quorum: no?,
                over_height: over?,
                over_height_block_headers: headers.clone(),
                forked: forked?,
                different_slot: different?,
                forging_not_allowed: refused?,
                delegates: QuorumDelegates {
                    has_quorum: has_quorum?,
                    no_quorum: no_quorum?,
                    no_response: no_response?,
                },
            },
        })
    }
}

impl InternalBody for EmitEvent {
    const ROUTE: RouteId = RouteId::EmitEvent;

    fn to_json(&self) -> Value {
        json!({ "event": self.event, "body": self.body })
    }

    fn from_json(value: &Value) -> Result<EmitEvent, InternalBodyError> {
        let event = object(Some(value), "event")?;
        Ok(EmitEvent {
            event: string(get(event, "event"), "event")?,
            body: get(event, "body").cloned().unwrap_or(Value::Null),
        })
    }
}

impl InternalBody for StatusToSignRequest {
    const ROUTE: RouteId = RouteId::GetStatusToSign;

    fn to_json(&self) -> Value {
        json!({ "publicKeys": self.public_keys })
    }

    fn from_json(value: &Value) -> Result<StatusToSignRequest, InternalBodyError> {
        let public_keys = listed(object(Some(value), "publicKeys")?, "publicKeys")?;
        Ok(StatusToSignRequest { public_keys })
    }
}

impl InternalBody for StatusToSignResponse {
    const ROUTE: RouteId = RouteId::GetStatusToSign;

    fn to_json(&self) -> Value {
        match self {
            StatusToSignResponse::Offer {
                status,
                public_keys,
                challenge,
            } => json!({ "status": status, "publicKeys": public_keys, "challenge": challenge }),
            StatusToSignResponse::Unchanged => json!({ "unchanged": true }),
        }
    }

    fn from_json(value: &Value) -> Result<StatusToSignResponse, InternalBodyError> {
        let answer = object(Some(value), "status")?;
        match get(answer, "status") {
            Some(status) => Ok(StatusToSignResponse::Offer {
                status: string(Some(status), "status")?,
                public_keys: listed(answer, "publicKeys")?,
                challenge: string(get(answer, "challenge"), "challenge")?,
            }),
            None if answer.get("unchanged") == Some(&Value::Bool(true)) => {
                Ok(StatusToSignResponse::Unchanged)
            }
            None => Err(field("status")),
        }
    }
}

impl InternalBody for StatusSignatures {
    const ROUTE: RouteId = RouteId::PostStatusSignatures;

    fn to_json(&self) -> Value {
        json!({ "offer": self.offer, "signatures": self.signatures, "proofs": self.proofs })
    }

    fn from_json(value: &Value) -> Result<StatusSignatures, InternalBodyError> {
        let signatures = object(Some(value), "offer")?;
        Ok(StatusSignatures {
            offer: string(get(signatures, "offer"), "offer")?,
            signatures: listed(signatures, "signatures")?,
            proofs: listed(signatures, "proofs")?,
        })
    }
}

/// `value` without its top-level `null` values: `JSON.stringify` leaves out a key whose value is
/// `undefined`, and no body here writes a `null` at its top level.
fn defined(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.retain(|_, found| !found.is_null());
    }
    value
}

fn field(path: &str) -> InternalBodyError {
    InternalBodyError::Field {
        field: path.to_owned(),
    }
}

/// `map[key]`, or `None` where it is absent or `null`, which the reference's readers treat as
/// not set.
fn get<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|found| !found.is_null())
}

fn object<'a>(
    value: Option<&'a Value>,
    path: &str,
) -> Result<&'a Map<String, Value>, InternalBodyError> {
    value.and_then(Value::as_object).ok_or_else(|| field(path))
}

/// The object `map[key]`, or `empty` where it is not set.
fn part<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    empty: &'a Map<String, Value>,
) -> Result<&'a Map<String, Value>, InternalBodyError> {
    get(map, key).map_or(Ok(empty), |found| object(Some(found), key))
}

/// `map[key]` read with `read`, or `None` where it is not set.
fn optional<T>(
    map: &Map<String, Value>,
    key: &str,
    read: impl Fn(Option<&Value>, &str) -> Result<T, InternalBodyError>,
) -> Result<Option<T>, InternalBodyError> {
    get(map, key)
        .map(|found| read(Some(found), key))
        .transpose()
}

/// A JSON number that `JSON.parse` reads as a safe integer, in the range of `T`.
fn integer<T: TryFrom<i64>>(value: Option<&Value>, path: &str) -> Result<T, InternalBodyError> {
    (value.and_then(js::safe_integer))
        .and_then(|number| T::try_from(number).ok())
        .ok_or_else(|| field(path))
}

fn string(value: Option<&Value>, path: &str) -> Result<String, InternalBodyError> {
    (value.and_then(Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| field(path))
}

/// A list of texts; none when absent.
fn strings(value: Option<&Value>, path: &str) -> Result<Vec<String>, InternalBodyError> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => (items.iter())
            .map(|item| string(Some(item), path))
            .collect(),
        Some(_) => Err(field(path)),
    }
}

/// The list of texts `map[key]`, which must be set.
fn listed(map: &Map<String, Value>, key: &str) -> Result<Vec<String>, InternalBodyError> {
    strings(Some(get(map, key).ok_or_else(|| field(key))?), key)
}
