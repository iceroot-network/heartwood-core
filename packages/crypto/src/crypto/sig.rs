//! Signatures: the one entry point to the signature schemes.
//!
//! Transactions, blocks and off-chain messages are signed with BIP340 Schnorr
//! ([`SchemeId::Secp256k1Bip340`]). The pre-BIP340 Schnorr scheme of the reference
//! (`SchemeId::Secp256k1Legacy`, which exists with the feature `legacy-schnorr`) signs peer status
//! attestations only.
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
    /// BIP340 Schnorr over secp256k1: transactions, blocks and off-chain messages.
    Secp256k1Bip340,
    /// The reference's pre-BIP340 Schnorr over secp256k1: peer status attestations only.
    #[cfg(feature = "legacy-schnorr")]
    Secp256k1Legacy,
}

/// What a signature is for.
///
/// In this signature format the domain changes no bytes: it names the purpose of every signature
/// so that a later format can separate the domains without changing the callers. From the
/// post-quantum formats on, a signature signs a tagged message, the domain's tag from the
/// signing-tag registry followed by the chain id, the genesis hash and the hash of the unsigned
/// bytes, so that a signature made for one domain or one chain is never valid for another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigningDomain {
    /// A transaction's first or second signature.
    Transaction,
    /// A block header.
    Block,
    /// A peer status attestation.
    PeerStatus,
    /// An off-chain message, which is never a transaction or a block. In this format the caller
    /// passes SHA-256 of the message's UTF-8 text as the message, which BIP340 signs as it is,
    /// exactly as the reference's `Message.sign` does, so message signatures stay the reference's.
    /// From the post-quantum formats on, this domain carries the off-chain message tag of the
    /// signing-tag registry.
    Message,
}

/// The auxiliary randomness of a BIP340 signature.
///
/// Every signing call takes one. [`Aux::random`] draws 32 fresh bytes from the operating system for
/// each signature, which is what a node signs with.
///
/// Reproducible signatures are a test seam. `Aux::fixed`, which takes the 32 bytes, exists only
/// with the feature `fixed-aux`, which only test code enables. The chain-definition builder
/// (feature `genesis`) derives its aux from the seed inside the crate. The one production path
/// that chooses the aux is a forger's `aux` option, which the reference's forger has:
/// `Aux::configured`, with the feature `forger-aux`, which only the forger enables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aux(AuxSource);

/// Where the aux bytes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuxSource {
    /// 32 fresh random bytes per signature.
    Random,
    /// These 32 bytes.
    #[cfg(any(
        test,
        feature = "fixed-aux",
        feature = "genesis",
        feature = "forger-aux"
    ))]
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

    /// These 32 bytes, from a forger's `aux` option (feature `forger-aux`): every block the
    /// forger signs takes them, as the reference's forger passes its configured aux to each
    /// signature. A fixed aux keeps the nonce bound to the key and the message (BIP340); it only
    /// gives up the hedging that fresh bytes add.
    #[cfg(feature = "forger-aux")]
    pub const fn configured(bytes: [u8; 32]) -> Aux {
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
            #[cfg(any(
                test,
                feature = "fixed-aux",
                feature = "genesis",
                feature = "forger-aux"
            ))]
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

/// Nor does a build without the feature `forger-aux` have `Aux::configured`: this doctest must fail
/// to compile, and `cargo test -p heartwood-crypto` (no features) runs it.
///
/// ```compile_fail
/// let _ = heartwood_crypto::Aux::configured([0; 32]);
/// ```
#[cfg(not(feature = "forger-aux"))]
mod no_configured_aux {}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::hash::sha256;
    use crate::identities::PublicKey;

    const PASSPHRASE: &str = "heartwood message domain";
    const PUBLIC_KEY: &str = "02cf05d38330aeddf8f5a668111881167458f86e325077c4911b0037cacc58c1a0";

    /// Messages signed by the reference with the key of [`PASSPHRASE`]: the message, the
    /// signature of `Message.sign` (random aux), and the BIP340 signature over the same digest
    /// with the aux 0x42 × 32. The last message is 32 bytes long.
    const MESSAGES: [(&str, &str, &str); 4] = [
        (
            "Hello from Heartwood",
            "1fc4f1dd9e261d77db661915ac1762f856176d8831425a4c46777fdab6401b75\
             657609d68aaa755e5237dcdbbf0eac543621657def7dca07886b1767f97667f9",
            "f25d1a5b3165bab0a01deb04d3c6357fd73e9b78c988ccf0988b11f0375b34cd\
             dd672a1538a702c5b4c7715a1d4a45df3d46f61855dfe6008f21b796360dd049",
        ),
        (
            "",
            "9691bc0ea053f90039012f895927575e9a7f5bd5c9de882bea86e0b1374bcf04\
             5de7fa64358ae63846856d2772c8a201c144ad46cf010a7b754bf9d04ef68aea",
            "c6f8b7c03c0851e0cf5e6be6e41b153e5d5dd621b7f8247b6da96a2ffa969203\
             a310d75fb03be4c83b64e371539dce5ff3d4954cdbd50828420892c7421f0a57",
        ),
        (
            "ünïcödé ✓",
            "8aab6bef4f2087b0f49fc8dc77cc99db2597f278bf2d5b5c5c5292a3061a88da\
             6dce61785fe053be0c4870483c51f34993314ee6a6689060b2edad2d09272d12",
            "1f8f9bd7be2457203fc7891a20713843e1addb208808b7b4ac38ebdc06cefb93\
             968edbd89c694d49cca106011f7c1df97b0a55834cedac144cc2ae1c634b1586",
        ),
        (
            "0123456789abcdef0123456789abcdef",
            "3045020e6280cf11ceaa87f2b5827011ffb4a4099b8a0b6eeef969298f9d3ce6\
             1d9e5b9461c5357a961286863a8538716b62a0fab7094d06aba0e431105ccb55",
            "04a94fa3b288597586c13ef926fe9bee1772da865259b4e5353a935bce8c7c99\
             8934e7d0a0dc163ae5d58730a21ab69ee902eb63e9177c68bf01ed5f9c568d0d",
        ),
    ];

    const DOMAINS: [SigningDomain; 4] = [
        SigningDomain::Transaction,
        SigningDomain::Block,
        SigningDomain::PeerStatus,
        SigningDomain::Message,
    ];

    fn secret() -> SecretKey {
        SecretKey::from_passphrase(PASSPHRASE).unwrap()
    }

    #[test]
    fn message_signatures_are_the_reference_ones() {
        let secret = secret();
        let public = PublicKey::from_secret_key(&secret);
        assert_eq!(public.to_hex(), PUBLIC_KEY);
        for (message, signed, fixed) in MESSAGES {
            let digest = sha256(message.as_bytes());
            let ours = sign(
                SchemeId::Secp256k1Bip340,
                SigningDomain::Message,
                &digest,
                &secret,
                Aux::fixed([0x42; 32]),
            )
            .unwrap();
            assert_eq!(ours.to_hex(), fixed, "{message:?}");
            for signature in [signed, fixed] {
                let signature = Signature::from_hex(signature).unwrap();
                assert!(
                    verify(
                        SchemeId::Secp256k1Bip340,
                        SigningDomain::Message,
                        &digest,
                        &signature,
                        public.as_bytes()
                    ),
                    "{message:?}"
                );
                let other = sha256(format!("{message}.").as_bytes());
                assert!(!verify(
                    SchemeId::Secp256k1Bip340,
                    SigningDomain::Message,
                    &other,
                    &signature,
                    public.as_bytes()
                ));
            }
        }
    }

    #[test]
    fn the_message_domain_changes_no_bytes() {
        let secret = secret();
        let public = PublicKey::from_secret_key(&secret);
        for (message, signed, _) in MESSAGES {
            let digest = sha256(message.as_bytes());
            let signatures = DOMAINS.map(|domain| {
                sign(
                    SchemeId::Secp256k1Bip340,
                    domain,
                    &digest,
                    &secret,
                    Aux::fixed([7; 32]),
                )
                .unwrap()
            });
            assert!(
                signatures.iter().all(|s| *s == signatures[0]),
                "{message:?}"
            );
            let signed = Signature::from_hex(signed).unwrap();
            for domain in DOMAINS {
                assert!(verify(
                    SchemeId::Secp256k1Bip340,
                    domain,
                    &digest,
                    &signed,
                    public.as_bytes()
                ));
            }
        }
    }

    #[test]
    fn the_signed_bytes_are_the_digest_even_for_a_32_byte_message() {
        // BIP340 signs a 32-byte message as given, so a caller that passed the text of a 32-byte
        // message would sign other bytes than the reference: the digest is what is signed.
        let secret = secret();
        let public = PublicKey::from_secret_key(&secret);
        let (message, signed, fixed) = MESSAGES[3];
        assert_eq!(message.len(), 32);
        let raw = sign(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Message,
            message.as_bytes(),
            &secret,
            Aux::fixed([0x42; 32]),
        )
        .unwrap();
        assert_ne!(raw.to_hex(), fixed);
        let signed = Signature::from_hex(signed).unwrap();
        assert!(!verify(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Message,
            message.as_bytes(),
            &signed,
            public.as_bytes()
        ));
    }
}
