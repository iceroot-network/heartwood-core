//! The crate's errors.
//!
//! Where the reference raises an error, the error's text (its `Display`) is the reference's
//! `${error}`, class name and message, because the reference logs and groups peers by it (the
//! cleanse's "Removed N peers because of ...").

use std::net::IpAddr;

use heartwood_p2p_wire::FrameError;
use thiserror::Error;

/// A request on a nes session failed. Every such error is of the JavaScript class `Error` in the
/// reference, so the communicator only counts it against the peer ([`SessionError::js_class`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    /// The request's timeout expired: "Request timed out".
    #[error("Error: Request timed out")]
    Timeout,
    /// The connection closed while the request was pending, a close for a message above the
    /// receive limit (1009) included: "Request failed - server disconnected".
    #[error("Error: Request failed - server disconnected")]
    Disconnect,
    /// The server answered with a status of 400 or more: the status and the first 512 bytes of the
    /// payload as text.
    #[error("Error: {payload}")]
    Server {
        /// The response's status code.
        status: u16,
        /// The first 512 bytes of the response's payload, as text.
        payload: String,
    },
    /// The server broke the nes protocol; the connector terminates and forgets the connection,
    /// and the requests pending on it fail with this error.
    #[error("Error: {0}")]
    Protocol(ProtocolError),
    /// The WebSocket could not be opened, or failed: the transport's error, as text.
    #[error("Error: {0}")]
    Ws(String),
    /// The session was used in a way it does not allow (the reference's `user` errors).
    #[error("Error: {0}")]
    User(&'static str),
}

impl SessionError {
    /// The reference's JavaScript class of the error: always `Error`, the class of every error the
    /// nes client raises.
    pub fn js_class(&self) -> &'static str {
        "Error"
    }
}

/// How a server broke the nes protocol. The session reports each to the connector, which
/// terminates and forgets the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProtocolError {
    /// A response to an id with no pending request.
    #[error("Received response for unknown request")]
    UnknownRequest,
    /// A text frame.
    #[error("Received message is not a Buffer")]
    TextFrame,
    /// A frame that does not parse as a nes message.
    #[error("Invalid message: {0}")]
    Frame(FrameError),
    /// A message whose type is not hello, request or ping.
    #[error("Received unknown response type")]
    UnknownType,
    /// Two server pings less than 1 s apart.
    #[error("Ping exceeded limit")]
    PingFlood,
    /// A WebSocket ping or pong frame, which the reference's client never expects.
    #[error("Received a WebSocket control frame")]
    ControlFrame,
    /// No message within the heartbeat interval plus its timeout.
    #[error("Disconnecting due to heartbeat timeout")]
    HeartbeatTimeout,
}

/// A ping of a peer failed. The cleanse disconnects and forgets the peer, and acceptance refuses
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PingError {
    /// The peer did not answer getStatus, or its answer was refused.
    #[error("PeerStatusResponseError: Failed to retrieve status from peer {0}")]
    StatusResponse(IpAddr),
    /// The peer's network, version or chain did not pass verification.
    #[error("PeerVerificationFailedError: Peer verification failed")]
    VerificationFailed,
    /// The ping's deadline passed before the verification started.
    #[error("PeerPingTimeoutError: Ping timeout ({timeout_ms} ms)")]
    PingTimeout {
        /// The ping's timeout, in milliseconds.
        timeout_ms: u64,
    },
    /// The deadline passed during the verification.
    #[error("Error: {0}")]
    Deadline(PastDeadline),
}

/// A peer verification stopped where the reference's throws: its deadline passed before a request
/// could be sent, a fork-segment header could not be read, or the node could not read its chain.
/// The ping then fails and keeps the peer's last verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("timeout elapsed before successful completion of the verification")]
pub struct PastDeadline;

/// The node could not read its chain for a peer's request or a verification: the node's own
/// error, as text. The P2P crate reaches the store only through [`crate::view::BlockLookup`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the node could not read its chain: {0}")]
pub struct LookupError(pub String);

/// A downloaded block could not be read as the reference reads it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DownloadError {
    /// A transaction of a downloaded block that the reference's `fromBytesUnsafe` refuses; the
    /// reference then fails the whole download from that peer.
    #[error("{source}")]
    Transaction {
        /// The height of the block.
        height: u32,
        /// The decoder's refusal.
        source: heartwood_crypto::errors::DecodeError,
    },
}

/// A downloaded block could not be rebuilt as the reference's `BlockFactory.fromData` rebuilds
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssembleError {
    /// A header field is not in the form the block needs (an id, key, hash or signature that is
    /// not hex of its length, an amount out of range).
    #[error("the block header's {field} is not valid")]
    Field {
        /// The field, by the reference's name.
        field: &'static str,
    },
    /// The block factory refused the block.
    #[error("{0}")]
    Block(#[from] heartwood_crypto::errors::BlockError),
}

/// The seed peers could not be gathered.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SeedError {
    /// Neither `peers.json` nor its sources gave a seed: the node stops.
    #[error("No seed peers defined in peers.json")]
    NoSeeds,
    /// A peer source could not be read.
    #[error("the peer source {source_name} could not be read: {reason}")]
    Source {
        /// The source, as `peers.json` names it.
        source_name: String,
        /// Why it could not be read.
        reason: String,
    },
}

/// A p2p option of `app.json` that the reference's option schema refuses.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the p2p option {key} is not valid: {reason}")]
pub struct OptionsError {
    /// The option's key.
    pub key: String,
    /// Why it is refused.
    pub reason: String,
}
