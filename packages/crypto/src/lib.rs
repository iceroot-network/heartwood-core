//! `heartwood-crypto`: the byte-exact layer of Heartwood Core.
//!
//! This crate holds everything whose bytes or verdicts must equal the reference implementation:
//! hashes, keys, addresses and signatures, the transaction and block codecs, the milestone rules
//! and the arithmetic that blocks carry (rewards, donations, the fee burn). The module names follow
//! the reference's file names.
//!
//! - [`crypto`]: hashes, the signature schemes behind [`crypto::sig`], and slot arithmetic.
//! - [`identities`]: secret keys, public keys and addresses.
//! - [`transactions`]: the version 3 transaction codec, signing, verification and builders.
//! - [`blocks`]: the block codec, block ids, verification and the block factory.
//! - [`managers`]: the network description and the milestones, merged and checked at load.
//! - [`validation`]: the transaction rules that the reference expresses as JSON schemas.
//! - [`utils`]: rewards, donations, the fee burn, vote order, amounts, Base58, hex, byte cursors
//!   and JavaScript number formatting.
//! - `genesis` (feature `genesis`): the devnet chain-definition builder.
//!
//! Rules for the whole crate:
//!
//! - No I/O, no clock and no global state. Every rule takes the parameters of its height
//!   explicitly: [`managers::Milestones::at`] gives the [`managers::Params`] in force at a height.
//! - Untrusted bytes never cause a panic. Decoders read through [`utils::bytes::Reader`] and return
//!   typed errors from [`errors`].
//! - No type of the elliptic-curve libraries appears in the public API: keys, signatures and
//!   addresses are the opaque types of [`identities`] and [`crypto::sig`].
//! - Every signing call takes an [`Aux`] value; there is no signing entry point without one.
//!
//! Features: `legacy-schnorr` enables the pre-BIP340 Schnorr scheme, which is used only for peer
//! status attestations; `genesis` enables the chain-definition builder. `fixed-aux` is a test seam
//! (`Aux::fixed`, reproducible signatures) that only test code enables; a release build never
//! has it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod blocks;
pub mod crypto;
pub mod errors;
#[cfg(feature = "genesis")]
pub mod genesis;
pub mod identities;
pub mod managers;
pub mod transactions;
pub mod utils;
pub mod validation;

pub use crate::blocks::{Block, BlockHeader, BlockId};
pub use crate::crypto::sig::{Aux, SchemeId, Signature, SigningDomain};
pub use crate::identities::{Address, KeyPair, PublicKey, PublicKeyBytes, SecretKey};
pub use crate::managers::{Milestones, Network, Params};
pub use crate::transactions::{Transaction, TransactionData, TransactionId, TransactionType};
