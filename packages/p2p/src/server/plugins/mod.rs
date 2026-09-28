//! The plugin chain, in the reference's order. Before the payload is read: the forger allowlist
//! on internal routes (403 "IP unauthorised on internal route"), the rate limit (429 "Rate limit
//! exceeded", the forger's addresses exempt), AwaitBlock on public routes; then the payload's
//! limit (413). After: a public route marks its socket as having made an endpoint request, then
//! the codec's decode, the version gate and the route's schema, a known peer's version recorded,
//! readiness (499 "App is not ready" before the blockchain starts). Before the handler: every
//! public route tries to accept the caller as a peer at its headers' port or the node's. Then the
//! handler, and before the response: the ban of a refusal below 499 (a 404 as "Not a websocket
//! connection") and `connection: close` on a refusal to a caller that is not the forger.
//!
//! A request whose body the route's codec cannot decode is also a transport violation of its
//! caller (no honest client sends one); the forger's addresses are never recorded.

use std::net::IpAddr;
use std::sync::Arc;

use heartwood_kernel::Clock;
use heartwood_p2p_wire::RouteId;
use tokio::time::Instant;

use super::ServerOptions;
use super::await_block::await_block;
use super::ban_hammer::BanHammer;
use super::rate_limit::InboundLimiter;
use super::seams::{RouteHandlers, ServerRefusal, ServerRequest, SessionId};
use super::validation::{VersionGate, check_request, check_size};
use crate::node_services::{NodeServices, PeerSide};
use crate::scoring::Violation;

/// A request's answer, and whether the close-connection plugin marked it `connection: close`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The response's payload, or the refusal.
    pub result: Result<Vec<u8>, ServerRefusal>,
    /// `connection: close`: a refusal to a caller that is not the forger. Over nes it only
    /// changes the text the refusal carries on (see [`super::session::reply`]).
    pub close: bool,
}

/// What the plugin chain is made of.
pub struct PluginParts {
    /// The server's options.
    pub options: Arc<ServerOptions>,
    /// The node.
    pub node: Arc<dyn NodeServices>,
    /// The peer side.
    pub peers: Arc<dyn PeerSide>,
    /// The route handlers.
    pub handlers: Arc<dyn RouteHandlers>,
    /// The bans.
    pub ban_hammer: Arc<BanHammer>,
    /// The wall clock of the bans.
    pub clock: Arc<dyn Clock>,
}

/// The plugin chain around the handlers.
pub struct Plugins {
    parts: PluginParts,
    limiter: InboundLimiter,
}

impl Plugins {
    /// The chain of `parts`.
    pub fn new(parts: PluginParts) -> Plugins {
        let options = &parts.options;
        let limiter = InboundLimiter::new(options.rate_limit, options.rate_limit_post_transactions);
        Plugins { parts, limiter }
    }

    /// The server's options.
    pub fn options(&self) -> &ServerOptions {
        &self.parts.options
    }

    /// The peer side.
    pub fn peers(&self) -> &dyn PeerSide {
        self.parts.peers.as_ref()
    }

    /// Whether `ip` is one of the forger's addresses (`remoteAccess`).
    pub fn is_forger(&self, ip: IpAddr) -> bool {
        self.parts.options.remote_access.contains(&ip)
    }

    /// A request from `ip` on `session` to `route` (`None`: a path that names no route, which
    /// the reference's HTTP router answers 404) with `payload`, through every plugin and the
    /// handler.
    pub async fn handle(
        &self,
        route: Option<RouteId>,
        payload: Vec<u8>,
        ip: IpAddr,
        session: SessionId,
    ) -> Answer {
        let from_forger = self.is_forger(ip);
        let result = match route {
            Some(route) => self.chain(route, payload, ip, session, from_forger).await,
            None => Err(ServerRefusal::new(404, "Not Found")),
        };
        if let Err(refusal) = &result
            && refusal.status < 499
        {
            let reason = match refusal.status {
                404 => "Not a websocket connection",
                _ => refusal.message.as_str(),
            };
            self.parts
                .ban_hammer
                .ban(ip, reason, self.parts.clock.now_ms());
        }
        let close = !from_forger && result.is_err();
        Answer { result, close }
    }

    async fn chain(
        &self,
        route: RouteId,
        payload: Vec<u8>,
        ip: IpAddr,
        session: SessionId,
        from_forger: bool,
    ) -> Result<Vec<u8>, ServerRefusal> {
        let parts = &self.parts;
        let public = !route.is_internal();
        if !public && !from_forger {
            return Err(ServerRefusal::new(403, "IP unauthorised on internal route"));
        }
        if !from_forger && !self.limiter.consume(ip, route, Instant::now()) {
            return Err(ServerRefusal::new(429, "Rate limit exceeded"));
        }
        if public {
            await_block(parts.node.queue_state(), parts.options.await_block_bound).await;
        }
        check_size(route, payload.len())?;
        if public {
            parts.ban_hammer.established(ip, session);
        }
        let chain = parts.node.chain();
        let gate = VersionGate {
            milestone: chain.milestones().at(chain.last_block().height()),
            configured: &parts.options.minimum_versions,
            network_name: &chain.network().name,
        };
        let limit = parts.options.max_transactions_per_request;
        let checked = check_request(route, &payload, &gate, limit).inspect_err(|refusal| {
            if !from_forger && refusal.message.starts_with("Payload deserialising failed") {
                parts.peers.record_violation(ip, Violation::RequestDecode);
            }
        })?;
        if let Some(version) = checked.version.as_deref().filter(|_| public) {
            parts.peers.record_version(ip, version);
        }
        if !parts.node.is_booted() {
            return Err(ServerRefusal::not_ready());
        }
        if public {
            parts
                .peers
                .accept(ip, checked.port.unwrap_or(parts.options.port));
        }
        let request = ServerRequest {
            payload,
            ip,
            session,
            from_forger,
        };
        parts.handlers.handle(route, request).await
    }
}
