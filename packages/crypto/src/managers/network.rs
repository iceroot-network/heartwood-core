//! The network description, `network.json`.

use serde_json::{Map, Value, json};

use crate::errors::ConfigError;
use crate::utils::js;

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
    ///
    /// Every key of [`Network::to_json`] is required except `addressCharacter`, with a string or
    /// an integer that fits its field; integers are read as `JSON.parse` reads them, so `90.0` is
    /// 90. Other keys are ignored.
    pub fn load(json: &str) -> Result<Network, ConfigError> {
        let value: Value =
            serde_json::from_str(json).map_err(|error| ConfigError::Json(error.to_string()))?;
        let Value::Object(file) = value else {
            return Err(ConfigError::Shape(
                "the network description is not an object",
            ));
        };
        let bip32 = object(&file, "bip32")?;
        let client = object(&file, "client")?;
        Ok(Network {
            name: text(&file, "name", "name")?,
            message_prefix: text(&file, "messagePrefix", "messagePrefix")?,
            address_character: match file.get("addressCharacter") {
                None => None,
                Some(Value::String(character)) => Some(character.clone()),
                Some(_) => return Err(invalid("addressCharacter", "must be a string")),
            },
            bip32_public: integer(bip32, "public", "bip32.public")?,
            bip32_private: integer(bip32, "private", "bip32.private")?,
            pub_key_hash: integer(&file, "pubKeyHash", "pubKeyHash")?,
            nethash: text(&file, "nethash", "nethash")?,
            wif: integer(&file, "wif", "wif")?,
            slip44: integer(&file, "slip44", "slip44")?,
            token: text(client, "token", "client.token")?,
            symbol: text(client, "symbol", "client.symbol")?,
            explorer: text(client, "explorer", "client.explorer")?,
        })
    }

    /// The description as `network.json` holds it, with the keys in the order the file has them:
    /// `name`, `messagePrefix`, `addressCharacter` (when set), `bip32`, `pubKeyHash`, `nethash`,
    /// `wif`, `slip44`, `client`.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), json!(self.name));
        map.insert("messagePrefix".into(), json!(self.message_prefix));
        if let Some(character) = &self.address_character {
            map.insert("addressCharacter".into(), json!(character));
        }
        map.insert(
            "bip32".into(),
            json!({ "public": self.bip32_public, "private": self.bip32_private }),
        );
        map.insert("pubKeyHash".into(), json!(self.pub_key_hash));
        map.insert("nethash".into(), json!(self.nethash));
        map.insert("wif".into(), json!(self.wif));
        map.insert("slip44".into(), json!(self.slip44));
        map.insert(
            "client".into(),
            json!({ "token": self.token, "symbol": self.symbol, "explorer": self.explorer }),
        );
        Value::Object(map)
    }
}

fn invalid(key: &str, reason: &'static str) -> ConfigError {
    ConfigError::Network {
        key: key.to_owned(),
        reason,
    }
}

fn object<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, ConfigError> {
    match map.get(key) {
        Some(Value::Object(inner)) => Ok(inner),
        Some(_) => Err(invalid(key, "must be an object")),
        None => Err(invalid(key, "is missing")),
    }
}

fn text(map: &Map<String, Value>, key: &str, path: &str) -> Result<String, ConfigError> {
    match map.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(invalid(path, "must be a string")),
        None => Err(invalid(path, "is missing")),
    }
}

fn integer<T: TryFrom<i64>>(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<T, ConfigError> {
    let value = map.get(key).ok_or_else(|| invalid(path, "is missing"))?;
    js::safe_integer(value)
        .and_then(|integer| T::try_from(integer).ok())
        .ok_or_else(|| invalid(path, "must be an integer in the range of its field"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVNET: &str = r#"{
    "name": "devnet",
    "messagePrefix": "devnet message:\n",
    "addressCharacter": "d",
    "bip32": {
        "public": 70617039,
        "private": 70615956
    },
    "pubKeyHash": 90,
    "nethash": "c9b03ab996ef3ac216a2ac53eaee71118cbf7995fa44449a7fb7f94bbe18bcca",
    "wif": 252,
    "slip44": 1,
    "client": {
        "token": "dROOT",
        "symbol": "dRT",
        "explorer": ""
    }
}
"#;

    #[test]
    fn round_trip() {
        let network = Network::load(DEVNET).unwrap();
        assert_eq!(network.pub_key_hash, 90);
        assert_eq!(network.address_character.as_deref(), Some("d"));
        assert_eq!(js::stringify(&network.to_json(), 4) + "\n", DEVNET);
    }

    #[test]
    fn numbers_read_as_json_parse_reads_them() {
        let text = DEVNET.replace("\"pubKeyHash\": 90", "\"pubKeyHash\": 9.0e1");
        assert_eq!(Network::load(&text).unwrap().pub_key_hash, 90);
    }

    #[test]
    fn refusals() {
        let cases = [
            DEVNET.replace("\"pubKeyHash\": 90", "\"pubKeyHash\": 256"),
            DEVNET.replace("\"pubKeyHash\": 90", "\"pubKeyHash\": \"90\""),
            DEVNET.replace("\"wif\": 252", "\"wif\": 25.2"),
            DEVNET.replace("\"name\": \"devnet\",", ""),
            DEVNET.replace("\"addressCharacter\": \"d\"", "\"addressCharacter\": 1"),
            DEVNET.replace("\"explorer\": \"\"", "\"explorer\": null"),
            "[]".to_owned(),
            "{".to_owned(),
        ];
        for text in cases {
            assert!(Network::load(&text).is_err(), "{text}");
        }
        let without_character = DEVNET.replace("\"addressCharacter\": \"d\",\n    ", "");
        let network = Network::load(&without_character).unwrap();
        assert_eq!(network.address_character, None);
        assert_eq!(
            js::stringify(&network.to_json(), 4) + "\n",
            without_character
        );
    }
}
