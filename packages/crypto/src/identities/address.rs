//! Addresses: a network byte followed by the RIPEMD-160 of the public key bytes, shown as
//! Base58Check.

use std::fmt;

use crate::crypto::hash::ripemd160;
use crate::errors::AddressError;
use crate::identities::PublicKey;
use crate::utils::base58;

/// An address: network byte and 20-byte key hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address {
    bytes: [u8; 21],
}

impl Address {
    /// The address of `public_key` on the network with byte `network`. The hash covers the key in
    /// its original encoding.
    pub fn from_public_key(public_key: &PublicKey, network: u8) -> Address {
        let mut bytes = [0u8; 21];
        bytes[0] = network;
        bytes[1..].copy_from_slice(&ripemd160(public_key.as_bytes()));
        Address { bytes }
    }

    /// The address with these 21 bytes (network byte first), for any network byte.
    pub const fn from_bytes(bytes: [u8; 21]) -> Address {
        Address { bytes }
    }

    /// The address in the Base58Check `text`, whose payload must be 21 bytes. The network byte is
    /// not checked; see [`Address::network`].
    pub fn from_base58(text: &str) -> Result<Address, AddressError> {
        let payload = base58::decode_check(text)?;
        let actual = payload.len();
        <[u8; 21]>::try_from(payload)
            .map(Address::from_bytes)
            .map_err(|_| AddressError::InvalidLength { actual })
    }

    /// The address in `text` if it belongs to the network with byte `network`.
    pub fn from_base58_for(text: &str, network: u8) -> Result<Address, AddressError> {
        let address = Address::from_base58(text)?;
        if address.network() == network {
            Ok(address)
        } else {
            Err(AddressError::WrongNetwork {
                expected: network,
                actual: address.network(),
            })
        }
    }

    /// The reference's `Address.validate`: whether `text` is Base58Check whose first payload byte
    /// is `network`. The payload length is not checked, as in the reference.
    pub fn validate(text: &str, network: u8) -> bool {
        base58::decode_check(text).is_ok_and(|payload| payload.first() == Some(&network))
    }

    /// The network byte.
    pub const fn network(&self) -> u8 {
        self.bytes[0]
    }

    /// The 21 bytes.
    pub const fn as_bytes(&self) -> &[u8; 21] {
        &self.bytes
    }

    /// The Base58Check text.
    pub fn to_base58(&self) -> String {
        base58::encode_check(&self.bytes)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base58())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::Base58Error;

    const KEY: &str = "034151a3ec46b5670a682b0a63394f863587d1bc97483b1b6c70eb58e7f0aed192";

    #[test]
    fn round_trip() {
        let key = PublicKey::from_hex(KEY).unwrap();
        let address = Address::from_public_key(&key, 90);
        assert_eq!(address.to_string(), "dEHxjxZybRiykTqqZUgfQoMqZZ3RxVj1dt");
        assert_eq!(address.network(), 90);
        assert_eq!(Address::from_base58(&address.to_base58()), Ok(address));
        assert_eq!(
            Address::from_base58_for(&address.to_base58(), 90),
            Ok(address)
        );
        assert_eq!(
            Address::from_base58_for(&address.to_base58(), 63),
            Err(AddressError::WrongNetwork {
                expected: 63,
                actual: 90
            })
        );
    }

    #[test]
    fn payload_length() {
        let short = base58::encode_check(&[90; 20]);
        assert!(Address::validate(&short, 90));
        assert_eq!(
            Address::from_base58(&short),
            Err(AddressError::InvalidLength { actual: 20 })
        );
        let empty = base58::encode_check(&[]);
        assert!(!Address::validate(&empty, 90));
        assert_eq!(
            Address::from_base58(""),
            Err(AddressError::Base58(Base58Error::TooShort))
        );
    }
}
