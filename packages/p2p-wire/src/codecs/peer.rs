//! The codecs of the peer routes: getPeers, getCommonBlocks, and the getStatus request. The
//! getStatus response is in [`crate::status`].

use crate::errors::DecodeError;
use crate::messages::Proto;
use crate::messages::peer::{
    GetCommonBlocksRequest, GetCommonBlocksResponse, GetPeersRequest, GetPeersResponse,
    GetStatusRequest, Peer,
};

/// The getPeers request: the message.
pub fn get_peers_encode_request(request: &GetPeersRequest) -> Vec<u8> {
    request.encode()
}

/// The getPeers request: the message as decoded.
pub fn get_peers_decode_request(bytes: &[u8]) -> Result<GetPeersRequest, DecodeError> {
    GetPeersRequest::decode(bytes)
}

/// The getPeers response: `{peers}`.
pub fn get_peers_encode_response(peers: &[Peer]) -> Vec<u8> {
    GetPeersResponse {
        peers: peers.to_vec(),
        ..GetPeersResponse::default()
    }
    .encode()
}

/// The getPeers response: the decoded message's `peers`.
pub fn get_peers_decode_response(bytes: &[u8]) -> Result<Vec<Peer>, DecodeError> {
    GetPeersResponse::decode(bytes).map(|response| response.peers)
}

/// The getCommonBlocks request: the message.
pub fn get_common_blocks_encode_request(request: &GetCommonBlocksRequest) -> Vec<u8> {
    request.encode()
}

/// The getCommonBlocks request: the message as decoded.
pub fn get_common_blocks_decode_request(
    bytes: &[u8],
) -> Result<GetCommonBlocksRequest, DecodeError> {
    GetCommonBlocksRequest::decode(bytes)
}

/// The getCommonBlocks response: the message.
pub fn get_common_blocks_encode_response(response: &GetCommonBlocksResponse) -> Vec<u8> {
    response.encode()
}

/// The getCommonBlocks response: the message as decoded.
pub fn get_common_blocks_decode_response(
    bytes: &[u8],
) -> Result<GetCommonBlocksResponse, DecodeError> {
    GetCommonBlocksResponse::decode(bytes)
}

/// The getStatus request: the message.
pub fn get_status_encode_request(request: &GetStatusRequest) -> Vec<u8> {
    request.encode()
}

/// The getStatus request: the message as decoded.
pub fn get_status_decode_request(bytes: &[u8]) -> Result<GetStatusRequest, DecodeError> {
    GetStatusRequest::decode(bytes)
}
