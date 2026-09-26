//! Cross-check with `k256`: every signature record of classes V04 and V11 is recomputed with a
//! second curve implementation, and random keys and messages are signed with both.
//!
//! The legacy scheme is written out here with `k256` arithmetic, independently of the crate's own
//! construction on libsecp256k1, with the quadratic-residue test done by Euler's criterion.

use heartwood_crypto::crypto::hash::{sha256, sha256_concat};
use heartwood_crypto::crypto::sig;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, PublicKey, SchemeId, SecretKey, Signature, SigningDomain};
use k256::elliptic_curve::PrimeField;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::point::AffineCoordinates;
use k256::elliptic_curve::sec1::ToSec1Point;
use k256::{AffinePoint, FieldBytes, ProjectivePoint, Scalar};
use num_bigint::BigUint;
use proptest::prelude::*;

use crate::common::{self, VectorFile};
use crate::support::{Case, cases};

/// The signature classes.
fn files() -> [VectorFile; 2] {
    [vector_file!("V04-signatures"), vector_file!("V11-external")]
}

/// The reference's BIP340 message rule.
fn digest(message: &[u8]) -> [u8; 32] {
    <[u8; 32]>::try_from(message).unwrap_or_else(|_| sha256(message))
}

fn k256_bip340_sign(secret: &[u8; 32], message: &[u8], aux: &[u8; 32]) -> [u8; 64] {
    let key = k256::schnorr::SigningKey::from_bytes(&FieldBytes::from(*secret)).unwrap();
    key.sign_raw(&digest(message), aux).unwrap().to_bytes()
}

/// The reference's BIP340 verification: a 33-byte key loses its first byte.
fn k256_bip340_verify(key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let x_only = match key.len() {
        33 => &key[1..],
        _ => key,
    };
    let Ok(key) = k256::schnorr::VerifyingKey::from_slice(x_only) else {
        return false;
    };
    let Ok(signature) = k256::schnorr::Signature::from_slice(signature) else {
        return false;
    };
    key.verify_raw(&digest(message), &signature).is_ok()
}

fn field_prime() -> BigUint {
    BigUint::parse_bytes(
        b"fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f",
        16,
    )
    .unwrap()
}

/// Euler's criterion for y modulo p.
fn is_square(y: &[u8]) -> bool {
    let p = field_prime();
    BigUint::from_bytes_be(y).modpow(&((&p - 1u32) >> 1u32), &p) == BigUint::from(1u32)
}

fn reduce(bytes: [u8; 32]) -> Scalar {
    <Scalar as Reduce<FieldBytes>>::reduce(&FieldBytes::from(bytes))
}

fn compressed(point: &AffinePoint) -> Vec<u8> {
    point.to_sec1_point(true).as_bytes().to_vec()
}

/// A key in the reference's acceptance set, as a `k256` point. `k256` has no hybrid encoding, so
/// a hybrid key with the right parity is read as the uncompressed key with the same coordinates.
fn k256_key(bytes: &[u8]) -> Option<AffinePoint> {
    let mut bytes = bytes.to_vec();
    if bytes.len() == 65 && matches!(bytes[0], 0x06 | 0x07) {
        if bytes[0] & 1 != bytes[64] & 1 {
            return None;
        }
        bytes[0] = 0x04;
    }
    let point = k256::PublicKey::from_sec1_bytes(&bytes).ok()?;
    Some(*point.as_affine())
}

fn k256_legacy_sign(secret: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let a = Option::<Scalar>::from(Scalar::from_repr(FieldBytes::from(*secret))).unwrap();
    let a_point = compressed(&(ProjectivePoint::GENERATOR * a).to_affine());
    let mut k = reduce(sha256_concat([secret.as_slice(), message]));
    let r = (ProjectivePoint::GENERATOR * k).to_affine();
    if !is_square(&r.y()) {
        k = -k;
    }
    let r_x = r.x();
    let e = reduce(sha256_concat([r_x.as_slice(), &a_point, message]));
    let s = k + e * a;
    let mut signature = [0u8; 64];
    signature[..32].copy_from_slice(&r_x);
    signature[32..].copy_from_slice(&s.to_bytes());
    signature
}

fn k256_legacy_verify(key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Some(a) = k256_key(key) else {
        return false;
    };
    if signature.len() != 64 {
        return false;
    }
    let (r, s) = signature.split_at(32);
    if BigUint::from_bytes_be(r) >= field_prime() {
        return false;
    }
    let Some(s) = Option::<Scalar>::from(Scalar::from_repr(FieldBytes::from(
        <[u8; 32]>::try_from(s).unwrap(),
    ))) else {
        return false;
    };
    let e = reduce(sha256_concat([r, &compressed(&a), message]));
    let point = ProjectivePoint::GENERATOR * s - ProjectivePoint::from(a) * e;
    if point == ProjectivePoint::IDENTITY {
        return false;
    }
    let point = point.to_affine();
    is_square(&point.y()) && point.x().as_slice() == r
}

fn secret_of(case: Case<'_>) -> [u8; 32] {
    hex::decode_array::<32>(case.text("privateKey")).unwrap()
}

#[test]
fn bip340_records_agree() {
    let mut checked = 0;
    for file in files() {
        for case in cases(&file, "sig.bip340.sign") {
            let aux = hex::decode_array::<32>(case.text("aux")).unwrap();
            let signature = k256_bip340_sign(&secret_of(case), &case.bytes("message"), &aux);
            assert_eq!(
                hex::encode(&signature),
                case.output_text("signature"),
                "{}",
                case.label()
            );
            checked += 1;
        }
        for case in cases(&file, "sig.bip340.verify") {
            let verdict = k256_bip340_verify(
                &case.bytes("publicKey"),
                &case.bytes("message"),
                &case.bytes("signature"),
            );
            assert_eq!(verdict, case.valid(), "{}", case.label());
            checked += 1;
        }
    }
    assert_eq!(checked, 59 + 279 + 8 + 19);
}

#[test]
fn legacy_records_agree() {
    let mut checked = 0;
    for file in files() {
        for case in cases(&file, "sig.legacy.sign") {
            let signature = k256_legacy_sign(&secret_of(case), &case.bytes("message"));
            assert_eq!(
                hex::encode(&signature),
                case.output_text("signature"),
                "{}",
                case.label()
            );
            checked += 1;
        }
        for case in cases(&file, "sig.legacy.verify") {
            let verdict = k256_legacy_verify(
                &case.bytes("publicKey"),
                &case.bytes("message"),
                &case.bytes("signature"),
            );
            assert_eq!(verdict, case.valid(), "{}", case.label());
            checked += 1;
        }
    }
    assert_eq!(checked, 68 + 285 + 3 + 16);
}

fn secret_strategy() -> impl Strategy<Value = [u8; 32]> {
    any::<[u8; 32]>().prop_filter("a valid secret key", |bytes| {
        SecretKey::from_bytes(*bytes).is_ok()
    })
}

fn message_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        proptest::collection::vec(any::<u8>(), 32..=32),
        proptest::collection::vec(any::<u8>(), 0..200),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn bip340_agrees(secret in secret_strategy(), message in message_strategy(), aux in any::<[u8; 32]>()) {
        let key = SecretKey::from_bytes(secret).unwrap();
        let ours = sig::sign(SchemeId::Secp256k1Bip340, SigningDomain::Transaction, &message, &key, Aux::fixed(aux)).unwrap();
        prop_assert_eq!(ours.as_bytes(), &k256_bip340_sign(&secret, &message, &aux));
        let public = PublicKey::from_secret_key(&key);
        prop_assert!(k256_bip340_verify(public.as_bytes(), &message, ours.as_bytes()));
        prop_assert!(sig::verify(SchemeId::Secp256k1Bip340, SigningDomain::Transaction, &message, &ours, public.as_bytes()));
    }

    #[test]
    fn bip340_mutations_agree(
        secret in secret_strategy(),
        message in message_strategy(),
        flip in 0usize..(64 * 8),
    ) {
        let key = SecretKey::from_bytes(secret).unwrap();
        let signature = sig::sign(SchemeId::Secp256k1Bip340, SigningDomain::Block, &message, &key, Aux::fixed([7; 32])).unwrap();
        let mut bytes = *signature.as_bytes();
        bytes[flip / 8] ^= 1 << (flip % 8);
        let public = PublicKey::from_secret_key(&key);
        let ours = sig::verify(SchemeId::Secp256k1Bip340, SigningDomain::Block, &message, &Signature::from_array(bytes), public.as_bytes());
        prop_assert_eq!(ours, k256_bip340_verify(public.as_bytes(), &message, &bytes));
        prop_assert!(!ours);
    }
}

#[cfg(feature = "legacy-schnorr")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn legacy_agrees(secret in secret_strategy(), message in message_strategy()) {
        let key = SecretKey::from_bytes(secret).unwrap();
        let ours = sig::sign(SchemeId::Secp256k1Legacy, SigningDomain::PeerStatus, &message, &key, Aux::random()).unwrap();
        prop_assert_eq!(ours.as_bytes(), &k256_legacy_sign(&secret, &message));
        let public = PublicKey::from_secret_key(&key);
        prop_assert!(k256_legacy_verify(public.as_bytes(), &message, ours.as_bytes()));
        prop_assert!(sig::verify(SchemeId::Secp256k1Legacy, SigningDomain::PeerStatus, &message, &ours, public.as_bytes()));
    }

    #[test]
    fn legacy_mutations_agree(
        secret in secret_strategy(),
        message in message_strategy(),
        flip in 0usize..(64 * 8),
    ) {
        let key = SecretKey::from_bytes(secret).unwrap();
        let signature = sig::sign(SchemeId::Secp256k1Legacy, SigningDomain::PeerStatus, &message, &key, Aux::random()).unwrap();
        let mut bytes = *signature.as_bytes();
        bytes[flip / 8] ^= 1 << (flip % 8);
        let public = PublicKey::from_secret_key(&key);
        let ours = sig::verify(SchemeId::Secp256k1Legacy, SigningDomain::PeerStatus, &message, &Signature::from_array(bytes), public.as_bytes());
        prop_assert_eq!(ours, k256_legacy_verify(public.as_bytes(), &message, &bytes));
        prop_assert!(!ours);
    }

    /// Arbitrary signatures, mostly invalid: the two implementations give the same verdict.
    #[test]
    fn legacy_arbitrary_agree(
        secret in secret_strategy(),
        message in message_strategy(),
        signature in any::<[u8; 64]>(),
    ) {
        let key = SecretKey::from_bytes(secret).unwrap();
        let public = PublicKey::from_secret_key(&key);
        let ours = sig::verify(SchemeId::Secp256k1Legacy, SigningDomain::PeerStatus, &message, &Signature::from_array(signature), public.as_bytes());
        prop_assert_eq!(ours, k256_legacy_verify(public.as_bytes(), &message, &signature));
    }
}
