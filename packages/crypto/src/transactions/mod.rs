//! Version 3 transactions with the standard header.
//!
//! Six types are carried: transfer (1/6), second-signature registration (1/1), delegate
//! registration (1/2), delegate resignation (1/7), burn (2/0) and vote (2/2). The other types that
//! the reference registers are refused at decode.
//!
//! - [`types`]: [`Transaction`] and its fields; the per-type assets in `types::core` and
//!   `types::group2`.
//! - [`codec`]: the common part of the wire format (header, memo, signatures).
//! - [`serialiser`] and [`deserialiser`]: whole transactions to and from bytes.
//! - [`factory`]: [`Transaction::decode`], [`Transaction::decode_trusted`] and
//!   [`Transaction::from_json`].
//! - [`signer`] and [`verifier`]: signatures and the transaction rules.
//! - [`registry`]: [`TransactionType`] and the wire type numbers.
//! - [`builders`]: building and signing new transactions.

pub mod builders;
pub mod codec;
pub mod deserialiser;
pub mod factory;
pub mod registry;
pub mod serialiser;
pub mod signer;
#[cfg(test)]
pub(crate) mod testing;
pub mod types;
pub mod verifier;

pub use self::builders::TransactionBuilder;
pub use self::registry::TransactionType;
pub use self::types::{Asset, Memo, Transaction, TransactionData, TransactionId};
