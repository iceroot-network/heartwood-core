//! Field and scalar helpers for the legacy Schnorr scheme that libsecp256k1 does not expose.
//!
//! Numbers are 32-byte big-endian values, computed over four 64-bit limbs.

#![expect(
    unused_variables,
    dead_code,
    reason = "the bodies are not written yet; remove this once they are"
)]

/// Whether `y` is a quadratic residue modulo the field prime p: the Jacobi symbol `(y | p)` is 1.
pub(super) fn is_quadratic_residue(y: &[u8; 32]) -> bool {
    todo!()
}

/// `value` reduced modulo the curve order n (one conditional subtraction, as `value < 2^256 < 2n`).
pub(super) fn reduce_mod_n(value: &[u8; 32]) -> [u8; 32] {
    todo!()
}
