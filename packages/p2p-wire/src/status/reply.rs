//! The verifier side: the getStatus response codec, the reply check and the verifying preimage.

use crate::codecs::Amount;
use crate::errors::{DecodeError, ReplyRefused};
use crate::messages::peer::GetStatusResponse;

/// A decoded getStatus response: `{...decoded, state: {...state, header: {...header, totalAmount,
/// totalFee, reward}}}`.
///
/// `message.state` and its header are always present. The header's amount fields hold the text
/// as it came; the three amounts are read from it (an absent amount is the empty text, which is
/// 0), and the header's wire order lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedStatus {
    /// The message, with `state` and its header present.
    pub message: GetStatusResponse,
    /// The header's sum of amounts.
    pub total_amount: Amount,
    /// The header's sum of fees.
    pub total_fee: Amount,
    /// The header's reward.
    pub reward: Amount,
}

/// The getStatus response codec's decoder.
pub fn decode_response(bytes: &[u8]) -> Result<ReceivedStatus, DecodeError> {
    let _ = bytes;
    todo!()
}

/// The getStatus response codec's encoder: the header's amounts as decimal text, then the
/// message.
pub fn encode_response(status: &ReceivedStatus) -> Vec<u8> {
    let _ = status;
    todo!()
}

/// The reference's reply schema for getStatus, with the changes it makes to the reply: plugin
/// names outside 4 to 64 UTF-16 code units (or with a line terminator) are deleted after the
/// entry count is checked, and the generator's public key is lowercased. `max_delegates` bounds
/// the public keys and signatures: the largest number of active delegates of the milestones.
pub fn check_reply(status: &mut ReceivedStatus, max_delegates: usize) -> Result<(), ReplyRefused> {
    let _ = (status, max_delegates);
    todo!()
}

/// The bytes the verifier checks the signatures against: the canonical JSON of `{state, config}`
/// of the checked reply, with the configuration as its own fields only (an empty `plugins` left
/// out). When the verifier's own slot is negative and the reply's slot is not 0, the slot is read
/// as a signed 32-bit value (`currentSlot >> 0`).
pub fn verifying_preimage(status: &ReceivedStatus, local_slot_negative: bool) -> Vec<u8> {
    let _ = (status, local_slot_negative);
    todo!()
}
