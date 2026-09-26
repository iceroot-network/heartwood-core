//! nes frames: the binary framing of the reference's WebSocket protocol on the peer-to-peer port.
//!
//! A frame is a 14-byte header (version, type, id, status code, path length, socket length,
//! heartbeat interval and timeout), then the path, the socket id and the payload. The reference
//! parses frames without the extended types: only hello, ping, post and error are accepted. Its
//! parser rewrites a non-empty path with the WHATWG URL parser (`/` + path, relative to
//! `http://127.0.0.1`) and decodes the path and the socket id as lossy UTF-8.
//!
//! The sessions (hello and version check, heartbeats, request ids) are not part of the codec.

mod frame;
mod path;

use crate::errors::FrameError;
use crate::routes::RouteId;

/// The kind of a frame: the frame types the peer-to-peer port accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Type byte 0: the client's hello and the server's reply to it.
    Hello,
    /// Type byte 1: the server's heartbeat and the client's reply.
    Ping,
    /// Type byte 3 (post): a request, or the response to one.
    Request,
    /// Type byte 9: an error.
    Error,
}

/// The heartbeat of a server's hello reply, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Heartbeat {
    /// How often the server sends a ping.
    pub interval: u16,
    /// How long the server waits for the reply to a ping.
    pub timeout: u16,
}

/// A nes frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The kind.
    pub kind: Kind,
    /// The version byte: 2 in the client's hello, 0 elsewhere. The reference parses it as its
    /// decimal text.
    pub version: u8,
    /// The request id. [`Message::encode`] writes 1 for 0, as the reference's writer does.
    pub id: u32,
    /// The status code. [`Message::encode`] writes 200 for 0.
    pub status_code: u16,
    /// The path: a route id in a request, empty elsewhere. [`Message::parse`] returns it in the
    /// reference's normalised form (`/p2p.peer.getStatus`).
    pub path: String,
    /// The socket id, in the server's hello reply only.
    pub socket: String,
    /// The heartbeat, in the server's hello reply only (15000 and 5000); zero elsewhere.
    pub heartbeat: Heartbeat,
    /// The payload: the route codec's bytes in a request or response.
    pub payload: Vec<u8>,
}

impl Message {
    /// Parse a frame as the reference's parser does on the peer-to-peer port.
    ///
    /// Beyond the header checks: the path is normalised; when the type byte is not post and the
    /// path length is exactly 255, the payload's text is appended to the path and the payload is
    /// empty.
    pub fn parse(bytes: &[u8]) -> Result<Message, FrameError> {
        frame::parse(bytes)
    }

    /// Write the frame as the reference's writer does: a path of a frame that is not a request is
    /// cut at 255 UTF-16 code units, and the rest becomes the payload's text; id 0 is written as 1
    /// and status code 0 as 200.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        frame::encode(self)
    }

    /// The client's hello: version 2, id `id`, status 200, no path, socket or payload.
    pub fn client_hello(id: u32) -> Message {
        let _ = id;
        todo!()
    }

    /// A request for `route`: version 0, id `id`, status 200, the route id as the path, and the
    /// route codec's bytes as the payload.
    pub fn request(id: u32, route: RouteId, payload: Vec<u8>) -> Message {
        let _ = (id, route, payload);
        todo!()
    }

    /// The response to request `id`: version 0, status 200, no path, the route codec's bytes as
    /// the payload.
    pub fn response(id: u32, payload: Vec<u8>) -> Message {
        let _ = (id, payload);
        todo!()
    }

    /// A ping with id `id`: the server's heartbeat, or the client's reply to it.
    pub fn ping(id: u32) -> Message {
        let _ = id;
        todo!()
    }

    /// The server's reply to a hello with id `id`: its heartbeat and its socket id.
    pub fn server_hello(id: u32, heartbeat: Heartbeat, socket: String) -> Message {
        let _ = (id, heartbeat, socket);
        todo!()
    }

    /// A graceful error answering request `id`: status 499 and the JSON `body` as the payload.
    pub fn graceful_error(id: u32, body: &[u8]) -> Message {
        let _ = (id, body);
        todo!()
    }
}
