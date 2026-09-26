//! getPeers, getCommonBlocks and the getStatus request: the messages from the oracle's JSON, and
//! the decoded messages in its JSON.

use heartwood_p2p_wire::Codec;
use serde_json::Value;

use crate::subject::{SubjectError, Verdict};
use crate::subjects::Outcome;

/// The payload of `value` for `codec`.
pub(super) fn encode(codec: Codec, value: &Value) -> Outcome<Vec<u8>> {
    let _ = (codec, value);
    Err(SubjectError::Unsupported(
        "the peer codecs are not answered yet".into(),
    ))
}

/// The verdict on `bytes` decoded with `codec`; its output is the decoded value.
pub(super) fn decode(codec: Codec, bytes: &[u8]) -> Outcome<Verdict> {
    let _ = (codec, bytes);
    Err(SubjectError::Unsupported(
        "the peer codecs are not answered yet".into(),
    ))
}
