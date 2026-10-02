//! Fuzz the transaction JSON reader: JSON text read as a transaction's data, strictly and not.
//! No input may panic, and an accepted transaction's bytes decode back, under the same rule, to
//! the same bytes, the same id and the same JSON.
//!
//! The reader renames a transfer's `asset.payments` and a truthy `vendorField` as the reference's
//! factory does; the replay sends each corpus file to the reference's
//! `TransactionFactory.fromData(data, strict)` and compares acceptance, id and bytes.

#![no_main]

use std::sync::LazyLock;

use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Milestones, Network, Params, Transaction};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

/// The transaction rules of the golden vectors' devnet: network byte 90, 150 transactions and
/// 2,097,152 bytes per block, 1 to 256 transfer items.
const MILESTONES: &str = include_str!("../milestones/transaction_decode.json");

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

fuzz_target!(|data: &[u8]| {
    let Ok(json) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let params = &*PARAMS;
    for mode in [Mode::Strict, Mode::NonStrict] {
        let Ok(tx) = Transaction::from_json_with(&json, params, mode) else {
            continue;
        };
        let again = Transaction::decode_with(tx.bytes(), params, mode)
            .expect("the bytes of an accepted transaction decode");
        assert_eq!(again.bytes(), tx.bytes());
        assert_eq!(again.id(), tx.id());
        assert_eq!(again.to_json(params), tx.to_json(params));
        let read = Transaction::from_json_with(&tx.to_json(params), params, mode)
            .expect("the JSON of an accepted transaction reads");
        assert_eq!(read.bytes(), tx.bytes());
    }
});
