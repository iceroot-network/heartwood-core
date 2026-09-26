//! Assets of type group 1: transfer, second-signature registration, delegate registration and
//! delegate resignation.

use crate::errors::{AddressError, DecodeError, EncodeError, SchemaError};
use crate::identities::{Address, PublicKeyBytes};
use crate::managers::Params;
use crate::transactions::codec::{self, MULTI_SIGNATURE_ENTRY_SIZE, PUBLIC_KEY_SIZE};
use crate::transactions::types::{Asset, ResignationType, TransferItem};
use crate::utils::bytes::{Reader, Writer};

/// Bytes of an address on the wire: the network byte and the 20-byte key hash.
const ADDRESS_SIZE: usize = 21;

/// Write a transfer asset: the item count (`u16`), then per item the amount (`u64`) and the
/// 21-byte recipient.
///
/// A recipient of another network than `params` is written as it is and returned: the reference
/// serialises and signs such a transfer, and only refuses to compute its id.
pub(crate) fn write_transfer(
    transfers: &[TransferItem],
    params: &Params,
    out: &mut Writer,
) -> Result<Option<AddressError>, EncodeError> {
    let count = u16::try_from(transfers.len()).map_err(|_| EncodeError::OutOfRange {
        field: "asset.transfers",
    })?;
    out.u16_le(count);
    let mut network_error = None;
    for item in transfers {
        out.u64_le(item.amount);
        let recipient = item.recipient;
        if recipient.network() != params.network() {
            network_error = Some(AddressError::WrongNetwork {
                expected: params.network(),
                actual: recipient.network(),
            });
        }
        out.bytes(recipient.as_bytes());
    }
    Ok(network_error)
}

/// Read a transfer asset.
pub(crate) fn read_transfer(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    let count = reader.u16_le()?;
    let mut transfers = Vec::with_capacity(usize::from(count).min(reader.remaining() / 29));
    for _ in 0..count {
        let amount = reader.u64_le()?;
        let recipient = Address::from_bytes(reader.array::<ADDRESS_SIZE>()?);
        transfers.push(TransferItem { amount, recipient });
    }
    Ok(Asset::Transfer { transfers })
}

/// Write a second-signature registration asset: the 33-byte key.
pub(crate) fn write_second_signature(public_key: &PublicKeyBytes, out: &mut Writer) {
    out.bytes(public_key.as_bytes());
}

/// Read a second-signature registration asset.
pub(crate) fn read_second_signature(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    let public_key = PublicKeyBytes::from_array(reader.array::<PUBLIC_KEY_SIZE>()?);
    Ok(Asset::SecondSignature { public_key })
}

/// Write a delegate registration asset: the name length in UTF-8 bytes (`u8`), then the name.
pub(crate) fn write_delegate_registration(
    username: &str,
    out: &mut Writer,
) -> Result<(), EncodeError> {
    let bytes = username.as_bytes();
    let length = u8::try_from(bytes.len()).map_err(|_| EncodeError::OutOfRange {
        field: "asset.delegate.username",
    })?;
    out.u8(length);
    out.bytes(bytes);
    Ok(())
}

/// Read a delegate registration asset. The name is read as the reference reads it, with every
/// ill-formed UTF-8 sequence replaced; the rules then refuse any name that is not a delegate name.
pub(crate) fn read_delegate_registration(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    let length = usize::from(reader.u8()?);
    let username = codec::lossy_utf8(reader.bytes(length)?);
    Ok(Asset::DelegateRegistration { username })
}

/// Write a delegate resignation asset: no byte for a temporary resignation, else `0xff − type`.
pub(crate) fn write_delegate_resignation(
    resignation_type: Option<ResignationType>,
    out: &mut Writer,
) {
    match resignation_type {
        None | Some(ResignationType::TemporaryResign) => {}
        Some(kind @ (ResignationType::PermanentResign | ResignationType::NotResigned)) => {
            out.u8(0xff - kind.number());
        }
    }
}

/// Read a delegate resignation asset, with the reference's rule for telling a resignation byte
/// from the signatures that follow: no byte is read when the bytes left are exactly one or two
/// signatures or a whole number (two or more) of multi-signature entries, and a byte of `0x0f` or
/// less is left for the signatures.
///
/// A byte for a type other than 0, 1 and 2 decodes in the reference and is then refused by the
/// rules; it comes back here as the second value, for the caller to report once the transaction
/// is otherwise read.
pub(crate) fn read_delegate_resignation(
    reader: &mut Reader<'_>,
) -> Result<(Asset, Option<SchemaError>), DecodeError> {
    let none = Asset::DelegateResignation {
        resignation_type: None,
    };
    let remaining = reader.remaining();
    if (remaining <= 128 && remaining.is_multiple_of(64))
        || (remaining >= 2 * MULTI_SIGNATURE_ENTRY_SIZE
            && remaining.is_multiple_of(MULTI_SIGNATURE_ENTRY_SIZE))
    {
        return Ok((none, None));
    }
    let byte = reader.peek_u8()?;
    if byte <= 0x0f {
        return Ok((none, None));
    }
    reader.u8()?;
    match ResignationType::from_number(0xff - byte) {
        Some(kind) => Ok((
            Asset::DelegateResignation {
                resignation_type: Some(kind),
            },
            None,
        )),
        None => Ok((
            none,
            Some(SchemaError::invalid(
                "asset.resignationType",
                "must be 0, 1 or 2",
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resignation(bytes: &[u8]) -> (Result<(Asset, Option<SchemaError>), DecodeError>, usize) {
        let mut reader = Reader::new(bytes);
        let result = read_delegate_resignation(&mut reader);
        (result, reader.offset())
    }

    fn kind(asset: &Asset) -> Option<ResignationType> {
        match asset {
            Asset::DelegateResignation { resignation_type } => *resignation_type,
            _ => None,
        }
    }

    #[test]
    fn resignation_byte_rule() {
        for length in [0, 64, 128, 130, 195] {
            let (result, offset) = resignation(&vec![0xfe; length]);
            let (asset, pending) = result.unwrap();
            assert_eq!((kind(&asset), pending, offset), (None, None, 0), "{length}");
        }
        let (result, offset) = resignation(&[0xfe; 65]);
        assert_eq!(
            kind(&result.unwrap().0),
            Some(ResignationType::PermanentResign)
        );
        assert_eq!(offset, 1);
        let (result, offset) = resignation(&[0xff; 65]);
        assert_eq!(
            kind(&result.unwrap().0),
            Some(ResignationType::TemporaryResign)
        );
        assert_eq!(offset, 1);
        let (result, offset) = resignation(&[0x0f; 65]);
        assert_eq!(kind(&result.unwrap().0), None);
        assert_eq!(offset, 0);
        let (result, offset) = resignation(&[0x10; 65]);
        assert!(result.unwrap().1.is_some());
        assert_eq!(offset, 1);
    }

    #[test]
    fn resignation_bytes_written() {
        let mut out = Writer::new();
        write_delegate_resignation(None, &mut out);
        write_delegate_resignation(Some(ResignationType::TemporaryResign), &mut out);
        write_delegate_resignation(Some(ResignationType::PermanentResign), &mut out);
        write_delegate_resignation(Some(ResignationType::NotResigned), &mut out);
        assert_eq!(out.into_vec(), [0xfe, 0xfd]);
    }

    #[test]
    fn username_length_is_bytes() {
        let mut out = Writer::new();
        write_delegate_registration("é", &mut out).unwrap();
        assert_eq!(out.into_vec(), [2, 0xc3, 0xa9]);
        assert_eq!(
            write_delegate_registration(&"a".repeat(256), &mut Writer::new()),
            Err(EncodeError::OutOfRange {
                field: "asset.delegate.username"
            })
        );
    }
}
