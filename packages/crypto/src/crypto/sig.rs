//! Signatures: the one entry point to the signature schemes.
//!
//! Transactions and blocks are signed with BIP340 Schnorr ([`SchemeId::Secp256k1Bip340`]). The
//! pre-BIP340 Schnorr scheme of the reference (`SchemeId::Secp256k1Legacy`, which exists with the
//! feature `legacy-schnorr`) signs peer status attestations only.
//!
//! Both schemes take the message bytes as the reference passes them. BIP340 signs a 32-byte
//! message as given and hashes any other length with SHA-256 first.

use crate::errors::{HexError, SigError};
use crate::identities::SecretKey;
use crate::utils::hex;

use super::bip340;
#[cfg(feature = "legacy-schnorr")]
use super::legacy_schnorr;

/// A signature scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SchemeId {
    /// BIP340 Schnorr over secp256k1: transactions and blocks.
    Secp256k1Bip340,
    /// The reference's pre-BIP340 Schnorr over secp256k1: peer status attestations only.
    #[cfg(feature = "legacy-schnorr")]
    Secp256k1Legacy,
}

/// What a signature is for.
///
/// In this signature format the domain changes no bytes: it names the purpose of every signature
/// so that a later format can separate the domains without changing the callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigningDomain {
    /// A transaction's first or second signature.
    Transaction,
    /// A block header.
    Block,
    /// A peer status attestation.
    PeerStatus,
}

/// The auxiliary randomness of a BIP340 signature.
///
/// Every signing call takes one. [`Aux::random`] draws 32 fresh bytes from the operating system for
/// each signature, which is what a node signs with.
///
/// Reproducible signatures are a test seam. `Aux::fixed`, which takes the 32 bytes, exists only
/// with the feature `fixed-aux`, which only test code enables; a release build has no way to
/// choose the aux. The chain-definition builder (feature `genesis`) derives its aux from the seed
/// inside the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aux(AuxSource);

/// Where the aux bytes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuxSource {
    /// 32 fresh random bytes per signature.
    Random,
    /// These 32 bytes.
    #[cfg(any(test, feature = "fixed-aux", feature = "genesis"))]
    Fixed([u8; 32]),
}

impl Aux {
    /// 32 fresh random bytes per signature.
    pub const fn random() -> Aux {
        Aux(AuxSource::Random)
    }

    /// These 32 bytes, for reproducible signatures in tests (feature `fixed-aux`).
    #[cfg(any(test, feature = "fixed-aux"))]
    pub const fn fixed(bytes: [u8; 32]) -> Aux {
        Aux(AuxSource::Fixed(bytes))
    }

    /// These 32 bytes, derived by the chain-definition builder from its seed.
    #[cfg(feature = "genesis")]
    pub(crate) const fn derived(bytes: [u8; 32]) -> Aux {
        Aux(AuxSource::Fixed(bytes))
    }

    /// The 32 aux bytes: the fixed ones, or fresh random ones.
    pub(crate) fn resolve(self) -> Result<[u8; 32], SigError> {
        match self.0 {
            #[cfg(any(test, feature = "fixed-aux", feature = "genesis"))]
            AuxSource::Fixed(bytes) => Ok(bytes),
            AuxSource::Random => {
                let mut bytes = [0u8; 32];
                getrandom::fill(&mut bytes).map_err(|_| SigError::Randomness)?;
                Ok(bytes)
            }
        }
    }
}

/// A build without the feature `fixed-aux` has no way to choose the aux: this doctest must fail to
/// compile, and `cargo test -p heartwood-crypto` (no features) runs it.
///
/// ```compile_fail
/// let _ = heartwood_crypto::Aux::fixed([0; 32]);
/// ```
#[cfg(not(feature = "fixed-aux"))]
mod no_fixed_aux {}

/// A 64-byte Schnorr signature of either scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature([u8; 64]);

impl Signature {
    /// The signature with these 64 bytes.
    pub const fn from_array(bytes: [u8; 64]) -> Signature {
        Signature(bytes)
    }

    /// The signature in `bytes`, which must be 64 bytes long.
    pub fn from_bytes(bytes: &[u8]) -> Result<Signature, SigError> {
        <[u8; 64]>::try_from(bytes)
            .map(Signature)
            .map_err(|_| SigError::InvalidLength {
                actual: bytes.len(),
            })
    }

    /// The signature in the hex `text`.
    pub fn from_hex(text: &str) -> Result<Signature, SigError> {
        hex::decode_array::<64>(text)
            .map(Signature)
            .map_err(|error| match error {
                HexError::WrongLength { actual, .. } => SigError::InvalidLength { actual },
                other => SigError::Hex(other),
            })
    }

    /// The 64 bytes.
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    /// Lowercase hex of the 64 bytes.
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

/// Sign `message` with `secret` under `scheme`.
///
/// `aux` is used by BIP340. The legacy scheme derives its nonce from the key and the message, as
/// the reference does, and does not use `aux`.
pub fn sign(
    scheme: SchemeId,
    domain: SigningDomain,
    message: &[u8],
    secret: &SecretKey,
    aux: Aux,
) -> Result<Signature, SigError> {
    // The domain changes no bytes in this signature format.
    let _ = domain;
    match scheme {
        SchemeId::Secp256k1Bip340 => Ok(bip340::sign(message, secret, &aux.resolve()?)),
        #[cfg(feature = "legacy-schnorr")]
        SchemeId::Secp256k1Legacy => legacy_schnorr::sign(message, secret),
    }
}

/// Whether `signature` signs `message` for the key in `public_key` under `scheme`.
///
/// `public_key` is the key as the reference receives it, not necessarily a valid key. BIP340 reads
/// a 33-byte key as its 32-byte x coordinate without checking the prefix byte, takes 32 bytes as an
/// x-only key, and fails on any other length. The legacy scheme parses the key with the reference's
/// acceptance set. A key that does not parse fails the check; it is never an error.
pub fn verify(
    scheme: SchemeId,
    domain: SigningDomain,
    message: &[u8],
    signature: &Signature,
    public_key: &[u8],
) -> bool {
    // The domain changes no bytes in this signature format.
    let _ = domain;
    match scheme {
        SchemeId::Secp256k1Bip340 => bip340::verify(message, signature, public_key),
        #[cfg(feature = "legacy-schnorr")]
        SchemeId::Secp256k1Legacy => legacy_schnorr::verify(message, signature, public_key),
    }
}
