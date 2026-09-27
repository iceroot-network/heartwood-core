//! The WebSocket connection of a nes session: `ws://<ip>:<port>/`, binary frames only, no
//! compression. The client never sends a WebSocket ping, and a received ping or pong frame ends
//! the connection. A frame's declared length is checked against the connection's receive limit
//! before the frame is read; a frame above it closes the connection with code 1009.

use std::net::SocketAddr;
use std::time::Duration;

use crate::errors::SessionError;

/// What the transport received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// A binary frame's payload.
    Binary(Vec<u8>),
    /// A text frame, which the nes client refuses.
    Text,
    /// A WebSocket ping or pong frame, which ends the connection.
    Control,
    /// The connection closed, with the close code when there was one.
    Closed(Option<u16>),
}

/// An open WebSocket connection.
#[derive(Debug)]
pub struct Transport {
    #[expect(dead_code, reason = "the transport's methods read it")]
    address: SocketAddr,
}

impl Transport {
    /// Open `ws://<address>/` within `timeout`.
    pub async fn connect(
        address: SocketAddr,
        timeout: Duration,
    ) -> Result<Transport, SessionError> {
        let _ = (address, timeout);
        todo!("open the WebSocket")
    }

    /// Send `bytes` as one binary frame.
    pub async fn send(&mut self, bytes: Vec<u8>) -> Result<(), SessionError> {
        let _ = bytes;
        todo!("send a binary frame")
    }

    /// Receive the next frame, refusing one longer than `max_payload`.
    pub async fn receive(&mut self, max_payload: usize) -> Result<Received, SessionError> {
        let _ = max_payload;
        todo!("receive a frame")
    }

    /// Close the connection at once.
    pub fn terminate(&mut self) {
        todo!("close the connection")
    }
}
