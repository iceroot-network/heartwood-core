//! The crate's errors.
//!
//! Each error names the JavaScript error class that the reference raises in the same case
//! (`js_class`). The differential tests compare these classes; messages are never compared.

use thiserror::Error;

/// A nes frame was refused by the parser, or cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FrameError {
    /// Parsing: fewer than the 14 header bytes.
    #[error("a nes frame has at least 14 bytes, got {0}")]
    TooShort(usize),
    /// Parsing: a type byte other than hello (0), ping (1), post (3) and error (9), the types the
    /// peer-to-peer port accepts.
    #[error("the nes frame type {0} is not accepted on the peer-to-peer port")]
    InvalidType(u8),
    /// Parsing: the path runs past the end of the frame.
    #[error("the nes path runs past the end of the frame")]
    InvalidPathLength,
    /// Parsing: the socket id is longer than 100 bytes, or runs past the end of the frame.
    #[error("the nes socket id is longer than 100 bytes or runs past the end of the frame")]
    InvalidSocketLength,
    /// Writing: the path, after the cut at 255 UTF-16 code units where one applies, is longer
    /// than 255 bytes of UTF-8, so its length does not fit the length byte.
    #[error("a nes path is at most 255 bytes of UTF-8, got {0}")]
    PathTooLong(usize),
    /// Writing: the socket id is longer than 255 bytes of UTF-8.
    #[error("a nes socket id is at most 255 bytes of UTF-8, got {0}")]
    SocketTooLong(usize),
}

impl FrameError {
    /// The JavaScript error class the reference raises: `Error` for every parse refusal, and
    /// `RangeError` when a length does not fit its byte.
    pub fn js_class(self) -> &'static str {
        match self {
            FrameError::TooShort(_)
            | FrameError::InvalidType(_)
            | FrameError::InvalidPathLength
            | FrameError::InvalidSocketLength => "Error",
            FrameError::PathTooLong(_) | FrameError::SocketTooLong(_) => "RangeError",
        }
    }
}

/// A payload of a public route was refused by its decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// A read past the end of the bytes: a varint, a `bytes` field, a fixed-width field or a
    /// group that the input cuts short (the protobuf library's `index out of range`).
    #[error("a protobuf read runs past the end of the payload")]
    IndexOutOfRange,
    /// An unknown field with wire type 4 (an end-group tag outside a group), 6 or 7.
    #[error("invalid protobuf wire type {0}")]
    InvalidWireType(u8),
    /// A getStatus response without `state`: the reference reads `header` of `null`.
    #[error("a status response has no state")]
    MissingState,
    /// A getStatus response whose `state` has no `header`: the reference reads an amount of
    /// `null`.
    #[error("a status response's state has no header")]
    MissingHeader,
    /// An amount's text is not an integer literal that JavaScript's `BigInt` reads.
    #[error("an amount is not an integer literal")]
    AmountSyntax,
    /// An amount's value needs more than 128 bits. The reference reads integers of any size; this
    /// crate refuses them on purpose (a documented difference), because reading them is unbounded
    /// work. The reference refuses such a getBlocks response at a later step (its reply schema, or
    /// when it writes the amount in 64 bits), but accepts the total amount and total fee of a
    /// getStatus reply whose header is at height 1 and verifies its attestations over the whole
    /// integer. Only a crafted payload has such an amount.
    #[error("an amount needs more than 128 bits")]
    AmountTooWide,
}

impl DecodeError {
    /// The JavaScript error class the reference raises, or `None` for
    /// [`DecodeError::AmountTooWide`], where the reference raises no error.
    pub fn js_class(self) -> Option<&'static str> {
        match self {
            DecodeError::IndexOutOfRange => Some("RangeError"),
            DecodeError::InvalidWireType(_) => Some("Error"),
            DecodeError::MissingState | DecodeError::MissingHeader => Some("TypeError"),
            DecodeError::AmountSyntax => Some("SyntaxError"),
            DecodeError::AmountTooWide => None,
        }
    }
}

/// A reply broke a rule of the reference's reply schema for its route. The variant names the part
/// of the reply whose rule failed first. The reference logs the reply and treats the peer as not
/// responding; it raises no error class of its own.
///
/// The getStatus parts are [`ReplyRefused::Root`] to [`ReplyRefused::Signatures`]; the other
/// routes' replies use [`ReplyRefused::Root`] and [`ReplyRefused::Header`] as well as their own
/// parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReplyRefused {
    /// The reply as a whole: a required key is missing (for getStatus: `state`, `config` or
    /// `signatures`), a key the schema does not allow is present, or a list has more items than
    /// the schema allows.
    #[error("the reply breaks its schema")]
    Root,
    /// `state`: `height`, `forgingAllowed` or `currentSlot` is missing or out of range.
    #[error("the status reply's state breaks the reply schema")]
    State,
    /// A block header breaks the block header schema: the getStatus `state.header` (neither a
    /// block header nor an empty object), or a block of a getBlocks reply.
    #[error("a block header breaks the block header schema")]
    Header,
    /// `config`: the version or a required key.
    #[error("the status reply's config breaks the reply schema")]
    Config,
    /// `config.network` or its token.
    #[error("the status reply's network breaks the reply schema")]
    Network,
    /// `config.plugins`: too many entries, or an entry that is not a valid plugin.
    #[error("the status reply's plugins break the reply schema")]
    Plugins,
    /// `publicKeys`: too many, or not 66 hex characters.
    #[error("the status reply's public keys break the reply schema")]
    PublicKeys,
    /// `signatures`: too many, or not 128 hex characters.
    #[error("the status reply's signatures break the reply schema")]
    Signatures,
    /// getPeers: an entry's `ip` is in neither the IPv4 nor the IPv6 format, or its `port` is not
    /// an integer from 0 to 65535.
    #[error("a getPeers entry breaks the reply schema")]
    Peer,
    /// getCommonBlocks: `common` is neither `null` nor a block with a height of at least 1 and a
    /// block id.
    #[error("the getCommonBlocks reply's common block breaks the reply schema")]
    Common,
    /// postBlock: `status` is not a boolean, or `height` is not an integer of at least 1.
    #[error("the postBlock reply breaks the reply schema")]
    PostBlock,
    /// getUnconfirmedTransactions: `poolSize` is not an integer of at least 0, or `transactions`
    /// has more entries than the largest `block.maxTransactions` of the milestones.
    #[error("the getUnconfirmedTransactions reply breaks the reply schema")]
    Unconfirmed,
}

/// A request the reference's client never writes, refused when it is built. A reference server
/// refuses each of these shapes and bans the caller's IP, so no such request may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RequestError {
    /// The request headers' version is empty.
    #[error("a request's headers need a version")]
    EmptyVersion,
    /// A getCommonBlocks request with no id, or with more than 10.
    #[error("a getCommonBlocks request asks for 1 to 10 ids, got {0}")]
    CommonBlockIds(usize),
}
