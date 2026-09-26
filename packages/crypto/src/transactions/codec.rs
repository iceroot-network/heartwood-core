//! The common part of the version 3 wire format.
//!
//! Layout: header type (`0xff` for the standard header), version, network byte, type group
//! (`u32`), type (`u16`), nonce (`u64`), the 33-byte sender key, fee (`u64`), then the memo (length
//! `u8` and UTF-8 bytes), the type's asset, and the signatures. All integers are little-endian.
//!
//! After the asset, the reference reads a 64-byte signature while the bytes left are a nonzero
//! multiple of 64 or not a multiple of 65, then a second one by the same rule; any bytes left then
//! are an error. That rule is kept.

#![expect(
    unused_variables,
    dead_code,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::crypto::sig::Signature;
use crate::errors::DecodeError;
use crate::identities::PublicKey;
use crate::transactions::serialiser::SerialiseOptions;
use crate::transactions::types::{Memo, TransactionData};
use crate::utils::bytes::{Reader, Writer};

/// The version this crate writes.
pub const VERSION: u8 = 3;

/// The header type byte of the standard header (`0xff − 0`).
pub const STANDARD_HEADER: u8 = 0xff;

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

/// Write the common header of `data`.
pub(crate) fn write_header(data: &TransactionData, out: &mut Writer) {
    todo!()
}

/// Read the common header. The sender key must be one the reference accepts.
pub(crate) fn read_header(reader: &mut Reader<'_>) -> Result<Header, DecodeError> {
    todo!()
}

/// Write the memo: its length and bytes, or a single zero byte when there is none.
pub(crate) fn write_memo(memo: Option<&Memo>, out: &mut Writer) {
    todo!()
}

/// Read the memo. Bytes that are not canonical UTF-8 are refused.
pub(crate) fn read_memo(reader: &mut Reader<'_>) -> Result<Option<Memo>, DecodeError> {
    todo!()
}

/// Write the signatures of `data` that `options` keeps.
pub(crate) fn write_signatures(
    data: &TransactionData,
    options: SerialiseOptions,
    out: &mut Writer,
) {
    todo!()
}

/// Read the signatures with the reference's 64/65-byte rule; returns the first and second
/// signature.
pub(crate) fn read_signatures(
    reader: &mut Reader<'_>,
) -> Result<(Option<Signature>, Option<Signature>), DecodeError> {
    todo!()
}
