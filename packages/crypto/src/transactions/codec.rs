//! The common part of the version 3 wire format.
//!
//! Layout: header type (`0xff` for the standard header), version, network byte, type group
//! (`u32`), type (`u16`), nonce (`u64`), the 33-byte sender key, fee (`u64`), then the memo (length
//! `u8` and UTF-8 bytes), the type's asset, and the signatures. All integers are little-endian.
//!
//! After the asset, the reference reads a 64-byte signature while the bytes left are a nonzero
//! multiple of 64 or not a multiple of 65, then a second one by the same rule. That rule is kept.
//! Bytes left after that are an error: 65-byte multi-signature entries, which the reference still
//! reads, are refused as [`DecodeError::MultiSignature`], and anything else as
//! [`DecodeError::SignatureBytes`].

use crate::crypto::sig::Signature;
use crate::errors::{DecodeError, EncodeError};
use crate::identities::PublicKey;
use crate::transactions::serialiser::SerialiseOptions;
use crate::transactions::types::{Memo, TransactionData};
use crate::utils::bytes::{Reader, Writer};
use crate::validation::MAX_AMOUNT;

/// The version this crate writes.
pub const VERSION: u8 = 3;

/// The header type byte of the standard header (`0xff − 0`).
pub const STANDARD_HEADER: u8 = 0xff;

/// Bytes of the standard header, up to and including the fee.
pub const HEADER_SIZE: usize = 58;

/// Bytes of a sender key on the wire.
pub const PUBLIC_KEY_SIZE: usize = 33;

/// Bytes of a signature.
pub const SIGNATURE_SIZE: usize = 64;

/// Bytes of a multi-signature entry: a participant index and a signature.
pub const MULTI_SIGNATURE_ENTRY_SIZE: usize = 65;

/// The fields of the common header, as read from the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) version: u8,
    pub(crate) network: u8,
    pub(crate) type_group: u32,
    pub(crate) type_id: u16,
    pub(crate) nonce: u64,
    pub(crate) sender_public_key: PublicKey,
    pub(crate) fee: u64,
}

/// Write the common header of `data`. The version byte 0 is written as 3 and the network byte 0 as
/// `network`, the configured network byte, as the reference does. The nonce and the fee are signed
/// 64-bit fields in the reference's writer, so values over `2^63 − 1` do not fit.
pub(crate) fn write_header(
    data: &TransactionData,
    network: u8,
    out: &mut Writer,
) -> Result<(), EncodeError> {
    let key = data.sender_public_key.as_bytes();
    if key.len() != PUBLIC_KEY_SIZE {
        return Err(EncodeError::OutOfRange {
            field: "senderPublicKey",
        });
    }
    out.u8(STANDARD_HEADER);
    out.u8(data.effective_version());
    out.u8(match data.network {
        0 => network,
        byte => byte,
    });
    out.u32_le(data.kind.type_group());
    out.u16_le(data.kind.type_id());
    out.u64_le(signed_field(data.nonce, "nonce")?);
    out.bytes(key);
    out.u64_le(signed_field(data.fee, "fee")?);
    Ok(())
}

/// `value`, if it fits the reference's signed 64-bit writer.
fn signed_field(value: u64, field: &'static str) -> Result<u64, EncodeError> {
    if value > MAX_AMOUNT {
        Err(EncodeError::OutOfRange { field })
    } else {
        Ok(value)
    }
}

/// Read the common header. The sender key must be one the reference accepts.
pub(crate) fn read_header(reader: &mut Reader<'_>) -> Result<Header, DecodeError> {
    let header_type = reader.u8()?;
    if header_type != STANDARD_HEADER {
        return Err(DecodeError::UnsupportedHeaderType(
            STANDARD_HEADER - header_type,
        ));
    }
    let version = reader.u8()?;
    let network = reader.u8()?;
    let type_group = reader.u32_le()?;
    let type_id = reader.u16_le()?;
    let nonce = reader.u64_le()?;
    let sender_public_key =
        PublicKey::from_bytes(reader.bytes(PUBLIC_KEY_SIZE)?).map_err(DecodeError::SenderKey)?;
    let fee = reader.u64_le()?;
    Ok(Header {
        version,
        network,
        type_group,
        type_id,
        nonce,
        sender_public_key,
        fee,
    })
}

/// Write the memo: its length and bytes, or a single zero byte when there is none.
pub(crate) fn write_memo(memo: Option<&Memo>, out: &mut Writer) {
    match memo {
        Some(memo) => {
            let bytes = memo.as_str().as_bytes();
            // A memo holds 1 to 255 bytes, so the length fits its byte.
            out.u8(u8::try_from(bytes.len()).unwrap_or(u8::MAX));
            out.bytes(bytes);
        }
        None => out.u8(0),
    }
}

/// Read the memo. Bytes that are not valid UTF-8 are refused.
pub(crate) fn read_memo(reader: &mut Reader<'_>) -> Result<Option<Memo>, DecodeError> {
    let length = usize::from(reader.u8()?);
    if length == 0 {
        return Ok(None);
    }
    let bytes = reader.bytes(length)?;
    let text = std::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidMemo)?;
    Ok(Some(Memo::from_wire(text)))
}

/// Write the signatures of `data` that `options` keeps.
pub(crate) fn write_signatures(
    data: &TransactionData,
    options: SerialiseOptions,
    out: &mut Writer,
) {
    if let Some(signature) = &data.signature
        && !options.exclude_signature
    {
        out.bytes(signature.as_bytes());
    }
    if let Some(signature) = &data.second_signature
        && !options.exclude_second_signature
    {
        out.bytes(signature.as_bytes());
    }
}

/// Read the signatures with the reference's 64/65-byte rule; returns the first and second
/// signature.
pub(crate) fn read_signatures(
    reader: &mut Reader<'_>,
) -> Result<(Option<Signature>, Option<Signature>), DecodeError> {
    let first = read_signature(reader)?;
    let second = read_signature(reader)?;
    let remaining = reader.remaining();
    if remaining == 0 {
        Ok((first, second))
    } else if remaining.is_multiple_of(MULTI_SIGNATURE_ENTRY_SIZE) {
        Err(DecodeError::MultiSignature)
    } else {
        Err(DecodeError::SignatureBytes)
    }
}

/// A 64-byte signature, if the bytes left are a nonzero multiple of 64 or not a multiple of 65.
fn read_signature(reader: &mut Reader<'_>) -> Result<Option<Signature>, DecodeError> {
    if !signature_follows(reader.remaining()) {
        return Ok(None);
    }
    Ok(Some(Signature::from_array(
        reader.array::<SIGNATURE_SIZE>()?,
    )))
}

/// The reference's test for a 64-byte signature at the cursor, given the bytes left.
pub(crate) fn signature_follows(remaining: usize) -> bool {
    remaining != 0
        && (remaining.is_multiple_of(SIGNATURE_SIZE)
            || !remaining.is_multiple_of(MULTI_SIGNATURE_ENTRY_SIZE))
}

/// `bytes` as the reference's `Buffer.toString("utf8")` reads them: every ill-formed sequence
/// becomes one U+FFFD, following the maximal-subpart rule both implementations use.
pub(crate) fn lossy_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::hex;

    #[test]
    fn lossy_utf8_matches_the_reference() {
        // Each case: bytes, and the text the reference's Buffer.toString("utf8") gives.
        let cases = [
            ("efbbbf61", "\u{feff}a"),
            ("ff", "\u{fffd}"),
            ("c080", "\u{fffd}\u{fffd}"),
            ("eda080", "\u{fffd}\u{fffd}\u{fffd}"),
            ("e298", "\u{fffd}"),
            ("f4908080", "\u{fffd}\u{fffd}\u{fffd}\u{fffd}"),
            ("f090", "\u{fffd}"),
            ("e0808061", "\u{fffd}\u{fffd}\u{fffd}a"),
            ("f880808080", "\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}"),
            ("e298e298", "\u{fffd}\u{fffd}"),
            ("ed9fbf", "\u{d7ff}"),
            ("f0908080e298", "\u{10000}\u{fffd}"),
        ];
        for (bytes, text) in cases {
            assert_eq!(lossy_utf8(&hex::decode(bytes).unwrap()), text, "{bytes}");
        }
    }

    #[test]
    fn signature_rule() {
        let reads = |remaining| signature_follows(remaining);
        assert!(!reads(0));
        assert!(reads(64));
        assert!(reads(128));
        assert!(reads(1));
        assert!(reads(66));
        assert!(!reads(65));
        assert!(!reads(130));
        assert!(reads(129));
        assert!(reads(192));
    }

    #[test]
    fn memo_bytes() {
        let decode = |bytes: &[u8]| read_memo(&mut Reader::new(bytes));
        assert_eq!(decode(&[0]), Ok(None));
        assert_eq!(decode(&[2, b'h', b'i']), Ok(Some(Memo::new("hi").unwrap())));
        assert_eq!(decode(&[2, 0xff, 0xfe]), Err(DecodeError::InvalidMemo));
        assert_eq!(decode(&[2, 0xc0, 0x80]), Err(DecodeError::InvalidMemo));
        assert!(matches!(decode(&[3, b'a']), Err(DecodeError::Truncated(_))));
        let mut out = Writer::new();
        write_memo(Some(&Memo::new("☃").unwrap()), &mut out);
        write_memo(None, &mut out);
        assert_eq!(out.into_vec(), [3, 0xe2, 0x98, 0x83, 0]);
    }
}
