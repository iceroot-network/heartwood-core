//! Transactions to bytes, and the hashes that signatures and ids are computed over.

use crate::crypto::hash::sha256;
use crate::errors::{AddressError, EncodeError};
use crate::managers::Params;
use crate::transactions::codec;
use crate::transactions::types::{Asset, TransactionData, TransactionId, core, group2};
use crate::utils::bytes::Writer;

/// Which signatures a serialisation leaves out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SerialiseOptions {
    /// Leave out the sender's signature.
    pub exclude_signature: bool,
    /// Leave out the second signature.
    pub exclude_second_signature: bool,
}

impl SerialiseOptions {
    /// Every signature: the bytes an id is computed over.
    pub const FULL: SerialiseOptions = SerialiseOptions {
        exclude_signature: false,
        exclude_second_signature: false,
    };

    /// No signature: the bytes the sender signs.
    pub const UNSIGNED: SerialiseOptions = SerialiseOptions {
        exclude_signature: true,
        exclude_second_signature: true,
    };

    /// The sender's signature only: the bytes the second key signs.
    pub const FOR_SECOND_SIGNATURE: SerialiseOptions = SerialiseOptions {
        exclude_signature: false,
        exclude_second_signature: true,
    };
}

/// The size of the reference's serialiser buffer when the milestone has no block limits.
const DEFAULT_SIZE_LIMIT: usize = 83_886;

/// The serialiser's size limit at the height of `params`: `floor(maxPayload / maxTransactions) × 2`
/// bytes, or 83,886 when either value is missing or zero.
pub fn size_limit(params: &Params) -> usize {
    match (params.max_payload(), params.max_transactions()) {
        (Some(payload), Some(transactions)) if payload != 0 && transactions != 0 => {
            usize::try_from(payload / transactions)
                .unwrap_or(usize::MAX)
                .saturating_mul(2)
        }
        _ => DEFAULT_SIZE_LIMIT,
    }
}

/// The bytes of `data` with the signatures that `options` keeps.
///
/// A transfer to an address of another network serialises; only its id is refused (see
/// [`transaction_id`]).
pub fn serialise(
    data: &TransactionData,
    options: SerialiseOptions,
    params: &Params,
) -> Result<Vec<u8>, EncodeError> {
    serialise_parts(data, options, params).map(|(bytes, _)| bytes)
}

/// The SHA-256 of [`serialise`]: the hash that a signature signs.
pub fn signing_hash(
    data: &TransactionData,
    options: SerialiseOptions,
    params: &Params,
) -> Result<[u8; 32], EncodeError> {
    serialise(data, options, params).map(|bytes| sha256(&bytes))
}

/// The id of `data`: the SHA-256 of its full serialisation, signatures included. A transfer to an
/// address of another network has no id, as in the reference.
pub fn transaction_id(
    data: &TransactionData,
    params: &Params,
) -> Result<TransactionId, EncodeError> {
    let (bytes, network_error) = serialise_parts(data, SerialiseOptions::FULL, params)?;
    match network_error {
        Some(error) => Err(EncodeError::AddressNetwork(error)),
        None => Ok(TransactionId::from_array(sha256(&bytes))),
    }
}

/// The bytes of `data`, and the network error of a transfer recipient, if any.
fn serialise_parts(
    data: &TransactionData,
    options: SerialiseOptions,
    params: &Params,
) -> Result<(Vec<u8>, Option<AddressError>), EncodeError> {
    let limit = size_limit(params);
    let check = |size: usize| {
        if size > limit {
            Err(EncodeError::TooLarge { size, limit })
        } else {
            Ok(())
        }
    };
    let mut out = Writer::with_capacity(256);
    codec::write_header(data, &mut out)?;
    codec::write_memo(data.memo.as_ref(), &mut out);
    check(out.len())?;
    let mut network_error = None;
    match &data.asset {
        Asset::Transfer { transfers } => {
            network_error = core::write_transfer(transfers, params, &mut out)?;
        }
        Asset::SecondSignature { public_key } => core::write_second_signature(public_key, &mut out),
        Asset::DelegateRegistration { username } => {
            core::write_delegate_registration(username, &mut out)?;
        }
        Asset::DelegateResignation { resignation_type } => {
            core::write_delegate_resignation(*resignation_type, &mut out);
        }
        Asset::Burn { amount } => group2::write_burn(*amount, &mut out),
        Asset::Vote { votes } => group2::write_vote(votes, &mut out)?,
    }
    codec::write_signatures(data, options, &mut out);
    check(out.len())?;
    Ok((out.into_vec(), network_error))
}
