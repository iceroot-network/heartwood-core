//! The internal controller: the forger's routes, which only the forger's addresses reach, and the
//! two attestation routes, which the reference does not have.

use super::ControllerParts;
use crate::server::seams::{ServerRefusal, ServerRequest};

/// emitEvent: the four forger events dispatched; any other name a graceful error with a warning, never a ban.
pub async fn emit_event(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("emitEvent")
}

/// getCurrentRound: the round's delegates, the current and next forger, the reward and the time, as the reference answers them.
pub async fn get_current_round(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getCurrentRound")
}

/// getSlotNumber: the slot of the given timestamp, or of now, with the block time at the tip plus one.
pub async fn get_slot_number(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getSlotNumber")
}

/// getNetworkState: a forced fast cleanse of every peer without common blocks, then the network state's analysis.
pub async fn get_network_state(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getNetworkState")
}

/// syncBlockchain: "Blockchain sync check WAKEUP requested by forger" at debug, then the blockchain's wake-up.
pub async fn sync_blockchain(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("syncBlockchain")
}

/// getStatusToSign: the forger's long poll for a status to attest.
pub async fn get_status_to_sign(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getStatusToSign")
}

/// postStatusSignatures: the forger's attestations of an offered status.
pub async fn post_status_signatures(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("postStatusSignatures")
}
