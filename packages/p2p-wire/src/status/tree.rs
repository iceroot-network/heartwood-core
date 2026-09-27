//! The tree a relay signs: `{state: {height, forgingAllowed, currentSlot, header}, config:
//! {version, network: {version, name, nethash, explorer, token: {name, symbol}}, plugins}}`.

use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockHeader, BlockId};

use super::json;
use crate::codecs::Amount;
use crate::messages::peer::{Config, Network, Plugin, Plugins, State, StatusHeader, Token};

/// The signed state: the node's last block and slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedState {
    /// The height of the last block.
    pub height: u32,
    /// Whether the node may forge in the current slot.
    pub forging_allowed: bool,
    /// The current slot, negative before the network's epoch.
    pub current_slot: i64,
    /// The last block's header.
    pub header: StatusHeaderData,
}

/// The header of the last block as the reference signs it (`getHeader(false)`): 13 keys, the
/// amounts as decimal text in the preimage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusHeaderData {
    /// The block id, in hex.
    pub id: String,
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The previous block's id, in hex.
    pub previous_block: String,
    /// The height.
    pub height: u32,
    /// The number of transactions.
    pub number_of_transactions: u32,
    /// The sum of the transactions' amounts.
    pub total_amount: u64,
    /// The sum of the transactions' fees.
    pub total_fee: u64,
    /// The block reward.
    pub reward: u64,
    /// The payload length.
    pub payload_length: u32,
    /// The payload hash, in hex.
    pub payload_hash: String,
    /// The generator's public key, in hex.
    pub generator_public_key: String,
    /// The block signature, in hex.
    pub block_signature: String,
}

impl StatusHeaderData {
    /// The header of `block` as `getHeader(false)` gives it.
    pub fn from_block(block: &Block) -> StatusHeaderData {
        StatusHeaderData::with_id(block.header(), block.id())
    }

    /// The header of a block from its decoded header alone: the same as
    /// [`StatusHeaderData::from_block`], with the id computed from the header.
    pub fn from_header(header: &BlockHeader) -> StatusHeaderData {
        StatusHeaderData::with_id(header, &serialiser::block_id(header))
    }

    fn with_id(header: &BlockHeader, id: &BlockId) -> StatusHeaderData {
        StatusHeaderData {
            id: id.to_hex(),
            version: header.version,
            timestamp: header.timestamp,
            previous_block: hex::encode(&header.previous_block),
            height: header.height,
            number_of_transactions: header.number_of_transactions,
            total_amount: header.total_amount,
            total_fee: header.total_fee,
            reward: header.reward,
            payload_length: header.payload_length,
            payload_hash: hex::encode(&header.payload_hash),
            generator_public_key: header.generator_public_key.to_hex(),
            block_signature: header.block_signature.to_hex(),
        }
    }

    /// The three amounts, in the order of the header.
    fn amounts(&self) -> [Amount; 3] {
        [self.total_amount, self.total_fee, self.reward].map(Amount::from)
    }
}

/// The node's public configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConfig {
    /// The node's version.
    pub version: String,
    /// The network.
    pub network: NetworkConfig,
    /// The public plugins by package name, in the configuration's order. The wire and the
    /// preimage see them as a JavaScript object: array-index names first, a repeated name once.
    /// A name `__proto__`, which no package can have, is left out; the reference would replace
    /// its object's prototype and sign the prototype's fields as entries.
    pub plugins: Vec<(String, PluginConfig)>,
}

/// The network of a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    /// The address version byte (`pubKeyHash`).
    pub version: u8,
    /// The network's name.
    pub name: String,
    /// The network's nethash, in hex.
    pub nethash: String,
    /// The explorer's URL.
    pub explorer: String,
    /// The token.
    pub token: TokenConfig,
}

/// The token of a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenConfig {
    /// The token's name.
    pub name: String,
    /// The token's symbol.
    pub symbol: String,
}

/// A public plugin of a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginConfig {
    /// Whether the plugin is enabled.
    pub enabled: bool,
    /// The plugin's port.
    pub port: u32,
    /// Whether the API estimates total counts: present for API plugins only.
    pub estimate_total_count: Option<bool>,
}

/// The bytes a relay signs: the canonical JSON of `{state, config}`, as UTF-8.
///
/// An empty `plugins` is signed as `"plugins":{}`, although every verifier leaves it out, so such
/// a node's attestations never verify (as in the reference).
pub fn signing_preimage(state: &SignedState, config: &PeerConfig) -> Vec<u8> {
    json::preimage(
        &state_message(state),
        Some(state.current_slot),
        state.header.amounts(),
        &config_message(config),
        true,
    )
}

/// The state as the wire carries it: every field present, the slot as its unsigned 32-bit value
/// (JavaScript's `ToUint32`), the amounts as decimal text.
pub(super) fn state_message(state: &SignedState) -> State {
    let header = &state.header;
    let [total_amount, total_fee, reward] = header.amounts().map(|amount| Some(amount.to_string()));
    State {
        height: Some(state.height),
        forging_allowed: Some(state.forging_allowed),
        // Truncation to the low 32 bits is ToUint32 for an integer.
        current_slot: Some(state.current_slot as u32),
        header: Some(StatusHeader {
            id: Some(header.id.clone()),
            version: Some(header.version),
            timestamp: Some(header.timestamp),
            previous_block: Some(header.previous_block.clone()),
            height: Some(header.height),
            number_of_transactions: Some(header.number_of_transactions),
            total_amount,
            total_fee,
            reward,
            payload_length: Some(header.payload_length),
            payload_hash: Some(header.payload_hash.clone()),
            generator_public_key: Some(header.generator_public_key.clone()),
            block_signature: Some(header.block_signature.clone()),
            ..StatusHeader::default()
        }),
        ..State::default()
    }
}

/// The configuration as the wire carries it: the plugins as a JavaScript object built by
/// assigning each entry in the configuration's order.
pub(super) fn config_message(config: &PeerConfig) -> Config {
    let network = &config.network;
    let mut plugins = Plugins::new();
    for (name, plugin) in &config.plugins {
        plugins.insert(
            name.clone(),
            Some(Plugin {
                port: Some(plugin.port),
                enabled: Some(plugin.enabled),
                estimate_total_count: plugin.estimate_total_count,
                ..Plugin::default()
            }),
        );
    }
    Config {
        version: Some(config.version.clone()),
        network: Some(Network {
            name: Some(network.name.clone()),
            nethash: Some(network.nethash.clone()),
            explorer: Some(network.explorer.clone()),
            token: Some(Token {
                name: Some(network.token.name.clone()),
                symbol: Some(network.token.symbol.clone()),
                ..Token::default()
            }),
            version: Some(u32::from(network.version)),
            ..Network::default()
        }),
        plugins,
        ..Config::default()
    }
}
