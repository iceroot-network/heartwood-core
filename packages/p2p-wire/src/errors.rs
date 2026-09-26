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
    /// work and the reference refuses every such amount at a later step anyway.
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

/// A getStatus reply broke a rule of the reference's reply schema. The variant names the part of
/// the reply whose rule failed. The reference logs the reply and treats the peer as not
/// responding; it raises no error class of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReplyRefused {
    /// The reply object: a required key is missing (`state`, `config` or `signatures`).
    #[error("the status reply lacks state, config or signatures")]
    Root,
    /// `state`: `height`, `forgingAllowed` or `currentSlot` is missing or out of range.
    #[error("the status reply's state breaks the reply schema")]
    State,
    /// `state.header`: neither a block header nor an empty object.
    #[error("the status reply's header breaks the block header schema")]
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
}
