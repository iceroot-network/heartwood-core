//! The network description, `network.json`.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::ConfigError;

/// The network description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    /// The network name.
    pub name: String,
    /// The prefix of signed messages.
    pub message_prefix: String,
    /// The first character of the network's addresses, when the file states it.
    pub address_character: Option<String>,
    /// BIP32 version bytes of public extended keys.
    pub bip32_public: u32,
    /// BIP32 version bytes of private extended keys.
    pub bip32_private: u32,
    /// The address network byte.
    pub pub_key_hash: u8,
    /// The network hash: the genesis block's payload hash, as 64 hex digits.
    pub nethash: String,
    /// The WIF version byte.
    pub wif: u8,
    /// The SLIP-44 coin type.
    pub slip44: u32,
    /// The token name shown by clients.
    pub token: String,
    /// The token symbol shown by clients.
    pub symbol: String,
    /// The explorer URL shown by clients.
    pub explorer: String,
}

impl Network {
    /// Read `network.json`.
    pub fn load(json: &str) -> Result<Network, ConfigError> {
        todo!()
    }

    /// The description as `network.json` holds it, with the keys in the order the file has them:
    /// `name`, `messagePrefix`, `addressCharacter` (when set), `bip32`, `pubKeyHash`, `nethash`,
    /// `wif`, `slip44`, `client`.
    pub fn to_json(&self) -> serde_json::Value {
        todo!()
    }
}
