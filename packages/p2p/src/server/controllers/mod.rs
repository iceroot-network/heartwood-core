//! The route handlers, as the reference's controllers answer the peer routes and the forger's
//! internal routes: [`peer`] (getPeers, getCommonBlocks, getStatus), [`blocks`] (getBlocks,
//! postBlock), [`transactions`] (getUnconfirmedTransactions, postTransactions) and [`internal`]
//! (the forger's routes, the attestation routes among them). Each decodes its request with the
//! route's codec and encodes its answer with it.

pub mod blocks;
pub mod internal;
pub mod peer;
pub mod transactions;

use std::sync::Arc;

use futures_util::future::BoxFuture;
use heartwood_kernel::Clock;
use heartwood_p2p_wire::RouteId;

use super::seams::{RouteHandlers, ServerRefusal, ServerRequest};
use crate::attestation_session::AttestationSession;
use crate::node_services::{NodeServices, PeerSide};
use crate::options::P2pOptions;
use crate::status_server::StatusServer;

/// What the controllers read and call.
pub struct ControllerParts {
    /// The node.
    pub node: Arc<dyn NodeServices>,
    /// The peer side.
    pub peers: Arc<dyn PeerSide>,
    /// The forger's attestation session.
    pub attestation: Arc<AttestationSession>,
    /// The served status.
    pub status: Arc<StatusServer>,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
    /// The p2p options: the node's port, `minimumDelegateReach`.
    pub options: Arc<P2pOptions>,
}

/// The controllers, as the server's route handlers.
pub struct Controllers {
    parts: ControllerParts,
}

impl Controllers {
    /// The controllers of `parts`.
    pub fn new(parts: ControllerParts) -> Controllers {
        Controllers { parts }
    }
}

impl RouteHandlers for Controllers {
    fn handle(
        &self,
        route: RouteId,
        request: ServerRequest,
    ) -> BoxFuture<'_, Result<Vec<u8>, ServerRefusal>> {
        Box::pin(async move {
            let (p, r) = (&self.parts, &request);
            match route {
                RouteId::GetPeers => peer::get_peers(p, r).await,
                RouteId::GetCommonBlocks => peer::get_common_blocks(p, r).await,
                RouteId::GetStatus => peer::get_status(p, r).await,
                RouteId::GetBlocks => blocks::get_blocks(p, r).await,
                RouteId::PostBlock => blocks::post_block(p, r).await,
                RouteId::GetUnconfirmedTransactions => transactions::get_unconfirmed(p, r).await,
                RouteId::PostTransactions => transactions::post_transactions(p, r).await,
                RouteId::EmitEvent => internal::emit_event(p, r).await,
                RouteId::GetCurrentRound => internal::get_current_round(p, r).await,
                RouteId::GetSlotNumber => internal::get_slot_number(p, r).await,
                RouteId::GetNetworkState => internal::get_network_state(p, r).await,
                RouteId::SyncBlockchain => internal::sync_blockchain(p, r).await,
                RouteId::GetStatusToSign => internal::get_status_to_sign(p, r).await,
                RouteId::PostStatusSignatures => internal::post_status_signatures(p, r).await,
            }
        })
    }
}
