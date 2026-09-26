//! Fuzz the transaction decoder: arbitrary bytes must decode or be refused without a panic, and a
//! decoded transaction must have a canonical form that is a fixed point of the codec.
//!
//! The canonical form is the decoded fields serialised again. It decodes to the same verdict,
//! serialises to itself, has its SHA-256 as its id, and is what the JSON form reads back to.

#![no_main]

use std::sync::LazyLock;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::transactions::serialiser::{SerialiseOptions, serialise};
use heartwood_crypto::{Milestones, Network, Params, Transaction};
use libfuzzer_sys::fuzz_target;

/// The transaction rules of the golden vectors' devnet: network byte 90, 150 transactions and
/// 2,097,152 bytes per block, 1 to 256 transfer items.
const MILESTONES: &str = r#"[{
    "height": 1,
    "activeDelegates": 53,
    "blockTime": 8,
    "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2097152 },
    "burn": { "feeBasisPoints": 9000, "txAmount": 2000000 },
    "epoch": "2026-01-01T00:00:00.000Z",
    "transfer": { "maximum": 256, "minimum": 1 },
    "reward": 0,
    "dynamicReward": { "enabled": false },
    "donations": {}
}]"#;

static PARAMS: LazyLock<Params> = LazyLock::new(|| {
    let network = Network {
        name: "devnet".into(),
        message_prefix: "devnet message:\n".into(),
        address_character: Some("d".into()),
        bip32_public: 70_617_039,
        bip32_private: 70_615_956,
        pub_key_hash: 90,
        nethash: "0".repeat(64),
        wif: 252,
        slip44: 1,
        token: "dROOT".into(),
        symbol: "dRT".into(),
        explorer: String::new(),
    };
    Milestones::load(MILESTONES, &network)
        .expect("the fuzz milestones load")
        .at(1)
        .clone()
});

fuzz_target!(|bytes: &[u8]| {
    let params = &*PARAMS;
    let _ = Transaction::decode_trusted(bytes, params);
    let Ok(tx) = Transaction::decode(bytes, params) else {
        return;
    };
    let canonical = serialise(tx.data(), SerialiseOptions::FULL, params)
        .expect("a decoded transaction serialises");
    let again = Transaction::decode(&canonical, params).expect("the canonical form decodes");
    assert_eq!(again.bytes(), canonical.as_slice());
    assert_eq!(
        serialise(again.data(), SerialiseOptions::FULL, params).expect("it serialises"),
        canonical
    );
    assert_eq!(again.id().as_bytes(), &sha256(&canonical));
    assert_eq!(again.is_verified(), tx.is_verified());
    let json = Transaction::from_json(&tx.to_json(params), params).expect("the JSON form reads");
    assert_eq!(json.bytes(), canonical.as_slice());
});
