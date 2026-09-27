//! The verifier side: the getStatus response codec, the reply check and the verifying preimage.

use super::json;
use crate::codecs::Amount;
use crate::errors::{DecodeError, ReplyRefused};
use crate::js;
use crate::messages::Proto;
use crate::messages::peer::{Config, GetStatusResponse, Network, Plugin, State, StatusHeader};

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

/// A getStatus reply that [`check_reply`] accepted, with the changes the check made to it: the
/// plugin names outside 4 to 64 UTF-16 code units removed and the generator's key lowercased.
///
/// Only [`check_reply`] makes one, so the verifying preimage ([`verifying_preimage`]) is always
/// built from a reply the reference would verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedStatus(ReceivedStatus);

impl CheckedStatus {
    /// The checked reply.
    pub fn status(&self) -> &ReceivedStatus {
        &self.0
    }

    /// The checked reply, taken out.
    pub fn into_status(self) -> ReceivedStatus {
        self.0
    }
}

impl std::ops::Deref for CheckedStatus {
    type Target = ReceivedStatus;

    fn deref(&self) -> &ReceivedStatus {
        &self.0
    }
}

/// The largest plugin count of a reply.
const MAX_PLUGINS: usize = 32;
/// The largest amount the block header schema accepts, except the totals at height 1.
const MAX_AMOUNT: i128 = i64::MAX as i128;

/// The getStatus response codec's decoder.
pub fn decode_response(bytes: &[u8]) -> Result<ReceivedStatus, DecodeError> {
    let mut message = GetStatusResponse::decode(bytes)?;
    let header = message
        .state
        .as_mut()
        .ok_or(DecodeError::MissingState)?
        .header
        .as_mut()
        .ok_or(DecodeError::MissingHeader)?;
    let parsed = [&header.total_amount, &header.total_fee, &header.reward]
        .map(|text| Amount::parse(text.as_deref().unwrap_or_default()));
    // The reference reads the three in order and stops at the first syntax error; an amount
    // beyond 128 bits is refused only here, so a later syntax error still decides the class.
    if parsed.contains(&Err(DecodeError::AmountSyntax)) {
        return Err(DecodeError::AmountSyntax);
    }
    let [total_amount, total_fee, reward] = parsed;
    let (total_amount, total_fee, reward) = (total_amount?, total_fee?, reward?);
    // The spread sets the three keys: a key the header has keeps its position, a new one goes last.
    for (number, text) in [
        (9, &mut header.total_amount),
        (10, &mut header.total_fee),
        (11, &mut header.reward),
    ] {
        text.get_or_insert_default();
        header.wire_order.note(number);
    }
    Ok(ReceivedStatus {
        message,
        total_amount,
        total_fee,
        reward,
    })
}

/// The getStatus response codec's encoder: the header's amounts as decimal text, then the
/// message.
pub fn encode_response(status: &ReceivedStatus) -> Vec<u8> {
    let mut message = status.message.clone();
    if let Some(header) = message.state.as_mut().and_then(|s| s.header.as_mut()) {
        header.total_amount = Some(status.total_amount.to_string());
        header.total_fee = Some(status.total_fee.to_string());
        header.reward = Some(status.reward.to_string());
    }
    message.encode()
}

/// The reference's reply schema for getStatus, with the changes it makes to the reply: plugin
/// names outside 4 to 64 UTF-16 code units (or with a line terminator) are deleted after the
/// entry count is checked, and the generator's public key is lowercased. `max_delegates` bounds
/// the public keys and signatures: the largest number of active delegates of the milestones.
///
/// The rules run in the reference's order, and the error names the part of the first rule that
/// fails: the schema library checks a required key that has a schema of its own where it checks
/// that key's schema, so the keys `state`, `config`, `publicKeys` and `signatures` are checked in
/// this order, each with its presence.
pub fn check_reply(
    mut status: ReceivedStatus,
    max_delegates: usize,
) -> Result<CheckedStatus, ReplyRefused> {
    let amounts = [status.total_amount, status.total_fee, status.reward];
    let message = &mut status.message;
    let state = message.state.as_mut().ok_or(ReplyRefused::Root)?;
    // `height < Some(1)` holds for an absent height too.
    if state.forging_allowed.is_none() || state.current_slot.is_none() || state.height < Some(1) {
        return Err(ReplyRefused::State);
    }
    let header = state.header.as_mut().ok_or(ReplyRefused::State)?;
    if !header_passes(header, amounts) {
        return Err(ReplyRefused::Header);
    }
    if let Some(key) = header.generator_public_key.as_mut() {
        key.make_ascii_lowercase();
    }
    // The decoded config is an object of the protobuf library: an absent field reads as its
    // default, which satisfies `required`, and the length and type rules then decide.
    let config = message.config.as_mut().ok_or(ReplyRefused::Root)?;
    if !(5..=24).contains(&code_points(&config.version)) {
        return Err(ReplyRefused::Config);
    }
    if !config.network.as_ref().is_some_and(network_passes) {
        return Err(ReplyRefused::Network);
    }
    if config.plugins.len() > MAX_PLUGINS {
        return Err(ReplyRefused::Plugins);
    }
    config.plugins.retain(|name, _| {
        (4..=64).contains(&js::utf16_len(name))
            && !name.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
    });
    // An entry without a value is `null`, which is not an object.
    let passes = |plugin: Option<&Plugin>| plugin.is_some_and(|p| p.port.unwrap_or(0) <= 65_535);
    if !config.plugins.iter().all(|(_, plugin)| passes(plugin)) {
        return Err(ReplyRefused::Plugins);
    }
    let listed = |items: &[String], length: usize| {
        items.len() <= max_delegates && items.iter().all(|item| is_hex(item, Some(length)))
    };
    if !listed(&message.public_keys, 66) {
        return Err(ReplyRefused::PublicKeys);
    }
    if !listed(&message.signatures, 128) {
        return Err(ReplyRefused::Signatures);
    }
    Ok(CheckedStatus(status))
}

/// The block header schema: the required keys, the block ids (at height 1 the previous block may
/// be empty or read as the number 0), the amounts from 0 to 2^63 − 1 (the totals of height 1
/// unbounded), and the hex keys, hash and signature. The getBlocks reply check reads its blocks'
/// headers with it too.
pub(crate) fn header_passes(header: &StatusHeader, amounts: [Amount; 3]) -> bool {
    let genesis = header.height == Some(1);
    let [total_amount, total_fee, reward] = amounts.map(Amount::value);
    let total = |value: i128| value >= 0 && (genesis || value <= MAX_AMOUNT);
    header.timestamp.is_some()
        && header.height >= Some(1)
        && header.id.as_deref().is_some_and(|id| is_hex(id, Some(64)))
        && header.previous_block.as_deref().is_some_and(|previous| {
            (genesis && reads_as_zero(previous)) || is_hex(previous, Some(64))
        })
        && total(total_amount)
        && total(total_fee)
        && (0..=MAX_AMOUNT).contains(&reward)
        && header
            .payload_hash
            .as_deref()
            .is_none_or(|hash| is_hex(hash, None))
        && header
            .generator_public_key
            .as_deref()
            .is_some_and(|key| is_hex(key, Some(66)))
        && header
            .block_signature
            .as_deref()
            .is_some_and(|signature| is_hex(signature, Some(128)))
}

/// The network and token rules; lengths are counted in code points, as the schema library does.
fn network_passes(network: &Network) -> bool {
    network.version.unwrap_or(0) <= 255
        && (1..=20).contains(&code_points(&network.name))
        && is_hex(network.nethash.as_deref().unwrap_or_default(), Some(64))
        && code_points(&network.explorer) <= 128
        && network.token.as_ref().is_some_and(|token| {
            (1..=8).contains(&code_points(&token.name))
                && (1..=4).contains(&code_points(&token.symbol))
        })
}

/// The length in code points of a text field; an absent field reads as the empty text.
fn code_points(text: &Option<String>) -> usize {
    text.as_deref().unwrap_or_default().chars().count()
}

/// At least one hex digit of either case and nothing else, `length` characters when given.
pub(crate) fn is_hex(text: &str, length: Option<usize>) -> bool {
    !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_hexdigit())
        && length.is_none_or(|length| text.len() == length)
}

/// `!text || Number(text) === 0`: empty or white space, a zero in any base, or a decimal literal
/// whose double value is 0.
fn reads_as_zero(text: &str) -> bool {
    let text = text.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}');
    js::parse_bigint(text) == Ok(0)
        || (text
            .bytes()
            .all(|b| b.is_ascii_digit() || b".+-eE".contains(&b))
            && text.parse::<f64>() == Ok(0.0))
}

/// The bytes the verifier checks the signatures against: the canonical JSON of `{state, config}`
/// of the checked reply, with the configuration as its own fields only (an empty `plugins` left
/// out). When the verifier's own slot is negative and the reply's slot is not 0, the slot is read
/// as a signed 32-bit value (`currentSlot >> 0`).
///
/// The reference verifies the reply as its check changed it (plugin names removed, the generator
/// key lowercased), which is why this takes a [`CheckedStatus`].
pub fn verifying_preimage(status: &CheckedStatus, local_slot_negative: bool) -> Vec<u8> {
    let status = status.status();
    let (empty_state, empty_config) = (State::default(), Config::default());
    let state = status.message.state.as_ref().unwrap_or(&empty_state);
    let config = status.message.config.as_ref().unwrap_or(&empty_config);
    let slot = state.current_slot.map(|slot| {
        if local_slot_negative && slot != 0 {
            // ToInt32: the same 32 bits, read as signed.
            i64::from(slot as i32)
        } else {
            i64::from(slot)
        }
    });
    let amounts = [status.total_amount, status.total_fee, status.reward];
    json::preimage(state, slot, amounts, config, false)
}
