//! The nes client: one session per connection, as the reference's `@solar-network/nes` client
//! speaks it over [`crate::transport`].
//!
//! - Hello: type 0, version 2, id 1, empty path, socket and payload. The server's reply sets the
//!   socket id and the heartbeat; the heartbeat timeout is its interval plus its timeout (15 s and
//!   5 s), and every received message restarts it.
//! - Requests: type post with the route id as the path and the next id; responses are matched by
//!   id in any order. The request timeout and the receive limit are connection-wide settings that
//!   each request sets before it sends, and a pending request keeps the timeout it was sent with.
//! - Server pings are answered with type 1 and the next id, untracked, at most one per second.
//! - `connect` times out after 5 s and allows one automatic reconnect after a close.
//!
//! The frames themselves are `heartwood_p2p_wire::nes`.

pub mod client;

pub use self::client::{ConnectOptions, ErrorHook, Session};
