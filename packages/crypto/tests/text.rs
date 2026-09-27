//! The reference's message formats (vector class V22): `formatSatoshi`, the `toLocaleString`
//! calls behind it and behind counts, and a transaction's `toString`, each compared exactly.

#[macro_use]
mod common;

use heartwood_crypto::Transaction;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::utils::{format_satoshi, hex, js};
use serde_json::Value;

/// The devnet milestones of the vector files (the first chain definition of V10), after checking
/// that `file` was generated under them.
fn devnet(file: &common::VectorFile) -> Milestones {
    let genesis = vector_file!("V10-genesis");
    let record = genesis.records.first().expect("a chain definition");
    let files = &record.expected.as_ref().expect("the chain definition")["files"];
    let text = |name: &str| files[name].as_str().expect(name).to_owned();
    let milestones = text("crypto/milestones.json");
    assert_eq!(
        file.meta["network"]["milestonesSha256"].as_str(),
        Some(hex::encode(&sha256(milestones.as_bytes())).as_str()),
        "generated under other devnet milestones"
    );
    let network = Network::load(&text("crypto/network.json")).expect("the devnet network");
    Milestones::load(&milestones, &network).expect("the devnet milestones")
}

/// A count as the vectors hold it: an integer, which JavaScript read as a double.
fn count(value: &Value) -> i128 {
    let number = value.as_number().expect("a number");
    number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
        .unwrap_or_else(|| number.as_f64().expect("a double") as i128)
}

#[test]
fn v22_text() {
    let file = vector_file!("V22-text");
    let milestones = devnet(&file);
    let params = milestones.at(2);
    let mut compared = std::collections::BTreeMap::new();
    for record in &file.records {
        let input = &record.input;
        let label = record.label(&file.class);
        let expected = record
            .expected
            .as_ref()
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert!(
            expected.get("notes").is_none(),
            "{label}: written in another locale"
        );
        let text = match record.op.as_str() {
            "text.formatSatoshi" => format_satoshi(
                input["amount"].as_str().unwrap().parse().unwrap(),
                input["symbol"].as_str().unwrap(),
            ),
            "text.toLocaleFixed" => js::to_locale_fixed(
                js::number_value(input["value"].as_number().unwrap()),
                u8::try_from(input["digits"].as_u64().unwrap()).unwrap(),
            ),
            "text.toLocaleString" => js::to_locale_integer(count(&input["count"])),
            "text.transaction" => {
                let bytes = hex::decode(input["hex"].as_str().unwrap()).unwrap();
                let transaction = Transaction::decode_trusted(&bytes, params).unwrap();
                transaction.display(params.network()).to_string()
            }
            other => panic!("{label}: unexpected operation {other}"),
        };
        assert_eq!(text, expected["text"].as_str().unwrap(), "{label}");
        *compared.entry(record.op.as_str()).or_insert(0) += 1;
    }
    assert_eq!(compared.len(), 4, "{compared:?}");
}
