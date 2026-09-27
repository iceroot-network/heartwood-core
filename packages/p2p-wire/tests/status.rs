//! The signed status records of vector class V13, and the status signatures of the signature
//! class of `heartwood-crypto`.
//!
//! - `status.sign`: the preimage and the response bytes, from the block, the slot, the
//!   configuration and the keys of the record.
//! - `status.verify`: the decoder's verdict and error class, the reply check's verdict and the
//!   part it names, both preimages, every pair's check, and the two changes the check makes (the
//!   generator key and the plugin names). The whole checked value is compared by the harness.
//! - The getStatus response codec: each decoded reply's verdict and amounts, and each encoded
//!   reply's bytes, read back and written again.
//!
//! Where the reference's result depends on its stack, the record is skipped here. Where it
//! depends on a plugin made the prototype or on integers beyond 128 bits, the record is one of the
//! documented differences, and only Heartwood's side of it is checked.
//!
//! The records are read as `common::records` says; a `wire.decodeSelected` record whose first
//! byte selects the getStatus response is checked as that codec's decode record.

mod common;

use common::CLASS;
use heartwood_crypto::blocks::deserialiser::deserialise_header;
use heartwood_crypto::utils::bytes::Reader;
use heartwood_crypto::{KeyPair, SecretKey};
use heartwood_p2p_wire::Proto;
use heartwood_p2p_wire::errors::{DecodeError, ReplyRefused};
use heartwood_p2p_wire::status;
use serde_json::{Value, json};

/// The signature class of `heartwood-crypto`, whose legacy records sign status preimages.
const SIGNATURES: &str = include_str!("../../crypto/tests/vectors/V04-signatures.jsonl");

fn unhex(text: &Value) -> Vec<u8> {
    hex::decode(text.as_str().unwrap()).unwrap()
}

fn label(record: &Value) -> String {
    format!(
        "{} {:?}",
        record["op"],
        record["name"].as_str().unwrap_or("")
    )
}

fn note(record: &Value, name: &str) -> bool {
    record["output"]["notes"][name] == json!(true)
}

/// Whether the reference's stack overflowed on the record's input.
fn overflowed(record: &Value) -> bool {
    record["error"]["class"] == "RangeError"
        && record["error"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("Maximum call stack size exceeded"))
}

fn part_name(part: ReplyRefused) -> &'static str {
    match part {
        ReplyRefused::Root => "root",
        ReplyRefused::State => "state",
        ReplyRefused::Header => "header",
        ReplyRefused::Config => "config",
        ReplyRefused::Network => "network",
        ReplyRefused::Plugins => "plugins",
        ReplyRefused::PublicKeys => "publicKeys",
        ReplyRefused::Signatures => "signatures",
    }
}

/// A `status.sign` record: the preimage and the response bytes.
fn check_sign(record: &Value) {
    let input = &record["input"];
    let block = unhex(&input["block"]);
    let header = deserialise_header(&mut Reader::new(&block)).unwrap();
    let state = status::SignedState {
        height: header.height,
        forging_allowed: input["forgingAllowed"].as_bool().unwrap(),
        current_slot: input["currentSlot"].as_i64().unwrap(),
        header: status::StatusHeaderData::from_header(&header),
    };
    let config = &input["config"];
    let network = &config["network"];
    let text = |value: &Value| value.as_str().unwrap().to_owned();
    let config = status::PeerConfig {
        version: text(&config["version"]),
        network: status::NetworkConfig {
            version: u8::try_from(network["version"].as_u64().unwrap()).unwrap(),
            name: text(&network["name"]),
            nethash: text(&network["nethash"]),
            explorer: text(&network["explorer"]),
            token: status::TokenConfig {
                name: text(&network["token"]["name"]),
                symbol: text(&network["token"]["symbol"]),
            },
        },
        plugins: (config["plugins"].as_array().unwrap().iter())
            .map(|entry| {
                let plugin = &entry[1];
                let config = status::PluginConfig {
                    enabled: plugin["enabled"].as_bool().unwrap(),
                    port: u32::try_from(plugin["port"].as_u64().unwrap()).unwrap(),
                    estimate_total_count: plugin["estimateTotalCount"].as_bool(),
                };
                (text(&entry[0]), config)
            })
            .collect(),
    };
    let preimage = status::signing_preimage(&state, &config);
    assert_eq!(
        hex::encode(&preimage),
        record["output"]["preimage"],
        "{}",
        label(record)
    );
    let attestations: Vec<_> = (input["keys"].as_array().unwrap().iter())
        .map(|key| {
            let key = KeyPair::from_secret_key(SecretKey::from_hex(key.as_str().unwrap()).unwrap());
            status::attest(&preimage, &key).unwrap()
        })
        .collect();
    let bytes = status::response(&state, &config, &attestations).encode();
    assert_eq!(
        hex::encode(&bytes),
        record["output"]["hex"],
        "{}",
        label(record)
    );
    // The response reads back and is written again byte for byte.
    let received = status::decode_response(&bytes).unwrap();
    assert_eq!(
        status::encode_response(&received),
        bytes,
        "{}",
        label(record)
    );
}

/// A `status.verify` record.
fn check_verify(record: &Value) {
    if overflowed(record) {
        return;
    }
    let max_delegates = usize::try_from(record["input"]["maxDelegates"].as_u64().unwrap()).unwrap();
    let decoded = status::decode_response(&unhex(&record["input"]["hex"]));
    if wide(record, &decoded) {
        return;
    }
    let mut received = match decoded {
        Ok(received) => received,
        Err(error) => {
            assert_eq!(
                error.js_class(),
                record["error"]["class"].as_str(),
                "{}",
                label(record)
            );
            return;
        }
    };
    let output = &record["output"];
    assert!(output.is_object(), "{}: Heartwood accepted", label(record));
    let inherits =
        (received.message.config.as_ref()).is_some_and(|config| config.plugins.inherits_plugin());
    let checked = status::check_reply(&mut received, max_delegates);
    if note(record, "pluginsPrototype") && output["refused"] == "plugins" {
        // The reference refuses the plugin it made the prototype; Heartwood drops that entry.
        assert!(inherits, "{}", label(record));
        assert!(
            matches!(
                checked,
                Ok(()) | Err(ReplyRefused::PublicKeys | ReplyRefused::Signatures)
            ),
            "{}",
            label(record)
        );
        return;
    }
    match checked {
        Err(part) => assert_eq!(
            (json!(false), json!(part_name(part))),
            (output["valid"].clone(), output["refused"].clone()),
            "{}",
            label(record)
        ),
        Ok(()) => {
            assert_eq!(output["valid"], true, "{}", label(record));
            let message = &received.message;
            for (preimage, verified, negative) in [
                ("preimage", "verified", false),
                ("preimageNegativeSlot", "verifiedNegativeSlot", true),
            ] {
                let bytes = status::verifying_preimage(&received, negative);
                assert_eq!(hex::encode(&bytes), output[preimage], "{}", label(record));
                let pairs: Vec<bool> = (message.signatures.iter().enumerate())
                    .map(|(i, signature)| {
                        (message.public_keys.get(i))
                            .is_some_and(|key| status::verify_attestation(&bytes, key, signature))
                    })
                    .collect();
                assert_eq!(json!(pairs), output[verified], "{}", label(record));
            }
            let value = &output["value"];
            let header = message.state.as_ref().unwrap().header.as_ref().unwrap();
            assert_eq!(
                json!(header.generator_public_key),
                value["state"]["header"]["generatorPublicKey"],
                "{}",
                label(record)
            );
            let config = message.config.as_ref().unwrap();
            let names: Vec<&str> = config.plugins.iter().map(|(name, _)| name).collect();
            let expected: Vec<&str> = (value["config"]["plugins"].as_object())
                .map(|plugins| plugins.keys().map(String::as_str).collect())
                .unwrap_or_default();
            assert_eq!(names, expected, "{}", label(record));
        }
    }
}

/// Whether the record reads an amount beyond 128 bits, which Heartwood refuses at decode.
fn wide(record: &Value, decoded: &Result<status::ReceivedStatus, DecodeError>) -> bool {
    let wide = note(record, "wideAmount");
    if wide {
        assert_eq!(
            decoded.as_ref().err(),
            Some(&DecodeError::AmountTooWide),
            "{}",
            label(record)
        );
    }
    wide
}

/// A record of the getStatus response codec.
fn check_codec(record: &Value) {
    if overflowed(record) {
        return;
    }
    if record["op"] == "wire.encode" {
        let bytes = unhex(&record["output"]["hex"]);
        let received = status::decode_response(&bytes).unwrap();
        assert_eq!(
            status::encode_response(&received),
            bytes,
            "{}",
            label(record)
        );
        return;
    }
    let decoded = status::decode_response(&unhex(&record["input"]["hex"]));
    if wide(record, &decoded) {
        return;
    }
    match decoded {
        Err(error) => assert_eq!(
            error.js_class(),
            record["error"]["class"].as_str(),
            "{}",
            label(record)
        ),
        Ok(received) => {
            let header = &record["output"]["value"]["state"]["header"];
            let amounts = [received.total_amount, received.total_fee, received.reward];
            let texts = amounts.map(|amount| json!(amount.to_string()));
            assert_eq!(
                texts,
                [
                    &header["totalAmount"],
                    &header["totalFee"],
                    &header["reward"]
                ]
                .map(Value::clone),
                "{}",
                label(record)
            );
        }
    }
}

#[test]
fn the_status_records_of_the_wire_class() {
    let mut checked = 0;
    for record in common::records().iter().map(common::selected_as_decode) {
        let record = &record;
        let codec =
            record["input"]["route"] == "getStatus" && record["input"]["direction"] == "response";
        match record["op"].as_str().unwrap() {
            "status.sign" => check_sign(record),
            "status.verify" => check_verify(record),
            "wire.encode" | "wire.decode" if codec => check_codec(record),
            _ => continue,
        }
        checked += 1;
    }
    assert!(checked > 0, "no status record in {CLASS}");
}

/// The legacy signatures of the signature class over status preimages are the attestations.
#[test]
fn the_status_signatures_of_the_signature_class() {
    let mut checked = 0;
    for record in SIGNATURES.lines().skip(1) {
        let record: Value = serde_json::from_str(record).unwrap();
        let name = record["name"].as_str().unwrap_or("");
        if record["op"] != "sig.legacy.sign" || !name.starts_with("status ") {
            continue;
        }
        let preimage = unhex(&record["input"]["message"]);
        let secret = SecretKey::from_hex(record["input"]["privateKey"].as_str().unwrap()).unwrap();
        let key = KeyPair::from_secret_key(secret);
        let attestation = status::attest(&preimage, &key).unwrap();
        assert_eq!(
            attestation.signature.to_hex(),
            record["output"]["signature"],
            "{name}"
        );
        let public_key = attestation.public_key.to_hex();
        let signature = attestation.signature.to_hex();
        assert!(
            status::verify_attestation(&preimage, &public_key, &signature),
            "{name}"
        );
        assert!(
            !status::verify_attestation(&preimage[1..], &public_key, &signature),
            "{name}"
        );
        checked += 1;
    }
    assert_eq!(checked, 10, "five statuses, two signers each");
}
