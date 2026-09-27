//! The canonical status text read back to its tree, for the forger's checks before it signs.
//!
//! The text is the one [`super::signing_preimage`] writes: the kernel's key-sorted `stringify` of
//! `{state, config}` with no white space. The reader accepts exactly that shape (the four state
//! keys, the 13 header keys, the configuration's keys and its plugins, nothing more) and then
//! writes the tree again: text that does not come back byte for byte is refused, so a forger
//! never signs bytes whose meaning it did not read.

use serde_json::{Map, Value};

use super::tree::{
    NetworkConfig, PeerConfig, PluginConfig, SignedState, StatusHeaderData, TokenConfig,
    signing_preimage,
};
use crate::errors::PreimageError;

/// The keys of the state, of its header, of the configuration, of its network and token, and
/// those a plugin may have, as the canonical text lists them.
const STATE: &[&str] = &["currentSlot", "forgingAllowed", "header", "height"];
#[rustfmt::skip]
const HEADER: &[&str] = &[
    "blockSignature", "generatorPublicKey", "height", "id", "numberOfTransactions", "payloadHash",
    "payloadLength", "previousBlock", "reward", "timestamp", "totalAmount", "totalFee", "version",
];
const CONFIG: &[&str] = &["network", "plugins", "version"];
const NETWORK: &[&str] = &["explorer", "name", "nethash", "token", "version"];
const TOKEN: &[&str] = &["name", "symbol"];
const PLUGIN: &[&str] = &["enabled", "estimateTotalCount", "port"];

/// Read `bytes`, an offered status text, as the tree it is the canonical form of.
///
/// Refused: text that is not UTF-8 JSON ([`PreimageError::NotJson`]); a missing or extra key, or
/// a value of the wrong type or out of range ([`PreimageError::Shape`]); a tree that
/// [`super::signing_preimage`] writes as other bytes ([`PreimageError::NotCanonical`]).
pub fn parse_preimage(bytes: &[u8]) -> Result<(SignedState, PeerConfig), PreimageError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| PreimageError::NotJson)?;
    let root = Node::root(&value, &["config", "state"])?;
    let state = root.object("state", STATE)?;
    let header = state.object("header", HEADER)?;
    let signed = SignedState {
        height: state.integer("height")?,
        forging_allowed: state.flag("forgingAllowed")?,
        current_slot: state.integer("currentSlot")?,
        header: StatusHeaderData {
            id: header.text("id")?,
            version: header.integer("version")?,
            timestamp: header.integer("timestamp")?,
            previous_block: header.text("previousBlock")?,
            height: header.integer("height")?,
            number_of_transactions: header.integer("numberOfTransactions")?,
            total_amount: header.amount("totalAmount")?,
            total_fee: header.amount("totalFee")?,
            reward: header.amount("reward")?,
            payload_length: header.integer("payloadLength")?,
            payload_hash: header.text("payloadHash")?,
            generator_public_key: header.text("generatorPublicKey")?,
            block_signature: header.text("blockSignature")?,
        },
    };
    let config = root.object("config", CONFIG)?;
    let network = config.object("network", NETWORK)?;
    let token = network.object("token", TOKEN)?;
    let plugins = config.field("plugins")?;
    let entries = plugins.0.as_object().ok_or_else(|| plugins.1.shape())?;
    let plugins = (entries.iter())
        .map(|(name, value)| {
            let plugin = Node::new(value, plugins.1.child(name), PLUGIN, &["enabled", "port"])?;
            let estimate_total_count = match plugin.map.get("estimateTotalCount") {
                None => None,
                Some(_) => Some(plugin.flag("estimateTotalCount")?),
            };
            let config = PluginConfig {
                enabled: plugin.flag("enabled")?,
                port: plugin.integer("port")?,
                estimate_total_count,
            };
            Ok((name.clone(), config))
        })
        .collect::<Result<_, PreimageError>>()?;
    let config = PeerConfig {
        version: config.text("version")?,
        network: NetworkConfig {
            version: network.integer("version")?,
            name: network.text("name")?,
            nethash: network.text("nethash")?,
            explorer: network.text("explorer")?,
            token: TokenConfig {
                name: token.text("name")?,
                symbol: token.text("symbol")?,
            },
        },
        plugins,
    };
    if signing_preimage(&signed, &config) != bytes {
        return Err(PreimageError::NotCanonical);
    }
    Ok((signed, config))
}

/// The dotted path of a value in the tree, for [`PreimageError::Shape`].
#[derive(Debug, Clone)]
struct Path(String);

impl Path {
    fn child(&self, key: &str) -> Path {
        match self.0.as_str() {
            "" => Path(key.to_owned()),
            parent => Path(format!("{parent}.{key}")),
        }
    }

    fn shape(&self) -> PreimageError {
        PreimageError::Shape {
            path: match self.0.as_str() {
                "" => "status".to_owned(),
                path => path.to_owned(),
            },
        }
    }
}

/// An object of the tree whose keys were checked: every required key present, no other key.
struct Node<'a> {
    map: &'a Map<String, Value>,
    path: Path,
}

impl<'a> Node<'a> {
    fn root(value: &'a Value, keys: &[&str]) -> Result<Node<'a>, PreimageError> {
        Node::new(value, Path(String::new()), keys, keys)
    }

    /// `value` at `path` as an object whose keys are among `allowed`, with every one of
    /// `required`.
    fn new(
        value: &'a Value,
        path: Path,
        allowed: &[&str],
        required: &[&str],
    ) -> Result<Node<'a>, PreimageError> {
        let map = value.as_object().ok_or_else(|| path.shape())?;
        if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(path.child(key).shape());
        }
        if let Some(key) = required.iter().find(|key| !map.contains_key(**key)) {
            return Err(path.child(key).shape());
        }
        Ok(Node { map, path })
    }

    fn field(&self, key: &str) -> Result<(&'a Value, Path), PreimageError> {
        let path = self.path.child(key);
        let value = self.map.get(key).ok_or_else(|| path.shape())?;
        Ok((value, path))
    }

    fn object(&self, key: &str, keys: &[&str]) -> Result<Node<'a>, PreimageError> {
        let (value, path) = self.field(key)?;
        Node::new(value, path, keys, keys)
    }

    fn text(&self, key: &str) -> Result<String, PreimageError> {
        let (value, path) = self.field(key)?;
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| path.shape())
    }

    fn flag(&self, key: &str) -> Result<bool, PreimageError> {
        let (value, path) = self.field(key)?;
        value.as_bool().ok_or_else(|| path.shape())
    }

    /// An integer that fits `T`.
    fn integer<T: TryFrom<i64>>(&self, key: &str) -> Result<T, PreimageError> {
        let (value, path) = self.field(key)?;
        (value.as_i64())
            .and_then(|n| T::try_from(n).ok())
            .ok_or_else(|| path.shape())
    }

    /// An amount: decimal text of a 64-bit unsigned integer.
    fn amount(&self, key: &str) -> Result<u64, PreimageError> {
        let (value, path) = self.field(key)?;
        (value.as_str())
            .filter(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| path.shape())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference's preimage of the block at height 2 of its status vectors, at slot 1.
    const PREIMAGE: &str = concat!(
        r#"{"config":{"network":{"explorer":"","name":"devnet","nethash":"#,
        r#""0000000000000000000000000000000000000000000000000000000000000000","#,
        r#""token":{"name":"dROOT","symbol":"dRT"},"version":90},"plugins":{"@solar-network/api":"#,
        r#"{"enabled":true,"estimateTotalCount":true,"port":6003}},"version":"4.3.1"},"state":"#,
        r#"{"currentSlot":1,"forgingAllowed":true,"header":{"blockSignature":"#,
        r#""d812e107620705e0ecad4d8cad23327134749dabe83074d54cd8b6ffd1ef3b20"#,
        r#"86b1ecfdb2cc167981c23a3fbb5ff23eded9f2c50ac24c41ccc48870d1625f23","#,
        r#""generatorPublicKey":"02dcb90a589d24e2d7e836ccc299f11d3f015ef86e208418d8b3d6de614a02cccd","#,
        r#""height":2,"id":"e5591e2ae5f063b1644206f825fa83e225a59ff81d96d4f01ed71ec0401c03b6","#,
        r#""numberOfTransactions":0,"#,
        r#""payloadHash":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","#,
        r#""payloadLength":0,"previousBlock":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","#,
        r#""reward":"180000000","timestamp":8,"totalAmount":"0","totalFee":"0","version":0},"#,
        r#""height":2}}"#,
    );

    fn shape(path: &str) -> PreimageError {
        PreimageError::Shape { path: path.into() }
    }

    #[test]
    fn the_canonical_text_reads_back_to_its_tree() {
        let (state, config) = parse_preimage(PREIMAGE.as_bytes()).unwrap();
        assert_eq!((state.height, state.current_slot), (2, 1));
        assert!(state.forging_allowed);
        assert_eq!(state.header.reward, 180_000_000);
        assert_eq!(state.header.timestamp, 8);
        assert_eq!(config.network.nethash, "00".repeat(32));
        assert_eq!(config.network.version, 90);
        assert_eq!(
            config.plugins,
            [(
                "@solar-network/api".to_owned(),
                PluginConfig {
                    enabled: true,
                    port: 6003,
                    estimate_total_count: Some(true),
                }
            )]
        );
        assert_eq!(signing_preimage(&state, &config), PREIMAGE.as_bytes());
    }

    #[test]
    fn a_negative_slot_and_no_plugins_read_back() {
        let text = PREIMAGE
            .replace(r#""currentSlot":1"#, r#""currentSlot":-3"#)
            .replace(
                r#"{"@solar-network/api":{"enabled":true,"estimateTotalCount":true,"port":6003}}"#,
                "{}",
            );
        let (state, config) = parse_preimage(text.as_bytes()).unwrap();
        assert_eq!(state.current_slot, -3);
        assert!(config.plugins.is_empty());
        let other = text.replace("{}", r#"{"p":{"enabled":false,"port":1}}"#);
        let (_, config) = parse_preimage(other.as_bytes()).unwrap();
        assert_eq!(config.plugins[0].1.estimate_total_count, None);
    }

    #[test]
    fn text_that_is_not_the_canonical_form_is_refused() {
        let cases = [
            // White space, another key order, an escape the writer does not use, leading zeros.
            PREIMAGE.replacen(':', ": ", 1),
            PREIMAGE.replacen(r#"{"config":"#, r#"{ "config":"#, 1),
            PREIMAGE.replace(r#""name":"devnet""#, r#""name":"dev\u006eet""#),
            PREIMAGE.replace(r#""reward":"180000000""#, r#""reward":"0180000000""#),
            PREIMAGE.replace(r#""timestamp":8"#, r#""timestamp":8,"timestamp":8"#),
            PREIMAGE.replace(
                r#""explorer":"","name":"devnet""#,
                r#""name":"devnet","explorer":"""#,
            ),
            // A plugin named like a prototype property, which the writer leaves out.
            PREIMAGE.replace(
                r#""plugins":{"#,
                r#""plugins":{"constructor":{"enabled":true,"port":1},"#,
            ),
        ];
        for text in cases {
            assert_eq!(
                parse_preimage(text.as_bytes()),
                Err(PreimageError::NotCanonical),
                "{text}"
            );
        }
    }

    #[test]
    fn a_tree_of_another_shape_is_refused_with_its_path() {
        let cases = [
            ("not json", PreimageError::NotJson),
            ("[]", shape("status")),
            (r#"{"state":{}}"#, shape("config")),
            (
                &PREIMAGE.replace(r#""height":2}}"#, r#""height":2,"x":1}}"#),
                shape("state.x"),
            ),
            (
                &PREIMAGE.replace(r#""height":2,"id""#, r#""id""#),
                shape("state.header.height"),
            ),
            (
                &PREIMAGE.replace(r#""timestamp":8"#, r#""timestamp":8.5"#),
                shape("state.header.timestamp"),
            ),
            (
                &PREIMAGE.replace(r#""timestamp":8"#, r#""timestamp":4294967296"#),
                shape("state.header.timestamp"),
            ),
            (
                &PREIMAGE.replace(r#""reward":"180000000""#, r#""reward":180000000"#),
                shape("state.header.reward"),
            ),
            (
                &PREIMAGE.replace(r#""totalFee":"0""#, r#""totalFee":"18446744073709551616""#),
                shape("state.header.totalFee"),
            ),
            (
                &PREIMAGE.replace(r#""version":90"#, r#""version":256"#),
                shape("config.network.version"),
            ),
            (
                &PREIMAGE.replace(r#""port":6003"#, r#""port":6003,"x":true"#),
                shape("config.plugins.@solar-network/api.x"),
            ),
            (
                &PREIMAGE.replace(r#""enabled":true,"#, ""),
                shape("config.plugins.@solar-network/api.enabled"),
            ),
            (
                &PREIMAGE.replace(r#""forgingAllowed":true"#, r#""forgingAllowed":1"#),
                shape("state.forgingAllowed"),
            ),
        ];
        for (text, error) in cases {
            assert_eq!(parse_preimage(text.as_bytes()), Err(error), "{text}");
        }
    }
}
