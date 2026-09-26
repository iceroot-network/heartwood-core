//! Field and scalar helpers for the legacy Schnorr scheme that libsecp256k1 does not expose.
//!
//! Numbers are 32-byte big-endian values, computed over four 64-bit limbs, least significant limb
//! first. [`reduce_mod_n`] also reduces the secret nonce and runs in constant time. The Jacobi
//! symbol runs in variable time, as the reference's residue check does; it sees only the y
//! coordinate of a nonce point whose x coordinate is published in the signature.

use std::cmp::Ordering;

/// Four 64-bit limbs, least significant first.
type Limbs = [u64; 4];

/// The field prime p = 2^256 - 2^32 - 977.
const FIELD_PRIME: Limbs = [
    0xffff_fffe_ffff_fc2f,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
];

/// The curve order n.
const CURVE_ORDER: Limbs = [
    0xbfd2_5e8c_d036_4141,
    0xbaae_dce6_af48_a03b,
    0xffff_ffff_ffff_fffe,
    0xffff_ffff_ffff_ffff,
];

/// Whether `y` is a quadratic residue modulo the field prime p: the Jacobi symbol `(y | p)` is 1.
///
/// Zero, and p itself, are not residues. The algorithm is the binary Jacobi symbol.
pub(super) fn is_quadratic_residue(y: &[u8; 32]) -> bool {
    let mut a = from_be_bytes(y);
    if compare(&a, &FIELD_PRIME) != Ordering::Less {
        a = subtract(&a, &FIELD_PRIME);
    }
    let mut n = FIELD_PRIME;
    let mut positive = true;
    while !is_zero(&a) {
        // (2 | n) is -1 exactly when n is 3 or 5 modulo 8.
        while a[0] & 1 == 0 {
            a = half(&a);
            if matches!(n[0] & 7, 3 | 5) {
                positive = !positive;
            }
        }
        // Both odd. Reciprocity, when swapping, flips the sign if both are 3 modulo 4.
        if compare(&a, &n) == Ordering::Less {
            std::mem::swap(&mut a, &mut n);
            if a[0] & 3 == 3 && n[0] & 3 == 3 {
                positive = !positive;
            }
        }
        // (a | n) = (a - n | n), and a - n is even.
        a = subtract(&a, &n);
    }
    n == [1, 0, 0, 0] && positive
}

/// Whether `value` is below the field prime p, that is, a canonical field element.
pub(super) fn is_field_element(value: &[u8; 32]) -> bool {
    compare(&from_be_bytes(value), &FIELD_PRIME) == Ordering::Less
}

/// `value` reduced modulo the curve order n (one conditional subtraction, as `value < 2^256 < 2n`).
pub(super) fn reduce_mod_n(value: &[u8; 32]) -> [u8; 32] {
    let value = from_be_bytes(value);
    let (reduced, borrow) = subtract_with_borrow(&value, &CURVE_ORDER);
    // Keep `value` when the subtraction borrowed (value < n), without branching on it.
    let keep = 0u64.wrapping_sub(u64::from(borrow));
    let mut out = [0u64; 4];
    for ((out, original), reduced) in out.iter_mut().zip(&value).zip(&reduced) {
        *out = (original & keep) | (reduced & !keep);
    }
    to_be_bytes(&out)
}

fn from_be_bytes(bytes: &[u8; 32]) -> Limbs {
    let mut limbs = [0u64; 4];
    let (chunks, _) = bytes.as_chunks::<8>();
    for (limb, chunk) in limbs.iter_mut().rev().zip(chunks) {
        *limb = u64::from_be_bytes(*chunk);
    }
    limbs
}

fn to_be_bytes(limbs: &Limbs) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let (chunks, _) = bytes.as_chunks_mut::<8>();
    for (chunk, limb) in chunks.iter_mut().zip(limbs.iter().rev()) {
        *chunk = limb.to_be_bytes();
    }
    bytes
}

fn compare(a: &Limbs, b: &Limbs) -> Ordering {
    a.iter().rev().cmp(b.iter().rev())
}

fn is_zero(a: &Limbs) -> bool {
    a.iter().all(|&limb| limb == 0)
}

/// `a - b` modulo 2^256, and whether it borrowed (a < b).
fn subtract_with_borrow(a: &Limbs, b: &Limbs) -> (Limbs, bool) {
    let mut out = [0u64; 4];
    let mut borrow = false;
    for ((out, &a), &b) in out.iter_mut().zip(a).zip(b) {
        let (difference, first) = a.overflowing_sub(b);
        let (difference, second) = difference.overflowing_sub(u64::from(borrow));
        *out = difference;
        borrow = first | second;
    }
    (out, borrow)
}

/// `a - b`, for `a >= b`.
fn subtract(a: &Limbs, b: &Limbs) -> Limbs {
    subtract_with_borrow(a, b).0
}

/// `a / 2`, rounded down.
fn half(a: &Limbs) -> Limbs {
    let mut out = [0u64; 4];
    let mut carry = 0u64;
    for (out, &limb) in out.iter_mut().zip(a).rev() {
        *out = (limb >> 1) | carry;
        carry = limb << 63;
    }
    out
}

#[cfg(test)]
mod tests {
    use num_bigint::BigUint;
    use proptest::prelude::*;

    use super::*;

    fn prime() -> BigUint {
        BigUint::from_bytes_be(&to_be_bytes(&FIELD_PRIME))
    }

    fn order() -> BigUint {
        BigUint::from_bytes_be(&to_be_bytes(&CURVE_ORDER))
    }

    /// Euler's criterion: y^((p - 1) / 2) is 1 modulo p exactly for the nonzero residues.
    fn euler(y: &[u8; 32]) -> bool {
        let p = prime();
        let exponent = (&p - 1u32) >> 1;
        BigUint::from_bytes_be(y).modpow(&exponent, &p) == BigUint::from(1u32)
    }

    fn be(value: &BigUint) -> [u8; 32] {
        let bytes = value.to_bytes_be();
        let mut out = [0u8; 32];
        out[32 - bytes.len()..].copy_from_slice(&bytes);
        out
    }

    #[test]
    fn constants() {
        let p: BigUint = (BigUint::from(1u32) << 256u32) - (BigUint::from(1u32) << 32u32) - 977u32;
        assert_eq!(prime(), p);
        let n = BigUint::parse_bytes(
            b"fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141",
            16,
        )
        .unwrap();
        assert_eq!(order(), n);
    }

    #[test]
    fn edges() {
        let p = prime();
        let n = order();
        let max: BigUint = (BigUint::from(1u32) << 256u32) - 1u32;
        let small = (0u32..64).map(BigUint::from);
        let near_p = (0u32..8).flat_map(|d| [&p - 1u32 - d, &p + d]);
        let near_n = (0u32..8).flat_map(|d| [&n - 1u32 - d, &n + d]);
        for value in small.chain(near_p).chain(near_n).chain([max.clone()]) {
            if value > max {
                continue;
            }
            let bytes = be(&value);
            assert_eq!(is_quadratic_residue(&bytes), euler(&bytes), "{value:x}");
            assert_eq!(is_field_element(&bytes), value < p, "{value:x}");
            assert_eq!(
                BigUint::from_bytes_be(&reduce_mod_n(&bytes)),
                &value % &n,
                "{value:x}"
            );
        }
        assert!(!is_quadratic_residue(&[0; 32]));
        assert!(!is_quadratic_residue(&be(&p)));
        assert!(is_quadratic_residue(&be(&BigUint::from(4u32))));
    }

    proptest! {
        #[test]
        fn residues_match_euler(y in any::<[u8; 32]>()) {
            prop_assert_eq!(is_quadratic_residue(&y), euler(&y));
        }

        #[test]
        fn squares_are_residues(x in any::<[u8; 32]>()) {
            let p = prime();
            let x = BigUint::from_bytes_be(&x) % &p;
            let square = (&x * &x) % &p;
            prop_assert_eq!(is_quadratic_residue(&be(&square)), x != BigUint::from(0u32));
        }

        #[test]
        fn reduction_matches(value in any::<[u8; 32]>()) {
            let expected = BigUint::from_bytes_be(&value) % order();
            prop_assert_eq!(BigUint::from_bytes_be(&reduce_mod_n(&value)), expected);
        }

        #[test]
        fn field_elements(value in any::<[u8; 32]>()) {
            prop_assert_eq!(is_field_element(&value), BigUint::from_bytes_be(&value) < prime());
        }
    }
}
