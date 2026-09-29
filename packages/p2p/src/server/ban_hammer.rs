//! The ban hammer: the bans by address and one socket per address.
//!
//! A banned address's new connection is destroyed until `banSeconds` pass. A new connection
//! destroys every other socket from its address. When a socket closes and its address is not
//! banned, a termination reason bans it with that reason; after the sync plus 60 s, a socket that
//! opened a WebSocket but made no endpoint request (no public route passed the checks before its
//! payload was read) bans "Did not make an endpoint request", and one that never completed the
//! handshake bans "Failed to complete websocket handshake". Every refusal below 499 bans with its
//! message. The forger's addresses are never banned, and nothing is with `banSeconds` 0. The first
//! ban of an address logs `Banning <ip> for <duration>. Reason: <reason>` at debug; a ban of an
//! address in the list, even one whose time has passed, renews it without a line.
//!
//! An expired ban leaves the list when the address connects again, as in the reference, and, where
//! the reference keeps it for good, when a new ban takes the list past [`SWEEP_AT`] entries (or
//! twice its size after the last sweep): then every expired ban leaves, so that addresses banned
//! once and never seen again cannot grow the list without a bound.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::seams::SessionId;
use super::session::SessionEnd;

/// How long after the node's first sync the socket-close bans apply.
pub const SYNC_GRACE_MS: i64 = 60_000;
/// The size of the ban list past which a new ban drops the expired ones.
pub const SWEEP_AT: usize = 10_000;

/// The bans and the sockets by address.
#[derive(Debug)]
pub struct BanHammer {
    ban_seconds: u32,
    ban_ms: i64,
    remote_access: Vec<IpAddr>,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// When each address was last banned, and the reason it was first banned with.
    bans: HashMap<IpAddr, (i64, String)>,
    /// The size of the list at which a new ban sweeps it (0 for [`SWEEP_AT`]).
    sweep_at: usize,
    /// Each address's sockets, and whether each made an endpoint request.
    sockets: HashMap<IpAddr, Vec<(SessionId, bool)>>,
}

impl BanHammer {
    /// A ban hammer banning for `ban_seconds`, never banning `remote_access`.
    pub fn new(ban_seconds: u32, remote_access: Vec<IpAddr>) -> BanHammer {
        let ban_ms = i64::from(ban_seconds).saturating_mul(1_000);
        let state = Mutex::default();
        BanHammer {
            ban_seconds,
            ban_ms,
            remote_access,
            state,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `ip` is banned at `now_ms`; a ban whose time has passed is lifted.
    pub fn is_banned(&self, ip: IpAddr, now_ms: i64) -> bool {
        let mut state = self.state();
        let Some((since, _)) = state.bans.get(&ip) else {
            return false;
        };
        if now_ms.saturating_sub(*since) >= self.ban_ms {
            state.bans.remove(&ip);
            return false;
        }
        true
    }

    /// Ban `ip` for `reason` from `now_ms`.
    pub fn ban(&self, ip: IpAddr, reason: &str, now_ms: i64) {
        if self.ban_ms == 0 || self.remote_access.contains(&ip) {
            return;
        }
        let first = {
            let mut state = self.state();
            let kept = state.bans.get(&ip).map(|(_, first)| first.clone());
            let first = kept.is_none();
            state
                .bans
                .insert(ip, (now_ms, kept.unwrap_or_else(|| reason.to_owned())));
            if state.bans.len() > state.sweep_at.max(SWEEP_AT) {
                let ban_ms = self.ban_ms;
                (state.bans).retain(|_, (since, _)| now_ms.saturating_sub(*since) < ban_ms);
                state.sweep_at = state.bans.len().saturating_mul(2);
            }
            first
        };
        if first {
            let duration = format_seconds(self.ban_seconds);
            heartwood_kernel::debug!(
                "Banning {ip} for {duration}. Reason: {reason} :oncoming_police_car:"
            );
        }
    }

    /// The reason `ip` was first banned with, while its ban is listed.
    pub fn reason(&self, ip: IpAddr) -> Option<String> {
        self.state().bans.get(&ip).map(|(_, reason)| reason.clone())
    }

    /// The socket of `session` from `ip` opened: the other sessions of the address, which the
    /// server destroys.
    pub fn opened(&self, ip: IpAddr, session: SessionId) -> Vec<SessionId> {
        let mut state = self.state();
        let sockets = state.sockets.entry(ip).or_default();
        let others = sockets.iter().map(|(other, _)| *other).collect();
        sockets.push((session, false));
        others
    }

    /// The socket of `session` from `ip` made an endpoint request. A socket that has closed
    /// already is not listed again (the reference lists it anew, and never removes it).
    pub fn established(&self, ip: IpAddr, session: SessionId) {
        let mut state = self.state();
        let sockets = state.sockets.get_mut(&ip).into_iter().flatten();
        if let Some((_, established)) = sockets.into_iter().find(|(other, _)| *other == session) {
            *established = true;
        }
    }

    /// The socket of `session` from `ip` closed with `end` at `now_ms`; `first_synced_ms` is when
    /// the node first reported itself synced, if it has.
    pub fn closed(
        &self,
        ip: IpAddr,
        session: SessionId,
        end: &SessionEnd,
        now_ms: i64,
        first_synced_ms: Option<i64>,
    ) {
        let (known, established, banned) = {
            let mut state = self.state();
            let banned = state.bans.contains_key(&ip);
            let Some(sockets) = state.sockets.get_mut(&ip) else {
                return;
            };
            let found = sockets.iter().position(|(other, _)| *other == session);
            let established = found.is_some_and(|at| sockets.remove(at).1);
            if sockets.is_empty() {
                state.sockets.remove(&ip);
            }
            (found.is_some(), established, banned)
        };
        if !known || banned {
            return;
        }
        let synced =
            first_synced_ms.is_some_and(|synced| now_ms >= synced.saturating_add(SYNC_GRACE_MS));
        let reason = match end {
            SessionEnd::Terminated { reason } => reason.as_str(),
            SessionEnd::Closed if synced && !established => "Did not make an endpoint request",
            SessionEnd::Handshake if synced => "Failed to complete websocket handshake",
            _ => return,
        };
        self.ban(ip, reason, now_ms);
    }
}

/// A duration as the reference's `formatSeconds` writes it: `10 seconds`, `1 minute`, `1 hour, 2
/// minutes and 3 seconds`.
pub fn format_seconds(seconds: u32) -> String {
    let units = [
        (86_400, "day"),
        (3_600, "hour"),
        (60, "minute"),
        (1, "second"),
    ];
    let mut rest = seconds;
    let mut parts = Vec::new();
    for (size, unit) in units {
        let count = rest.checked_div(size).unwrap_or_default();
        rest = rest.checked_rem(size).unwrap_or_default();
        if count > 0 {
            parts.push(format!(
                "{count} {unit}{}",
                if count == 1 { "" } else { "s" }
            ));
        }
    }
    match parts.split_last() {
        Some((last, first)) if !first.is_empty() => format!("{} and {last}", first.join(", ")),
        _ => parts.concat(),
    }
}
