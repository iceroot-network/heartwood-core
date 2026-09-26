//! Assets of type group 1: transfer, second-signature registration, delegate registration and
//! delegate resignation.

#![expect(
    unused_variables,
    dead_code,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::{DecodeError, EncodeError};
use crate::identities::PublicKeyBytes;
use crate::managers::Params;
use crate::transactions::types::{Asset, ResignationType, TransferItem};
use crate::utils::bytes::{Reader, Writer};

/// Write a transfer asset: the item count (`u16`), then per item the amount (`u64`) and the
/// 21-byte recipient. A recipient of another network than `params` makes the id fail, as in the
/// reference.
pub(crate) fn write_transfer(
    transfers: &[TransferItem],
    params: &Params,
    out: &mut Writer,
) -> Result<(), EncodeError> {
    todo!()
}

/// Read a transfer asset.
pub(crate) fn read_transfer(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    todo!()
}

/// Write a second-signature registration asset: the 33-byte key.
pub(crate) fn write_second_signature(public_key: &PublicKeyBytes, out: &mut Writer) {
    todo!()
}

/// Read a second-signature registration asset.
pub(crate) fn read_second_signature(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    todo!()
}

/// Write a delegate registration asset: the name length (`u8`), then the name.
pub(crate) fn write_delegate_registration(
    username: &str,
    out: &mut Writer,
) -> Result<(), EncodeError> {
    todo!()
}

/// Read a delegate registration asset.
pub(crate) fn read_delegate_registration(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    todo!()
}

/// Write a delegate resignation asset: no byte for a temporary resignation, else `0xff − type`.
pub(crate) fn write_delegate_resignation(
    resignation_type: Option<ResignationType>,
    out: &mut Writer,
) {
    todo!()
}

/// Read a delegate resignation asset, with the reference's rule for telling a resignation byte
/// from the signatures that follow.
pub(crate) fn read_delegate_resignation(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    todo!()
}
