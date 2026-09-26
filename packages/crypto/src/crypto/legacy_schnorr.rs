//! The reference's pre-BIP340 Schnorr signatures over secp256k1.
//!
//! This is the bip-schnorr draft at commit `d194620` as the reference's native backend implements
//! it, with messages of any length:
//!
//! - signing with secret `a` and public key `A = a·G`: `k = SHA-256(a ‖ m) mod n`, refused when
//!   zero; `R = k·G`, and `k` becomes `n - k` when `y(R)` is not a quadratic residue modulo p;
//!   `e = SHA-256(x(R) ‖ A ‖ m) mod n` with `A` compressed; the signature is `x(R) ‖ (k + e·a mod n)`;
//! - verification: the key parses with the reference's acceptance set; `r < p` and `s < n`;
//!   `e` as above with the key compressed; `R = s·G - e·A` must not be the point at infinity,
//!   `y(R)` must be a quadratic residue, and `x(R)` must equal `r`.
//!
//! Built on the public API of libsecp256k1 (scalar and point tweaks, key combination) and the
//! Jacobi-symbol helper in `legendre`, so secret keys never enter a second curve library. Used for
//! peer status attestations only.

use secp256k1::{PublicKey, Scalar, SecretKey as CurveSecret};
use zeroize::Zeroizing;

use crate::crypto::hash::sha256_concat;
use crate::crypto::legendre::{is_field_element, is_quadratic_residue, reduce_mod_n};
use crate::crypto::sig::Signature;
use crate::errors::SigError;
use crate::identities::SecretKey;

/// The legacy Schnorr signature of `message` by `secret`.
pub(super) fn sign(message: &[u8], secret: &SecretKey) -> Result<Signature, SigError> {
    let a = secret.backend();
    let a_point = PublicKey::from_secret_key(a).serialize();

    let nonce = Zeroizing::new(reduce_mod_n(&sha256_concat([
        secret.expose().as_slice(),
        message,
    ])));
    let mut k = CurveSecret::from_secret_bytes(*nonce).map_err(|_| SigError::SigningFailed)?;
    let r_point = PublicKey::from_secret_key(&k);
    let Some((r, r_y)) = coordinates(&r_point) else {
        k.non_secure_erase();
        return Err(SigError::SigningFailed);
    };
    if !is_quadratic_residue(&r_y) {
        k = k.negate();
    }

    let e = challenge(&r, &a_point, message);
    let s = add_product(&k, &e, a);
    k.non_secure_erase();
    let s = s.ok_or(SigError::SigningFailed)?;

    let mut signature = [0u8; 64];
    signature[..32].copy_from_slice(&r);
    signature[32..].copy_from_slice(&s);
    Ok(Signature::from_array(signature))
}

/// Whether `signature` is a valid legacy Schnorr signature of `message` for the key bytes
/// `public_key`, which must parse with the reference's acceptance set.
pub(super) fn verify(message: &[u8], signature: &Signature, public_key: &[u8]) -> bool {
    let Ok(a_point) = PublicKey::from_slice(public_key) else {
        return false;
    };
    let (halves, _) = signature.as_bytes().as_chunks::<32>();
    let [r, s] = halves else {
        return false;
    };
    if !is_field_element(r) {
        return false;
    }
    let Ok(s) = Scalar::from_be_bytes(*s) else {
        return false;
    };
    let e = challenge(r, &a_point.serialize(), message);
    let Some(point) = combination(&s, &e, &a_point) else {
        // The point at infinity, or a scalar the curve library refused.
        return false;
    };
    let Some((x, y)) = coordinates(&point) else {
        return false;
    };
    is_quadratic_residue(&y) && x == *r
}

/// `e = SHA-256(r ‖ A ‖ m) mod n`, with `A` the compressed key.
fn challenge(r: &[u8; 32], a_point: &[u8; 33], message: &[u8]) -> Scalar {
    let e = reduce_mod_n(&sha256_concat([r.as_slice(), a_point.as_slice(), message]));
    // A value reduced modulo n is always in range; the fallback is never taken.
    Scalar::from_be_bytes(e).unwrap_or(Scalar::ZERO)
}

/// `k + e·a mod n`, as 32 bytes. Zero is a possible result, as in the reference.
///
/// `None` only if the curve library refuses a step, which it does not for a nonzero `e` below n
/// and valid keys `k` and `a`.
fn add_product(k: &CurveSecret, e: &Scalar, a: &CurveSecret) -> Option<[u8; 32]> {
    if *e == Scalar::ZERO {
        return Some(k.to_secret_bytes());
    }
    let mut product = a.mul_tweak(e).ok()?;
    let tweak = Scalar::from_be_bytes(k.to_secret_bytes()).ok();
    let s = tweak.map(|tweak| match product.add_tweak(&tweak) {
        Ok(mut sum) => {
            let bytes = sum.to_secret_bytes();
            sum.non_secure_erase();
            bytes
        }
        // Adding a tweak below n to a valid key fails only when the sum is zero modulo n.
        Err(_) => [0u8; 32],
    });
    product.non_secure_erase();
    s
}

/// `s·G - e·A`, or `None` for the point at infinity.
fn combination(s: &Scalar, e: &Scalar, a_point: &PublicKey) -> Option<PublicKey> {
    // s·G, with s = 0 giving the point at infinity (no key of the curve library is zero).
    let s_g = CurveSecret::from_secret_bytes(s.to_be_bytes())
        .ok()
        .map(|s| PublicKey::from_secret_key(&s));
    // -e·A, with e = 0 giving the point at infinity.
    let minus_e_a = if *e == Scalar::ZERO {
        None
    } else {
        Some(a_point.mul_tweak(e).ok()?.negate())
    };
    match (s_g, minus_e_a) {
        // The sum fails exactly when it is the point at infinity.
        (Some(s_g), Some(minus_e_a)) => s_g.combine(&minus_e_a).ok(),
        (Some(point), None) | (None, Some(point)) => Some(point),
        (None, None) => None,
    }
}

/// The affine x and y coordinates of `point`.
fn coordinates(point: &PublicKey) -> Option<([u8; 32], [u8; 32])> {
    let uncompressed = point.serialize_uncompressed();
    let (_, xy) = uncompressed.split_first()?;
    let (chunks, _) = xy.as_chunks::<32>();
    match chunks {
        [x, y] => Some((*x, *y)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::hex;

    const DRAFT: &str =
        include_str!("../../tests/vectors/external/bip-schnorr-d194620-test-vectors.csv");

    /// The published draft vectors: 16 verifications, 3 of them with a secret key to sign with.
    #[test]
    fn draft_vectors() {
        let mut rows = 0;
        for line in DRAFT.lines().skip(1).filter(|line| !line.trim().is_empty()) {
            let fields: Vec<&str> = line.splitn(7, ',').map(str::trim).collect();
            let (index, secret, public, message, signature, verdict) = (
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5],
            );
            let message = hex::decode(message).unwrap();
            let signature = Signature::from_hex(signature).unwrap();
            let public = hex::decode(public).unwrap();
            let valid = verdict == "TRUE";
            if !secret.is_empty() {
                let secret = SecretKey::from_hex(secret).unwrap();
                assert_eq!(sign(&message, &secret).unwrap(), signature, "row {index}");
            }
            assert_eq!(verify(&message, &signature, &public), valid, "row {index}");
            rows += 1;
        }
        assert_eq!(rows, 16);
    }

    /// `s·G - e·A` at the point at infinity is refused, whatever `r` claims.
    #[test]
    fn infinity_refused() {
        let secret = SecretKey::from_hex(&format!("{:064x}", 7)).unwrap();
        let public = PublicKey::from_secret_key(secret.backend());
        let message = b"infinity";
        for r in [[0u8; 32], generator_x()] {
            // With s = e·a, s·G - e·A is the point at infinity.
            let e = challenge(&r, &public.serialize(), message);
            let s = secret.backend().mul_tweak(&e).unwrap().to_secret_bytes();
            let s_scalar = Scalar::from_be_bytes(s).unwrap();
            assert!(combination(&s_scalar, &e, &public).is_none());
            let mut bytes = [0u8; 64];
            bytes[..32].copy_from_slice(&r);
            bytes[32..].copy_from_slice(&s);
            let signature = Signature::from_array(bytes);
            assert!(!verify(message, &signature, &public.serialize()));
        }
    }

    fn generator_x() -> [u8; 32] {
        hex::decode_array("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
            .unwrap()
    }

    #[test]
    fn round_trip_and_encodings() {
        let secret = SecretKey::from_passphrase("legacy round trip").unwrap();
        let public = PublicKey::from_secret_key(secret.backend());
        for length in [0usize, 1, 31, 32, 33, 64, 1000] {
            let message = vec![0xa5; length];
            let signature = sign(&message, &secret).unwrap();
            assert!(
                verify(&message, &signature, &public.serialize()),
                "{length}"
            );
            let uncompressed = public.serialize_uncompressed();
            assert!(verify(&message, &signature, &uncompressed), "{length}");
            assert!(
                !verify(&message, &signature, &uncompressed[1..33]),
                "{length}"
            );
            let mut other = message.clone();
            other.push(0);
            assert!(!verify(&other, &signature, &public.serialize()), "{length}");
        }
    }
}
