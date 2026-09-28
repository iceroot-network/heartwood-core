//! The ban hammer: the bans by address and one socket per address.
//!
//! A banned address's new connection is destroyed until `banSeconds` pass. A new connection
//! destroys every other socket from its address. When a socket closes and its address is not
//! banned, a termination reason bans it with that reason; after the sync plus 60 s, a socket that
//! opened a WebSocket but made no endpoint request bans "Did not make an endpoint request", and one
//! that never completed the handshake bans "Failed to complete websocket handshake". Every refusal
//! below 499 bans with its message. The forger's addresses are never banned. The first ban of an
//! address logs `Banning <ip> for <duration>. Reason: <reason>` at debug.

use std::net::IpAddr;

use super::seams::SessionId;
use super::session::SessionEnd;

/// The bans and the sockets by address.
pub struct BanHammer {}

impl BanHammer {
    /// A ban hammer banning for `ban_seconds`, never banning `remote_access`.
    pub fn new(ban_seconds: u32, remote_access: Vec<IpAddr>) -> BanHammer {
        let _ = (ban_seconds, remote_access);
        todo!("the ban hammer")
    }

    /// Whether `ip` is banned at `now_ms`.
    pub fn is_banned(&self, ip: IpAddr, now_ms: i64) -> bool {
        let _ = (ip, now_ms);
        todo!("the ban list")
    }

    /// Ban `ip` for `reason` from `now_ms`.
    pub fn ban(&self, ip: IpAddr, reason: &str, now_ms: i64) {
        let _ = (ip, reason, now_ms);
        todo!("a ban")
    }

    /// The socket of `session` from `ip` opened: the other sessions of the address, which the
    /// server destroys.
    pub fn opened(&self, ip: IpAddr, session: SessionId) -> Vec<SessionId> {
        let _ = (ip, session);
        todo!("one socket per address")
    }

    /// The socket of `session` from `ip` closed with `end` at `now_ms`; `synced_since_ms` is when
    /// the node was last synced, if it is.
    pub fn closed(
        &self,
        ip: IpAddr,
        session: SessionId,
        end: &SessionEnd,
        now_ms: i64,
        synced_since_ms: Option<i64>,
    ) {
        let _ = (ip, session, end, now_ms, synced_since_ms);
        todo!("the socket-close bans")
    }
}
