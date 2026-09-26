//! BIP340 Schnorr signatures over secp256k1, through libsecp256k1.
//!
//! The message rule is the reference's: 32 bytes are signed as given, any other length is hashed
//! with SHA-256 first. Verification reads a 33-byte key as its x coordinate (bytes 1 to 32) without
//! checking the prefix byte, takes a 32-byte key as it is, and fails on any other length.

use secp256k1::{Keypair, XOnlyPublicKey, schnorr};

use crate::crypto::hash::sha256;
use crate::crypto::sig::Signature;
use crate::identities::SecretKey;

/// The BIP340 signature of `message` by `secret`, with the aux bytes `aux`.
pub(super) fn sign(message: &[u8], secret: &SecretKey, aux: &[u8; 32]) -> Signature {
    match <&[u8; 32]>::try_from(message) {
        Ok(digest) => sign_raw(digest, secret, aux),
        Err(_) => sign_raw(&sha256(message), secret, aux),
    }
}

/// Whether `signature` is a valid BIP340 signature of `message` for the key bytes `public_key`.
pub(super) fn verify(message: &[u8], signature: &Signature, public_key: &[u8]) -> bool {
    let x_only = match public_key {
        [_prefix, x @ ..] if x.len() == 32 => x,
        _ => public_key,
    };
    match <&[u8; 32]>::try_from(message) {
        Ok(digest) => verify_raw(digest, signature, x_only),
        Err(_) => verify_raw(&sha256(message), signature, x_only),
    }
}

/// BIP340 signing of `message` as it is, whatever its length.
fn sign_raw(message: &[u8], secret: &SecretKey, aux: &[u8; 32]) -> Signature {
    let mut keypair = Keypair::from_secret_key(secret.backend());
    let signature = schnorr::sign_with_aux_rand(message, &keypair, aux);
    keypair.non_secure_erase();
    Signature::from_array(signature.to_byte_array())
}

/// BIP340 verification of `message` as it is, whatever its length, for the x-only key `x_only`,
/// which must be 32 bytes.
fn verify_raw(message: &[u8], signature: &Signature, x_only: &[u8]) -> bool {
    let Ok(x_only) = <[u8; 32]>::try_from(x_only) else {
        return false;
    };
    let Ok(key) = XOnlyPublicKey::from_byte_array(x_only) else {
        return false;
    };
    let signature = schnorr::Signature::from_byte_array(*signature.as_bytes());
    schnorr::verify(&signature, message, &key).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::hex;

    const OFFICIAL: &str = include_str!("../../tests/vectors/external/bip340-test-vectors.csv");

    struct Row<'a> {
        index: &'a str,
        secret: &'a str,
        public: &'a str,
        aux: &'a str,
        message: Vec<u8>,
        signature: Signature,
        valid: bool,
    }

    fn rows() -> Vec<Row<'static>> {
        OFFICIAL
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let fields: Vec<&str> = line.splitn(8, ',').map(str::trim).collect();
                Row {
                    index: fields[0],
                    secret: fields[1],
                    public: fields[2],
                    aux: fields[3],
                    message: hex::decode(fields[4]).unwrap(),
                    signature: Signature::from_hex(fields[5]).unwrap(),
                    valid: match fields[6] {
                        "TRUE" => true,
                        "FALSE" => false,
                        other => panic!("row {}: verdict {other}", fields[0]),
                    },
                }
            })
            .collect()
    }

    /// The published vectors, messages of every length taken as they are.
    #[test]
    fn official_vectors_raw() {
        let rows = rows();
        assert_eq!(rows.len(), 19);
        for row in &rows {
            if !row.secret.is_empty() {
                let secret = SecretKey::from_hex(row.secret).unwrap();
                let aux = hex::decode_array::<32>(row.aux).unwrap();
                let signature = sign_raw(&row.message, &secret, &aux);
                assert_eq!(signature, row.signature, "row {} sign", row.index);
                let public = secp256k1::PublicKey::from_secret_key(secret.backend());
                assert_eq!(
                    hex::encode(&public.x_only_public_key().0.to_byte_array()),
                    row.public.to_ascii_lowercase(),
                    "row {} key",
                    row.index
                );
            }
            let key = hex::decode(row.public).unwrap();
            assert_eq!(
                verify_raw(&row.message, &row.signature, &key),
                row.valid,
                "row {} verify",
                row.index
            );
        }
    }

    /// The reference's message rule: 32 bytes as given, other lengths hashed first. The published
    /// rows with other message lengths therefore fail through the public entry points.
    #[test]
    fn message_rule() {
        for row in rows() {
            let key = hex::decode(row.public).unwrap();
            let expected = row.valid && row.message.len() == 32;
            assert_eq!(
                verify(&row.message, &row.signature, &key),
                expected,
                "row {}",
                row.index
            );
            if !row.secret.is_empty() {
                let secret = SecretKey::from_hex(row.secret).unwrap();
                let aux = hex::decode_array::<32>(row.aux).unwrap();
                let hashed = sign_raw(&sha256(&row.message), &secret, &aux);
                let signature = sign(&row.message, &secret, &aux);
                if row.message.len() == 32 {
                    assert_eq!(signature, row.signature);
                } else {
                    assert_eq!(signature, hashed);
                    assert!(verify(&row.message, &signature, &key));
                    assert!(verify_raw(&sha256(&row.message), &signature, &key));
                }
            }
        }
    }

    /// A 33-byte key is read as its x coordinate whatever its prefix; other lengths fail.
    #[test]
    fn key_slice() {
        let row = &rows()[1];
        let x = hex::decode(row.public).unwrap();
        for prefix in [0x00, 0x02, 0x03, 0x05, 0xff] {
            let mut key = vec![prefix];
            key.extend_from_slice(&x);
            assert!(
                verify(&row.message, &row.signature, &key),
                "prefix {prefix:02x}"
            );
        }
        assert!(verify(&row.message, &row.signature, &x));
        assert!(!verify(&row.message, &row.signature, &x[1..]));
        let mut long = vec![0x02];
        long.extend_from_slice(&x);
        long.push(0);
        assert!(!verify(&row.message, &row.signature, &long));
        assert!(!verify(&row.message, &row.signature, &[]));
    }
}
