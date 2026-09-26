//! getPeers, getCommonBlocks and the getStatus request: the messages from the oracle's JSON, and
//! the decoded messages in its JSON. Each codec passes its message through as it is, except the
//! getPeers response, whose value is the list of peers.

use heartwood_p2p_wire::Codec;
use heartwood_p2p_wire::codecs::peer;
use serde_json::Value;

use super::messages::{Json, list_from_json, list_json, verdict};
use crate::subject::Verdict;
use crate::subjects::{Outcome, failed};

/// The payload of `value` for `codec`.
pub(super) fn encode(codec: Codec, value: &Value) -> Outcome<Vec<u8>> {
    Ok(match codec {
        Codec::GetPeersRequest => peer::get_peers_encode_request(&Json::from_json(value)?),
        Codec::GetPeersResponse => peer::get_peers_encode_response(&list_from_json(value)?),
        Codec::GetCommonBlocksRequest => {
            peer::get_common_blocks_encode_request(&Json::from_json(value)?)
        }
        Codec::GetCommonBlocksResponse => {
            peer::get_common_blocks_encode_response(&Json::from_json(value)?)
        }
        Codec::GetStatusRequest => peer::get_status_encode_request(&Json::from_json(value)?),
        other => return Err(failed(format!("{other:?} is not a peer codec"))),
    })
}

/// The verdict on `bytes` decoded with `codec`; its output is the decoded value.
pub(super) fn decode(codec: Codec, bytes: &[u8]) -> Outcome<Verdict> {
    match codec {
        Codec::GetPeersRequest => verdict(peer::get_peers_decode_request(bytes), Json::to_json),
        Codec::GetPeersResponse => verdict(peer::get_peers_decode_response(bytes), |peers| {
            list_json(peers)
        }),
        Codec::GetCommonBlocksRequest => {
            verdict(peer::get_common_blocks_decode_request(bytes), Json::to_json)
        }
        Codec::GetCommonBlocksResponse => verdict(
            peer::get_common_blocks_decode_response(bytes),
            Json::to_json,
        ),
        Codec::GetStatusRequest => verdict(peer::get_status_decode_request(bytes), Json::to_json),
        other => Err(failed(format!("{other:?} is not a peer codec"))),
    }
}
