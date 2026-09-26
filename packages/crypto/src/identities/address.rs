//! Addresses: a network byte followed by the RIPEMD-160 of the public key bytes, shown as
//! Base58Check.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use std::fmt;

use crate::errors::AddressError;
use crate::identities::PublicKey;

/// An address: network byte and 20-byte key hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address {
    bytes: [u8; 21],
}

impl Address {
    /// The address of `public_key` on the network with byte `network`. The hash covers the key in
    /// its original encoding.
    pub fn from_public_key(public_key: &PublicKey, network: u8) -> Address {
        todo!()
    }

    /// The address with these 21 bytes (network byte first), for any network byte.
    pub const fn from_bytes(bytes: [u8; 21]) -> Address {
        Address { bytes }
    }

    /// The address in the Base58Check `text`, whose payload must be 21 bytes. The network byte is
    /// not checked; see [`Address::network`].
    pub fn from_base58(text: &str) -> Result<Address, AddressError> {
        todo!()
    }

    /// The address in `text` if it belongs to the network with byte `network`.
    pub fn from_base58_for(text: &str, network: u8) -> Result<Address, AddressError> {
        todo!()
    }

    /// The reference's `Address.validate`: whether `text` is Base58Check whose first payload byte
    /// is `network`. The payload length is not checked, as in the reference.
    pub fn validate(text: &str, network: u8) -> bool {
        todo!()
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
        todo!()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base58())
    }
}
