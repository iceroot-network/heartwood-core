//! Class V01: passphrase keys, the public-key acceptance set and addresses.

use heartwood_crypto::errors::{AddressError, Base58Error, KeyError};
use heartwood_crypto::utils::{base58, hex};
use heartwood_crypto::{Address, KeyPair, PublicKey, SecretKey};
use num_bigint::BigUint;
use proptest::prelude::*;

use crate::common::{self, VectorFile};
use crate::support::{assert_coverage, cases, configured_network};

fn file() -> VectorFile {
    vector_file!("V01-keys-addresses")
}

#[test]
fn every_record_is_checked() {
    let file = file();
    assert_eq!(file.records.len(), 85);
    assert_coverage(
        &file,
        &[
            "keys.fromPassphrase",
            "address.fromPublicKey",
            "address.validate",
            "address.toBuffer",
            "publicKey.verify",
        ],
        &[],
    );
}

#[test]
fn keys_from_passphrase() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "keys.fromPassphrase") {
        let passphrase = case.text("passphrase");
        let secret = SecretKey::from_passphrase(passphrase).unwrap();
        assert_eq!(
            secret.to_hex().as_str(),
            case.output_text("privateKey"),
            "{}",
            case.label()
        );
        let pair = KeyPair::from_passphrase(passphrase).unwrap();
        assert_eq!(
            pair.public_key().to_hex(),
            case.output_text("publicKey"),
            "{}",
            case.label()
        );
        assert_eq!(pair.public_key(), &PublicKey::from_secret_key(&secret));
        checked += 1;
    }
    assert_eq!(checked, 6);
}

#[test]
fn addresses_from_public_keys() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "address.fromPublicKey") {
        let network = case
            .optional_byte("networkByte")
            .unwrap_or(configured_network(&file));
        let parsed = PublicKey::from_bytes(&case.bytes("publicKey"));
        match &case.record.expected {
            Ok(_) => {
                let key = parsed.unwrap_or_else(|e| panic!("{}: {e}", case.label()));
                let address = Address::from_public_key(&key, network);
                assert_eq!(
                    address.to_base58(),
                    case.output_text("address"),
                    "{}",
                    case.label()
                );
                assert_eq!(address.network(), network);
                assert_eq!(Address::from_base58(&address.to_base58()), Ok(address));
            }
            Err(class) => {
                assert_eq!(class, "PublicKeyError", "{}", case.label());
                assert_eq!(parsed, Err(KeyError::InvalidPublicKey), "{}", case.label());
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 38);
}

#[test]
fn public_key_acceptance_set() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "publicKey.verify") {
        let bytes = case.bytes("publicKey");
        assert_eq!(
            PublicKey::is_valid(&bytes),
            case.valid(),
            "{}",
            case.label()
        );
        if let Ok(key) = PublicKey::from_bytes(&bytes) {
            assert_eq!(key.as_bytes(), bytes.as_slice(), "{}", case.label());
            let compressed = key.to_compressed();
            assert_eq!(
                compressed.parse().map(|k| k.to_compressed()),
                Ok(compressed)
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 20);
}

#[test]
fn address_validation() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "address.validate") {
        let network = case
            .optional_byte("networkByte")
            .unwrap_or(configured_network(&file));
        let text = case.text("address");
        assert_eq!(
            Address::validate(text, network),
            case.valid(),
            "{}",
            case.label()
        );
        checked += 1;
    }
    assert_eq!(checked, 15);
}

/// The reference's `Address.toBuffer`: the Base58Check payload of any length, and a network
/// mismatch reported beside it rather than raised. [`Address::from_base58_for`] is the strict
/// form, which also requires a 21-byte payload.
#[test]
fn address_payloads() {
    let file = file();
    let network = configured_network(&file);
    let mut checked = 0;
    for case in cases(&file, "address.toBuffer") {
        let text = case.text("address");
        let decoded = base58::decode_check(text);
        match &case.record.expected {
            Ok(output) => {
                let payload = decoded.unwrap_or_else(|e| panic!("{}: {e}", case.label()));
                assert_eq!(hex::encode(&payload), case.output_text("addressBuffer"));
                let first = payload.first().copied();
                let strict = Address::from_base58_for(text, network);
                match output["addressError"].as_str() {
                    None => {
                        assert!(output["addressError"].is_null(), "{}", case.label());
                        assert_eq!(first, Some(network), "{}", case.label());
                        assert!(Address::validate(text, network), "{}", case.label());
                    }
                    Some(message) => {
                        let actual = first.unwrap_or_else(|| panic!("{}", case.label()));
                        assert_eq!(
                            message,
                            format!("Expected address network byte {network}, but got {actual}."),
                            "{}",
                            case.label()
                        );
                        assert_eq!(
                            strict,
                            Err(AddressError::WrongNetwork {
                                expected: network,
                                actual
                            }),
                            "{}",
                            case.label()
                        );
                    }
                }
                if payload.len() == 21 {
                    let address = Address::from_base58(text).unwrap();
                    assert_eq!(address.as_bytes().as_slice(), payload.as_slice());
                    assert_eq!(address.to_base58(), text);
                } else {
                    assert_eq!(
                        strict,
                        Err(AddressError::InvalidLength {
                            actual: payload.len()
                        }),
                        "{}",
                        case.label()
                    );
                }
            }
            Err(class) => {
                // The reference raises a plain Error for a bad checksum.
                assert_eq!(class, "Error", "{}", case.label());
                assert_eq!(
                    decoded,
                    Err(Base58Error::InvalidChecksum),
                    "{}",
                    case.label()
                );
                assert_eq!(
                    Address::from_base58(text),
                    Err(AddressError::Base58(Base58Error::InvalidChecksum))
                );
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 6);
}

const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Base58 by long division, independent of the encoder under test.
fn base58_by_division(bytes: &[u8]) -> String {
    let zeros = bytes.iter().take_while(|&&byte| byte == 0).count();
    let mut digits = BigUint::from_bytes_be(bytes).to_radix_be(58);
    if digits == [0] {
        digits.clear();
    }
    let mut text = "1".repeat(zeros);
    text.extend(
        digits
            .iter()
            .map(|&digit| char::from(ALPHABET[usize::from(digit)])),
    );
    text
}

proptest! {
    #[test]
    fn base58_matches_long_division(bytes in proptest::collection::vec(any::<u8>(), 0..80)) {
        let text = base58::encode(&bytes);
        prop_assert_eq!(&text, &base58_by_division(&bytes));
        prop_assert_eq!(base58::decode(&text).unwrap(), bytes);
    }

    #[test]
    fn base58_leading_zeros(zeros in 0usize..8, bytes in proptest::collection::vec(any::<u8>(), 0..8)) {
        let mut input = vec![0u8; zeros];
        input.extend(bytes);
        let text = base58::encode(&input);
        prop_assert_eq!(&text, &base58_by_division(&input));
        prop_assert_eq!(base58::decode(&text).unwrap(), input);
    }

    #[test]
    fn base58_check_round_trip(payload in proptest::collection::vec(any::<u8>(), 0..64)) {
        let text = base58::encode_check(&payload);
        prop_assert_eq!(base58::decode_check(&text).unwrap(), payload);
    }

    #[test]
    fn base58_check_detects_a_flipped_bit(
        payload in proptest::collection::vec(any::<u8>(), 1..40),
        position in any::<proptest::sample::Index>(),
        bit in 0u8..8,
    ) {
        let mut bytes = base58::decode(&base58::encode_check(&payload)).unwrap();
        let index = position.index(bytes.len());
        bytes[index] ^= 1 << bit;
        let decoded = base58::decode_check(&base58::encode(&bytes));
        prop_assert_eq!(decoded, Err(Base58Error::InvalidChecksum));
    }

    #[test]
    fn addresses_round_trip(bytes in any::<[u8; 21]>()) {
        let address = Address::from_bytes(bytes);
        let text = address.to_base58();
        prop_assert_eq!(Address::from_base58(&text), Ok(address));
        prop_assert_eq!(Address::from_base58_for(&text, bytes[0]), Ok(address));
        prop_assert!(Address::validate(&text, bytes[0]));
        prop_assert!(!Address::validate(&text, bytes[0].wrapping_add(1)));
    }

    #[test]
    fn derived_keys_parse(seed in any::<[u8; 32]>()) {
        if let Ok(secret) = SecretKey::from_bytes(seed) {
            let public = PublicKey::from_secret_key(&secret);
            prop_assert_eq!(public.as_bytes().len(), 33);
            prop_assert_eq!(PublicKey::from_bytes(public.as_bytes()), Ok(public.clone()));
            let compressed = public.to_compressed();
            prop_assert_eq!(compressed.as_bytes().as_slice(), public.as_bytes());
            prop_assert_eq!(SecretKey::from_hex(&secret.to_hex()).unwrap().to_hex(), secret.to_hex());
        }
    }

    #[test]
    fn decoders_do_not_panic(text in "\\PC{0,40}") {
        let _ = base58::decode_check(&text);
        let _ = Address::from_base58(&text);
        let _ = Address::validate(&text, 90);
        let _ = SecretKey::from_hex(&text);
        let _ = PublicKey::from_hex(&text);
    }
}
