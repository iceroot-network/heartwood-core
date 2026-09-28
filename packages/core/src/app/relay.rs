//! The relay's peer-to-peer server in its composition root ([`crate::app`]): the server's options
//! from the p2p options, the node's services and the peer side ([`crate::node_services`]), the
//! forger's attestation session with the served status and the controllers, and the listener. A
//! listener that cannot bind stops the relay, as the reference ends its application when its
//! server cannot start; a graceful shutdown closes the forger's sessions first.

use std::sync::Arc;

use heartwood_p2p::node_services::{NodeServices, PeerSide};
use heartwood_p2p::{P2pServer, ServerOptions};

use crate::app::RelayError;

/// The P2P server of `options` over `node` and `peers`, its listener bound.
pub async fn serve_p2p(
    options: ServerOptions,
    node: Arc<dyn NodeServices>,
    peers: Arc<dyn PeerSide>,
) -> Result<P2pServer, RelayError> {
    let _ = (options, node, peers);
    todo!("the relay's P2P server")
}
