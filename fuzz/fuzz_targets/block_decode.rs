//! The block decoder on arbitrary bytes: no input may panic it, and a block it accepts encodes to
//! bytes that decode to the same block.

#![no_main]

use std::sync::LazyLock;

use heartwood_crypto::{Block, Milestones, Network};
use libfuzzer_sys::fuzz_target;

const NETWORK: &str = r#"{
    "name": "fuzz",
    "messagePrefix": "fuzz message:\n",
    "bip32": { "public": 70617039, "private": 70615956 },
    "pubKeyHash": 90,
    "nethash": "0000000000000000000000000000000000000000000000000000000000000000",
    "wif": 252,
    "slip44": 1,
    "client": { "token": "FUZZ", "symbol": "FZ", "explorer": "" }
}"#;

/// Two milestones, so that inputs decode under the genesis rules of height 1, the rules of the
/// first milestone, and a later milestone with a smaller transfer limit.
const MILESTONES: &str = include_str!("../milestones/block_decode.json");

static RULES: LazyLock<Milestones> = LazyLock::new(|| {
    let network = Network::load(NETWORK).expect("the fuzz network loads");
    Milestones::load(MILESTONES, &network).expect("the fuzz milestones load")
});

fuzz_target!(|bytes: &[u8]| {
    if let Ok(block) = Block::decode(bytes, &RULES) {
        let _ = block.verify(&RULES, 0);
        let _ = block.to_json(&RULES);
        let again = Block::decode(&block.encode(), &RULES).expect("an accepted block decodes again");
        assert_eq!(again, block);
    }
    let _ = Block::decode_trusted(bytes, &RULES);
});
