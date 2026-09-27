//! The frame parser and writer.
//!
//! Parse checks, in the reference's order: at least 14 bytes; the type byte; the path length
//! against the frame; the socket length (at most 100) against the frame. Every refusal is of
//! class `Error`. Write: path and socket lengths are UTF-8 byte lengths, and one above 255 is a
//! `RangeError`; a socket id of 101 to 255 bytes is written, although every parser refuses it.

use super::{Heartbeat, Kind, Message, path};
use crate::errors::FrameError;
use crate::js;

/// The header's length in bytes.
const HEADER: usize = 14;
/// The longest socket id the parser accepts, in bytes.
const MAX_SOCKET: usize = 100;
/// The longest path, in bytes for the length byte and in UTF-16 code units for the writer's cut.
/// A parsed frame that is not a post and has a path of exactly this many bytes carries the rest of
/// its path in the payload.
const MAX_PATH: usize = 255;

impl Kind {
    /// The type byte: post for a request.
    pub(super) fn byte(self) -> u8 {
        match self {
            Kind::Hello => 0,
            Kind::Ping => 1,
            Kind::Request => 3,
            Kind::Error => 9,
        }
    }

    /// The kind of a type byte that the peer-to-peer port accepts.
    fn from_byte(byte: u8) -> Option<Kind> {
        match byte {
            0 => Some(Kind::Hello),
            1 => Some(Kind::Ping),
            3 => Some(Kind::Request),
            9 => Some(Kind::Error),
            _ => None,
        }
    }
}

/// Parse `bytes` as a frame.
pub(crate) fn parse(bytes: &[u8]) -> Result<Message, FrameError> {
    let (header, rest) = bytes
        .split_first_chunk::<HEADER>()
        .ok_or(FrameError::TooShort(bytes.len()))?;
    let kind = Kind::from_byte(header[1]).ok_or(FrameError::InvalidType(header[1]))?;
    let path_len = usize::from(header[8]);
    let (raw_path, rest) = rest
        .split_at_checked(path_len)
        .ok_or(FrameError::InvalidPathLength)?;
    let mut path = js::utf8_lossy(raw_path);
    if !path.is_empty() {
        path = path::normalise(&path);
    }
    let socket_len = usize::from(header[9]);
    let (raw_socket, payload) = rest
        .split_at_checked(socket_len)
        .filter(|_| socket_len <= MAX_SOCKET)
        .ok_or(FrameError::InvalidSocketLength)?;
    let mut payload = payload.to_vec();
    if kind != Kind::Request && path_len == MAX_PATH {
        path.push_str(&js::utf8_lossy(&payload));
        payload.clear();
    }
    Ok(Message {
        kind,
        version: header[0],
        id: u32::from_be_bytes([header[2], header[3], header[4], header[5]]),
        status_code: u16::from_be_bytes([header[6], header[7]]),
        path,
        socket: js::utf8_lossy(raw_socket),
        heartbeat: Heartbeat {
            interval: u16::from_be_bytes([header[10], header[11]]),
            timeout: u16::from_be_bytes([header[12], header[13]]),
        },
        payload,
    })
}

/// Write `message` as a frame.
pub(crate) fn encode(message: &Message) -> Result<Vec<u8>, FrameError> {
    // The reference cuts the path of a message without the post method; a request is a post.
    let cut = (message.kind != Kind::Request && js::utf16_len(&message.path) > MAX_PATH)
        .then(|| js::utf16_split(&message.path, MAX_PATH));
    let (path, payload) = match &cut {
        Some((path, rest)) => (path.as_str(), rest.as_bytes()),
        None => (message.path.as_str(), message.payload.as_slice()),
    };
    let path_len = u8::try_from(path.len()).map_err(|_| FrameError::PathTooLong(path.len()))?;
    let socket_len = u8::try_from(message.socket.len())
        .map_err(|_| FrameError::SocketTooLong(message.socket.len()))?;
    let status_code = match message.status_code {
        0 => 200,
        code => code,
    };
    let mut out = Vec::with_capacity(HEADER);
    out.extend([message.version, message.kind.byte()]);
    out.extend(message.id.max(1).to_be_bytes());
    out.extend(status_code.to_be_bytes());
    out.extend([path_len, socket_len]);
    out.extend(message.heartbeat.interval.to_be_bytes());
    out.extend(message.heartbeat.timeout.to_be_bytes());
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(message.socket.as_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}
