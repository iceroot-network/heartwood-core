use heartwood_crypto::utils::js::stringify;
use serde_json::Value;

use super::*;
use crate::status::{
    NetworkConfig, PeerConfig, PluginConfig, SignedState, StatusHeaderData, TokenConfig,
    signing_preimage,
};

#[test]
fn utf16_lengths_order_and_cuts() {
    assert_eq!(utf16_len("a\u{e9}\u{1F600}"), 4);
    assert_eq!(utf16_cmp("\u{FF61}", "\u{1F600}"), Ordering::Greater);
    assert_eq!(utf16_cmp("10", "9"), Ordering::Less);
    assert_eq!(utf16_split("abc", 2), ("ab".into(), "c".into()));
    assert_eq!(utf16_split("abc", 5), ("abc".into(), String::new()));
    assert_eq!(
        utf16_split("a\u{1F600}b", 2),
        ("a\u{FFFD}".into(), "\u{FFFD}b".into())
    );
    assert_eq!(
        utf16_split("a\u{1F600}b", 3),
        ("a\u{1F600}".into(), "b".into())
    );
    assert_eq!(utf8_lossy(&[0x61, 0xff, 0xfe]), "a\u{FFFD}\u{FFFD}");
}

/// The status preimage of a configuration whose version is `version` and whose one plugin is
/// named `plugin`.
fn preimage(version: &str, plugin: &str) -> String {
    let header = StatusHeaderData {
        id: "1".repeat(64),
        version: 0,
        timestamp: 1,
        previous_block: "2".repeat(64),
        height: 2,
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 0,
        payload_hash: "3".repeat(64),
        generator_public_key: "4".repeat(66),
        block_signature: "5".repeat(128),
    };
    let state = SignedState {
        height: 2,
        forging_allowed: true,
        current_slot: 1,
        header,
    };
    let token = TokenConfig {
        name: "t".into(),
        symbol: "T".into(),
    };
    let network = NetworkConfig {
        version: 30,
        name: "n".into(),
        nethash: "6".repeat(64),
        explorer: String::new(),
        token,
    };
    let port = PluginConfig {
        enabled: true,
        port: 4003,
        estimate_total_count: None,
    };
    let config = PeerConfig {
        version: version.to_owned(),
        network,
        plugins: vec![(plugin.to_owned(), port)],
    };
    String::from_utf8(signing_preimage(&state, &config)).expect("UTF-8")
}

#[test]
fn the_status_json_writes_every_code_point_below_u0080_as_stringify_does() {
    let plain = preimage("VERSION", "PLUGIN");
    let quoted = |text: &str| stringify(&Value::String(text.to_owned()), 0);
    for c in (0..0x80).filter_map(char::from_u32) {
        let text = format!("a{c}z");
        let expected = (plain.replacen("\"VERSION\"", &quoted(&text), 1)).replacen(
            "\"PLUGIN\"",
            &quoted(&text),
            1,
        );
        assert_eq!(preimage(&text, &text), expected, "U+{:04X}", u32::from(c));
    }
}
