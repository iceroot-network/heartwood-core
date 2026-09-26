//! Keys and addresses.
//!
//! - [`SecretKey`] and [`KeyPair`]: passphrase keys (the SHA-256 of the passphrase's UTF-8 bytes).
//! - [`PublicKey`]: a key that parses with the reference's acceptance set, in its original encoding.
//! - [`PublicKeyBytes`]: the 33 bytes of a public-key field that the reference does not check.
//! - [`Address`]: network byte and RIPEMD-160 of the key bytes, shown as Base58Check.

pub mod address;
pub mod keys;
pub mod public_key;

pub use self::address::Address;
pub use self::keys::{KeyPair, SecretKey};
pub use self::public_key::{PublicKey, PublicKeyBytes};
