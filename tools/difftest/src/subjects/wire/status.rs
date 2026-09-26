//! The signed status: the getStatus response codec, `status.sign` (the relay's tree, preimage and
//! attestations over a block) and `status.verify` (decode, reply check, both preimages and the
//! signature checks of a received response).

use serde_json::Value;

use crate::subject::{Case, SubjectError, Verdict};
use crate::subjects::{Env, Outcome};

/// The getStatus response payload of `value`.
pub(super) fn encode_response(value: &Value) -> Outcome<Vec<u8>> {
    let _ = value;
    Err(SubjectError::Unsupported(
        "the status codec is not answered yet".into(),
    ))
}

/// The verdict on the getStatus response `bytes`; its output is the decoded value.
pub(super) fn decode_response(bytes: &[u8]) -> Outcome<Verdict> {
    let _ = bytes;
    Err(SubjectError::Unsupported(
        "the status codec is not answered yet".into(),
    ))
}

/// `status.sign`: `{preimage, hex}` for a block, a slot, a configuration and keys.
pub(super) fn sign(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(
        "the status attestation is not answered yet".into(),
    ))
}

/// `status.verify`: `{valid, value, preimage, preimageNegativeSlot, verified}` for a received
/// response.
pub(super) fn verify(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(
        "the status attestation is not answered yet".into(),
    ))
}
