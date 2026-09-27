//! A getBlocks reply on arbitrary bytes through the client's download path: the codec, the reply
//! check, the transactions read as the reference's `fromBytesUnsafe` reads them, and
//! `heartwood_p2p::to_block`, with the transactions and with headers only. No input may panic it,
//! and a block it rebuilds decodes again to the same block; that it accepts exactly what the
//! reference's path accepts is checked by the replay against `block.fromListed`.

#![no_main]

use std::net::IpAddr;
use std::sync::LazyLock;

use heartwood_crypto::{Block, Milestones, Network};
use heartwood_p2p::download::read_listed;
use heartwood_p2p::to_block;
use heartwood_p2p_wire::codecs::blocks::get_blocks_decode_response;
use heartwood_p2p_wire::replies::check_blocks;
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

/// The block decoder's milestones: genesis rules at height 1, and a later milestone.
const MILESTONES: &str = include_str!("../milestones/block_decode.json");

static RULES: LazyLock<Milestones> = LazyLock::new(|| {
    let network = Network::load(NETWORK).expect("the fuzz network loads");
    Milestones::load(MILESTONES, &network).expect("the fuzz milestones load")
});

fuzz_target!(|bytes: &[u8]| {
    let Ok(mut listed) = get_blocks_decode_response(bytes) else {
        return;
    };
    if check_blocks(&mut listed).is_err() {
        return;
    }
    let from = IpAddr::from([192, 0, 2, 1]);
    for block in listed {
        let params = RULES.at(block.header.height.unwrap_or(0));
        let headers = read_listed(block.clone(), from, true, params);
        let _ = headers.map(|headers| to_block(&headers, &RULES));
        let Ok(downloaded) = read_listed(block, from, false, params) else {
            continue;
        };
        if let Ok(rebuilt) = to_block(&downloaded, &RULES) {
            let again = Block::decode(&rebuilt.encode(), &RULES).expect("a rebuilt block decodes");
            assert_eq!(again, rebuilt);
        }
    }
});
