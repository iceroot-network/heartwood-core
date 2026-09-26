//! The tree a relay signs: `{state: {height, forgingAllowed, currentSlot, header}, config:
//! {version, network: {version, name, nethash, explorer, token: {name, symbol}}, plugins}}`.

use heartwood_crypto::Block;

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
        let _ = block;
        todo!()
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
    let _ = (state, config);
    todo!()
}
