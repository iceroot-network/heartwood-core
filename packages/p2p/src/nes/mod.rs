//! The nes client: one session per connection, as the reference's `@solar-network/nes` client
//! speaks it over [`crate::transport`].
//!
//! - Hello: type 0, version 2, the first id, empty path, socket and payload. The server's reply
//!   sets the heartbeat; the heartbeat timeout is its interval plus its timeout (15 s and 5 s),
//!   and every received message restarts it. When it expires, the session reports "Disconnecting
//!   due to heartbeat timeout" and closes.
//! - Requests: type post with the route id as the path and the next id; responses are matched by
//!   id in any order. The request timeout and the receive limit are connection-wide settings
//!   that each request sets before it is sent; a pending request keeps the timeout it was sent
//!   with, and every received message other than a ping resets the limit to the session's own
//!   (100 KiB for the connector's sessions, 20 MiB for a forger's).
//! - A response with status 400 or more fails its request with the first 512 bytes of its
//!   payload as text. A response to an unknown id, a text frame, a frame that does not parse, a
//!   type other than hello, request or ping, and two server pings less than 1 s apart are
//!   reported through the session's error hook, and the connector then terminates it. A response
//!   to a request that timed out is reported the same way, as a late response.
//! - A WebSocket ping or pong frame ends the connection for good, without a report.
//! - Server pings are answered with type 1 and the next id, untracked.
//! - The WebSocket opens within 5 s, and one automatic reconnect follows a close, after 1 s, in
//!   the session's whole life (the connector's options; a forger's link reconnects without
//!   limit).
//! - [`Session::is_ready`] tells whether the WebSocket is open, as the reference's `_isReady`
//!   does.
//!
//! The frames themselves are `heartwood_p2p_wire::nes`.

pub mod client;

pub use self::client::{ConnectOptions, DEFAULT_MAX_PAYLOAD, ErrorHook, Session};
