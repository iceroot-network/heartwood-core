//! The frame parser and writer.
//!
//! Parse checks, in the reference's order: at least 14 bytes; the type byte; the path length
//! against the frame; the socket length (at most 100) against the frame. Every refusal is of
//! class `Error`. Write: path and socket lengths are UTF-8 byte lengths, and one above 255 is a
//! `RangeError`; a socket id of 101 to 255 bytes is written, although every parser refuses it.

use super::Message;
use crate::errors::FrameError;

/// Parse `bytes` as a frame.
pub(crate) fn parse(bytes: &[u8]) -> Result<Message, FrameError> {
    let _ = bytes;
    todo!()
}

/// Write `message` as a frame.
pub(crate) fn encode(message: &Message) -> Result<Vec<u8>, FrameError> {
    let _ = message;
    todo!()
}
