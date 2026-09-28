//! The internal controller: the forger's routes, which only the forger's addresses reach, and the
//! two attestation routes, which the reference does not have.
//!
//! Each reads its body with the route's JSON reader; a body the reader refuses (a field of
//! another type than the reference's forger writes) is a 400 "Validation failed".

use heartwood_kernel::{debug, warning};
use heartwood_p2p_wire::internal::{
    self, EmitEvent, NetworkStateRequest, SlotNumber, SlotNumberRequest, StatusSignatures,
    StatusToSignRequest,
};

use super::{ControllerParts, internal as failed, invalid};
use crate::monitor::CleanseOptions;
use crate::network_state::{AnalyzeInput, analyze};
use crate::node_services::{AttestedKeys, ForgerEvent};
use crate::server::seams::{ServerRefusal, ServerRequest};

/// The answer of the routes that answer `true`.
const TRUE: &[u8] = b"true";

/// emitEvent: the four forger events dispatched; any other name a graceful error with a warning,
/// never a ban.
pub async fn emit_event(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let emitted: EmitEvent = internal::decode(&request.payload).map_err(invalid)?;
    let name = emitted.event.clone();
    let Some(event) = ForgerEvent::from_emit(emitted) else {
        warning!(
            "The forger emitted the event {name}, which is not a forger event: not dispatched"
        );
        return Err(ServerRefusal::new(
            499,
            format!("{name} is not a forger event"),
        ));
    };
    parts.node.forger_event(event);
    Ok(TRUE.to_vec())
}

/// getCurrentRound: the round's delegates, the current and next forger, the reward and the time,
/// as the reference answers them.
pub async fn get_current_round(
    parts: &ControllerParts,
    _request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let round = (parts.node.current_round(parts.clock.now_ms())).map_err(failed)?;
    Ok(internal::encode(&round))
}

/// getSlotNumber: the slot of the given timestamp, or of now, with the block time at the tip plus
/// one.
pub async fn get_slot_number(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let asked: SlotNumberRequest = internal::decode(&request.payload).map_err(invalid)?;
    let slot = (parts.node).slot_number(asked.timestamp, parts.clock.now_ms());
    Ok(internal::encode(&SlotNumber(slot)))
}

/// getNetworkState: a forced fast cleanse of every peer without common blocks, then the network
/// state's analysis.
pub async fn get_network_state(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let asked: NetworkStateRequest = internal::decode(&request.payload).map_err(invalid)?;
    let cleanse = CleanseOptions {
        fast: true,
        force_ping: true,
        log: asked.log,
        peer_count: None,
        skip_common_blocks: true,
    };
    parts.peers.cleanse(cleanse).await;
    let (chain, peers) = (parts.node.chain(), parts.peers.peers());
    let attested = parts.attestation.attested();
    let active = parts.node.active_keys_next_round().map_err(failed)?;
    let analysis = analyze(&AnalyzeInput {
        chain: chain.as_ref(),
        peers: &peers,
        attested: &attested,
        active_next_round: &active,
        port: parts.options.port,
        now_ms: parts.clock.now_ms(),
        cold_start: parts.peers.is_cold_start(),
        minimum_delegate_reach: parts.options.minimum_delegate_reach,
    });
    if analysis.state.status == internal::NetworkStateStatus::ColdStart {
        parts.peers.complete_cold_start();
    }
    if !analysis.on_this_node.is_empty() {
        parts.peers.remove_keys(&analysis.on_this_node);
    }
    parts.node.network_state_served(request.ip, &analysis.state);
    Ok(internal::encode(&analysis.state))
}

/// syncBlockchain: "Blockchain sync check WAKEUP requested by forger" at debug, then the
/// blockchain's wake-up.
pub async fn sync_blockchain(
    parts: &ControllerParts,
    _request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    debug!("Blockchain sync check WAKEUP requested by forger :bed:");
    parts.node.force_wakeup();
    Ok(TRUE.to_vec())
}

/// getStatusToSign: the forger's long poll for a status to attest.
pub async fn get_status_to_sign(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let asked: StatusToSignRequest = internal::decode(&request.payload).map_err(invalid)?;
    let session = &parts.attestation;
    let answer = (session.status_to_sign(request.session, request.ip, asked)).await?;
    Ok(internal::encode(&answer))
}

/// postStatusSignatures: the forger's attestations of an offered status.
pub async fn post_status_signatures(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let signatures: StatusSignatures = internal::decode(&request.payload).map_err(invalid)?;
    parts
        .attestation
        .post_signatures(request.session, signatures)?;
    Ok(TRUE.to_vec())
}
