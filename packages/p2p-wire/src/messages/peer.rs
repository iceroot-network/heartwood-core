//! The messages of `peer.proto`: getPeers, getCommonBlocks and getStatus.

use std::collections::{BTreeMap, HashMap};

use heartwood_crypto::utils::js::is_array_index;

use super::shared::Headers;
use crate::js;
use crate::proto::message;

message! {
    /// The getPeers request.
    GetPeersRequest {
        /// The client's headers.
        1 headers "headers": message(Headers),
    }
}

message! {
    /// The getPeers response.
    GetPeersResponse {
        /// The peers.
        1 peers "peers": messages(Peer),
    }
}

message! {
    /// A peer of a getPeers response (`GetPeersResponse.Peer`).
    Peer {
        /// The peer's IP address.
        1 ip "ip": string,
        /// The peer's peer-to-peer port.
        2 port "port": u32,
    }
}

message! {
    /// The getCommonBlocks request.
    GetCommonBlocksRequest {
        /// Block ids, in hex.
        1 ids "ids": strings,
        /// The client's headers.
        2 headers "headers": message(Headers),
    }
}

message! {
    /// The getCommonBlocks response.
    GetCommonBlocksResponse {
        /// The highest block of the request's ids that the server has, if any.
        1 common "common": message(Common),
    }
}

message! {
    /// A common block (`GetCommonBlocksResponse.Common`).
    Common {
        /// The block's height.
        1 height "height": u32,
        /// The block's id, in hex.
        2 id "id": string,
    }
}

message! {
    /// The getStatus request.
    GetStatusRequest {
        /// The client's headers.
        1 headers "headers": message(Headers),
    }
}

message! {
    /// The getStatus response: the signed state and configuration, and the attestations.
    GetStatusResponse {
        /// The node's state.
        1 state "state": message(State),
        /// The node's configuration.
        2 config "config": message(Config),
        /// The public keys of the attestations, in hex.
        3 public_keys "publicKeys": strings,
        /// The attestations' signatures, in hex, one per public key.
        4 signatures "signatures": strings,
    }
}

message! {
    /// The state of a status (`GetStatusResponse.State`).
    State {
        /// The height of the node's last block.
        1 height "height": u32,
        /// Whether the node may forge in the current slot.
        2 forging_allowed "forgingAllowed": bool,
        /// The current slot; a negative slot before the network's epoch is written as its
        /// unsigned 32-bit value.
        3 current_slot "currentSlot": u32,
        /// The header of the node's last block.
        4 header "header": message(StatusHeader),
    }
}

message! {
    /// The block header of a status (`GetStatusResponse.State.BlockHeader`).
    StatusHeader {
        /// The block id, in hex.
        1 id "id": string,
        /// The block version.
        3 version "version": u32,
        /// Seconds since the network epoch.
        4 timestamp "timestamp": u32,
        /// The previous block's id, in hex.
        5 previous_block "previousBlock": string,
        /// The height.
        7 height "height": u32,
        /// The number of transactions.
        8 number_of_transactions "numberOfTransactions": u32,
        /// The sum of the transactions' amounts, as decimal text.
        9 total_amount "totalAmount": string,
        /// The sum of the transactions' fees, as decimal text.
        10 total_fee "totalFee": string,
        /// The block reward, as decimal text.
        11 reward "reward": string,
        /// The payload length.
        12 payload_length "payloadLength": u32,
        /// The payload hash, in hex.
        13 payload_hash "payloadHash": string,
        /// The generator's public key, in hex.
        14 generator_public_key "generatorPublicKey": string,
        /// The block signature, in hex.
        15 block_signature "blockSignature": string,
    }
}

message! {
    /// The configuration of a status (`GetStatusResponse.Config`).
    Config {
        /// The node's version.
        1 version "version": string,
        /// The network.
        2 network "network": message(Network),
        /// The node's public plugins, by package name.
        3 plugins "plugins": map(Plugins),
    }
}

message! {
    /// The network of a status (`GetStatusResponse.Config.Network`).
    Network {
        /// The network's name.
        1 name "name": string,
        /// The network's nethash, in hex.
        2 nethash "nethash": string,
        /// The explorer's URL.
        3 explorer "explorer": string,
        /// The token.
        4 token "token": message(Token),
        /// The address version byte (`pubKeyHash`).
        5 version "version": u32,
    }
}

message! {
    /// The token of a network (`GetStatusResponse.Config.Network.Token`).
    Token {
        /// The token's name.
        1 name "name": string,
        /// The token's symbol.
        2 symbol "symbol": string,
    }
}

message! {
    /// A public plugin of a status (`GetStatusResponse.Config.Plugin`).
    Plugin {
        /// The plugin's port.
        1 port "port": u32,
        /// Whether the plugin is enabled.
        2 enabled "enabled": bool,
        /// Whether the API estimates total counts.
        3 estimate_total_count "estimateTotalCount": bool,
    }
}

/// The `plugins` map of a status, as the reference holds it: a plain JavaScript object.
///
/// The reference decodes each map entry with `object[name] = value`, so the entries follow
/// JavaScript's rules: array-index names come first in numeric order, and other names in the order
/// they were first set; a repeated name keeps its first position and takes the last value; an entry
/// without a value is `None` (the reference's `null`). An entry named `__proto__` replaces the
/// object's prototype instead of adding an entry, and [`Plugins::replaced_prototype`] records it
/// (a documented difference: the reference's reply check then sees the prototype's properties).
/// Once an entry without a value has made the prototype `null`, `__proto__` is an ordinary name.
///
/// Setting and finding a name take logarithmic or constant time, so a payload of many entries
/// decodes in time proportional to its size, as the reference's object does.
///
/// Equality compares the entries, in order.
#[derive(Debug, Clone, Default)]
pub struct Plugins {
    /// The entries with an array-index name, by that index.
    indexed: BTreeMap<u32, (String, Option<Plugin>)>,
    /// The other entries, in the order their names were first set.
    named: Vec<(String, Option<Plugin>)>,
    /// The position of each name of `named`.
    positions: HashMap<String, usize>,
    prototype: Prototype,
}

/// The prototype of the reference's `plugins` object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Prototype {
    /// `Object.prototype`, as created.
    #[default]
    Object,
    /// A plugin: an entry named `__proto__` with a value.
    Plugin,
    /// `null`: an entry named `__proto__` without a value. The `__proto__` setter is then out of
    /// reach, and the name is an ordinary own key.
    Null,
}

/// The index of an array-index name: the canonical decimal text of 0 to 2^32 - 2.
fn array_index(name: &str) -> Option<u32> {
    is_array_index(name).then(|| name.parse().ok()).flatten()
}

impl Plugins {
    /// An empty map.
    pub fn new() -> Plugins {
        Plugins::default()
    }

    /// `object[name] = plugin`.
    pub fn insert(&mut self, name: String, plugin: Option<Plugin>) {
        if name == js::PROTO_KEY && self.prototype != Prototype::Null {
            self.prototype = match plugin {
                Some(_) => Prototype::Plugin,
                None => Prototype::Null,
            };
        } else if let Some(index) = array_index(&name) {
            self.indexed.entry(index).or_insert_with(|| (name, None)).1 = plugin;
        } else if let Some(entry) = self.position(&name).and_then(|at| self.named.get_mut(at)) {
            entry.1 = plugin;
        } else {
            self.positions.insert(name.clone(), self.named.len());
            self.named.push((name, plugin));
        }
    }

    /// The position in `named` of `name`.
    fn position(&self, name: &str) -> Option<usize> {
        self.positions.get(name).copied()
    }

    /// The entry named `name`: `None` when there is none, `Some(None)` when it has no value.
    pub fn get(&self, name: &str) -> Option<Option<&Plugin>> {
        let entry = match array_index(name) {
            Some(index) => self.indexed.get(&index),
            None => self.position(name).and_then(|at| self.named.get(at)),
        };
        entry.map(|(_, plugin)| plugin.as_ref())
    }

    /// `delete object[name]`: remove the entry named `name`, and return its value if it had one.
    pub fn remove(&mut self, name: &str) -> Option<Option<Plugin>> {
        if let Some(index) = array_index(name) {
            return self.indexed.remove(&index).map(|(_, plugin)| plugin);
        }
        let at = self.positions.remove(name)?;
        let (_, plugin) = self.named.remove(at);
        self.reindex();
        Some(plugin)
    }

    /// Keep the entries for which `keep` is true, in their order.
    pub fn retain(&mut self, mut keep: impl FnMut(&str, Option<&Plugin>) -> bool) {
        self.indexed
            .retain(|_, (name, plugin)| keep(name, plugin.as_ref()));
        self.named
            .retain(|(name, plugin)| keep(name, plugin.as_ref()));
        self.reindex();
    }

    /// Record the position of each name of `named` again.
    fn reindex(&mut self) {
        self.positions = self
            .named
            .iter()
            .enumerate()
            .map(|(at, (name, _))| (name.clone(), at))
            .collect();
    }

    /// The entries, in the object's own-key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Option<&Plugin>)> {
        self.indexed
            .values()
            .chain(&self.named)
            .map(|(name, plugin)| (name.as_str(), plugin.as_ref()))
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.indexed.len().saturating_add(self.named.len())
    }

    /// Whether there is no entry.
    pub fn is_empty(&self) -> bool {
        self.indexed.is_empty() && self.named.is_empty()
    }

    /// Whether an entry named `__proto__` replaced the prototype of the reference's object.
    pub fn replaced_prototype(&self) -> bool {
        self.prototype != Prototype::Object
    }

    /// Whether the reference's object has a `null` prototype and an own key `__proto__`, which
    /// its bytes must set up again: an entry named `__proto__` without a value first.
    pub(crate) fn has_own_proto_key(&self) -> bool {
        self.prototype == Prototype::Null && self.positions.contains_key(js::PROTO_KEY)
    }
}

impl PartialEq for Plugins {
    fn eq(&self, other: &Plugins) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for Plugins {}
