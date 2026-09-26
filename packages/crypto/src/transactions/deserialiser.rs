//! Bytes to transaction fields.

use crate::errors::{DecodeError, SchemaError};
use crate::transactions::TransactionType;
use crate::transactions::codec::{self, VERSION};
use crate::transactions::types::{TransactionData, core, solar};
use crate::utils::bytes::Reader;

/// The fields in `bytes`: header, memo, asset and signatures, with every byte consumed. Removed
/// and unknown types are refused here, and every version but 3 (or the wire byte 0, which reads
/// as 3).
pub fn deserialise(bytes: &[u8]) -> Result<TransactionData, DecodeError> {
    let Deserialised { data, pending } = deserialise_raw(bytes, true)?;
    match pending {
        Some(error) => Err(DecodeError::Schema(error)),
        None => Ok(data),
    }
}

/// Fields read from the wire, and a rule refusal found while reading that the reference only
/// reports once the id is computed (a resignation byte for an unknown type).
#[derive(Debug)]
pub(crate) struct Deserialised {
    pub(crate) data: TransactionData,
    pub(crate) pending: Option<SchemaError>,
}

/// Read `bytes` in the reference's order: header, type, memo, asset, signatures, then the version.
/// `check_version` is off only for bytes verified before.
pub(crate) fn deserialise_raw(
    bytes: &[u8],
    check_version: bool,
) -> Result<Deserialised, DecodeError> {
    let mut reader = Reader::new(bytes);
    let header = codec::read_header(&mut reader)?;
    let kind = TransactionType::from_wire(header.type_group, header.type_id)?;
    let memo = codec::read_memo(&mut reader)?;
    let mut pending = None;
    let asset = match kind {
        TransactionType::Transfer => core::read_transfer(&mut reader)?,
        TransactionType::SecondSignature => core::read_second_signature(&mut reader)?,
        TransactionType::DelegateRegistration => core::read_delegate_registration(&mut reader)?,
        TransactionType::DelegateResignation => {
            let (asset, refusal) = core::read_delegate_resignation(&mut reader)?;
            pending = refusal;
            asset
        }
        TransactionType::Burn => solar::read_burn(&mut reader)?,
        TransactionType::Vote => solar::read_vote(&mut reader)?,
    };
    let (signature, second_signature) = codec::read_signatures(&mut reader)?;
    if check_version && header.version != 0 && header.version != VERSION {
        return Err(DecodeError::UnsupportedVersion(header.version));
    }
    let data = TransactionData {
        version: header.version,
        network: header.network,
        kind,
        nonce: header.nonce,
        sender_public_key: header.sender_public_key,
        fee: header.fee,
        memo,
        asset,
        signature,
        second_signature,
    };
    Ok(Deserialised { data, pending })
}
