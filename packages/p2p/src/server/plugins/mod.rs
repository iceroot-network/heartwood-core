//! The plugin chain, in the reference's order. Before the payload is read: the forger allowlist
//! on internal routes (403 "IP unauthorised on internal route"), the rate limit, AwaitBlock on
//! public routes. After: the codec's decode, the version gate and the route's schema, a known
//! peer's version recorded, readiness (499 "App is not ready" before the blockchain starts).
//! Before the handler: every public route tries to accept the caller as a peer at its headers'
//! port or the node's. Then the handler, and before the response: `connection: close` on a
//! refusal to a caller that is not the forger, and the ban of a refusal below 499.

use std::net::IpAddr;
use std::sync::Arc;

use heartwood_p2p_wire::RouteId;

use super::ServerOptions;
use super::seams::{RouteHandlers, ServerRefusal, SessionId};
use crate::node_services::{NodeServices, PeerSide};

/// A request's answer, and whether the socket closes after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The response's payload, or the refusal.
    pub result: Result<Vec<u8>, ServerRefusal>,
    /// `connection: close`: the socket ends after the answer.
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
}

/// The plugin chain around the handlers.
pub struct Plugins {
    parts: PluginParts,
}

impl Plugins {
    /// The chain of `parts`.
    pub fn new(parts: PluginParts) -> Plugins {
        Plugins { parts }
    }

    /// A request on `route` with `payload` from `ip` on `session`, through every plugin and the
    /// handler.
    pub async fn handle(
        &self,
        route: RouteId,
        payload: Vec<u8>,
        ip: IpAddr,
        session: SessionId,
    ) -> Answer {
        let _ = (&self.parts, route, payload, ip, session);
        todo!("the plugin chain")
    }
}
