//! The chain-definition builder against the reference's devnet generator (vector class
//! `V10-genesis`), and BIP39 mnemonics against the published English vectors (the BIP39 part of
//! `V11-external`).
//!
//! A `V10-genesis` record gives the generator's inputs: the seed, the epoch and the genesis profile
//! (every other value the chain definition is built from). The builder must write the four crypto
//! files byte for byte as the generator did, derive the same key material, and report the same
//! nethash, genesis id and issued amount. The block it writes must then read back and verify as the
//! reference's check of the same files did (see [`check_reads_back`] for what is compared).

#[macro_use]
mod common;

use heartwood_crypto::blocks::factory;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::errors::GenesisError;
use heartwood_crypto::genesis::{
    self, Allocation, ChainDefinition, DerivedKey, DonationShare, GenesisDelegate, GenesisInput,
    RuleOverride, bip39,
};
use heartwood_crypto::utils::hex;
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Block, Milestones, Network, Transaction};
use serde_json::{Map, Value, json};

use common::Record;

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string in {value}"))
}

fn number(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} is not an integer in {value}"))
}

fn amount(value: &Value, key: &str) -> u64 {
    text(value, key)
        .parse()
        .unwrap_or_else(|e| panic!("{key} is not an amount in {value}: {e}"))
}

/// The builder's input for a genesis profile, a seed and an epoch.
fn input_of(profile: &Value, seed: &str, epoch: &str) -> GenesisInput {
    let network = &profile["network"];
    let client = &network["client"];
    let network = Network {
        name: text(network, "name").into(),
        message_prefix: text(network, "messagePrefix").into(),
        address_character: None,
        bip32_public: number(&network["bip32"], "public").try_into().unwrap(),
        bip32_private: number(&network["bip32"], "private").try_into().unwrap(),
        pub_key_hash: number(network, "pubKeyHash").try_into().unwrap(),
        nethash: String::new(),
        wif: number(network, "wif").try_into().unwrap(),
        slip44: number(network, "slip44").try_into().unwrap(),
        token: text(client, "token").into(),
        symbol: text(client, "symbol").into(),
        explorer: text(client, "explorer").into(),
    };
    let numbered = |pattern: &Value, key: &str| -> Vec<String> {
        (1..=number(pattern, "count"))
            .map(|i| format!("{}{i}", text(pattern, key)))
            .collect()
    };
    let delegate_roles = numbered(&profile["delegates"], "rolePrefix");
    let usernames = numbered(&profile["delegates"], "usernamePrefix");
    GenesisInput {
        seed: seed.into(),
        derivation_tag: text(profile, "derivationTag").into(),
        epoch: epoch.into(),
        network,
        rules: profile["rules"].as_object().unwrap().clone(),
        overrides: profile["overrides"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, value)| RuleOverride {
                path: path.clone(),
                value: value.clone(),
            })
            .collect(),
        generator_role: text(profile, "generatorRole").into(),
        allocations: profile["allocations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| Allocation {
                role: text(a, "role").into(),
                amount: amount(a, "amount"),
            })
            .collect(),
        delegates: delegate_roles
            .into_iter()
            .zip(usernames)
            .map(|(role, username)| GenesisDelegate { role, username })
            .collect(),
        donations: profile["donations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| DonationShare {
                role: text(d, "role").into(),
                basis_points: number(d, "basisPoints").try_into().unwrap(),
                purpose: d["purpose"].as_str().map(str::to_owned),
            })
            .collect(),
        donations_from_height: number(profile, "donationsFromHeight").try_into().unwrap(),
        wallet_roles: numbered(&profile["wallets"], "rolePrefix"),
    }
}

fn record_input(record: &Record) -> GenesisInput {
    input_of(
        &record.input["profile"],
        text(&record.input, "seed"),
        text(&record.input, "epoch"),
    )
}

/// Fail with the first line where two files differ.
fn assert_same_file(label: &str, file: &str, actual: &str, expected: &str) {
    if actual == expected {
        return;
    }
    let line = actual
        .lines()
        .zip(expected.lines())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()));
    panic!(
        "{label}: {file} differs at line {}:\n  built:    {:?}\n  expected: {:?}\n  ({} and {} bytes)",
        line + 1,
        actual.lines().nth(line),
        expected.lines().nth(line),
        actual.len(),
        expected.len()
    );
}

/// The fields of a derived key as the generator writes them.
fn key_json(key: &DerivedKey) -> Value {
    json!({
        "role": key.role,
        "address": key.address.to_base58(),
        "publicKey": key.keys.public_key().to_hex(),
        "passphrase": key.passphrase.as_str(),
    })
}

fn with(mut value: Value, extra: Value) -> Value {
    let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) else {
        panic!("not objects");
    };
    map.extend(extra);
    value
}

/// Compare everything a record holds with the built chain definition.
fn check_record(label: &str, record: &Record, built: &ChainDefinition) {
    let output = record
        .expected
        .as_ref()
        .expect("a genesis record has an output");
    let input = record_input(record);

    let files = [
        ("crypto/network.json", &built.network_json),
        ("crypto/milestones.json", &built.milestones_json),
        ("crypto/genesisBlock.json", &built.genesis_block_json),
        ("crypto/exceptions.json", &built.exceptions_json),
    ];
    for (file, text) in files {
        assert_same_file(label, file, text, output["files"][file].as_str().unwrap());
        assert_eq!(
            hex::encode(&sha256(text.as_bytes())),
            output["fileSha256"][file],
            "{label}: {file}"
        );
    }
    assert_eq!(output["files"].as_object().unwrap().len(), files.len());

    let summary = &built.summary;
    assert_eq!(hex::encode(&summary.nethash), output["nethash"], "{label}");
    assert_eq!(summary.genesis_id.to_hex(), output["genesisId"], "{label}");
    assert_eq!(
        u64::from(summary.transactions),
        number(output, "transactions")
    );
    assert_eq!(summary.issued, amount(output, "issued"), "{label}");
    assert_eq!(
        hex::encode(&sha256(input.seed.as_bytes())),
        output["seedSha256"],
        "{label}"
    );

    let delegates: Vec<Value> = built
        .delegates
        .iter()
        .zip(&input.delegates)
        .map(|(key, delegate)| {
            let fields = key_json(key);
            json!({
                "username": delegate.username,
                "role": fields["role"],
                "address": fields["address"],
                "publicKey": fields["publicKey"],
                "privateKey": key.keys.secret_key().to_hex().as_str(),
                "passphrase": fields["passphrase"],
            })
        })
        .collect();
    assert_eq!(Value::from(delegates), output["delegates"], "{label}");

    let genesis_wallets: Vec<Value> = built
        .allocations
        .iter()
        .zip(&input.allocations)
        .map(|(key, allocation)| {
            with(
                key_json(key),
                json!({ "genesisBalance": allocation.amount.to_string() }),
            )
        })
        .collect();
    assert_eq!(Value::from(genesis_wallets), output["genesisWallets"]);

    let donation_wallets: Vec<Value> = built
        .donations
        .iter()
        .zip(&input.donations)
        .map(|(key, share)| {
            with(
                key_json(key),
                json!({
                    "genesisBalance": "0",
                    "basisPoints": share.basis_points,
                    "fromHeight": input.donations_from_height,
                }),
            )
        })
        .collect();
    assert_eq!(Value::from(donation_wallets), output["donationWallets"]);

    let test_wallets: Vec<Value> = built.wallets.iter().map(key_json).collect();
    assert_eq!(Value::from(test_wallets), output["testWallets"]);

    let generator = with(
        key_json(&built.generator),
        json!({ "issued": summary.issued.to_string() }),
    );
    assert_eq!(generator, output["generator"], "{label}");

    check_reads_back(label, built, &output["check"]);
}

/// Read the written files back, and compare with the reference's check of the same files: the
/// block read from `genesisBlock.json` with the block factory's JSON reader, verified, and every
/// transaction decoded from its bytes under the rules for genesis transactions (which allow the
/// zero fee, as the reference does for its configured genesis block).
fn check_reads_back(label: &str, built: &ChainDefinition, check: &Value) {
    let network = Network::load(&built.network_json).expect("network.json loads");
    let milestones =
        Milestones::load(&built.milestones_json, &network).expect("milestones.json loads");
    let json: Value = serde_json::from_str(&built.genesis_block_json).unwrap();
    let block = factory::from_json(&json, &milestones).expect("genesisBlock.json reads back");
    assert_eq!(block.id(), built.block.id(), "{label}");
    assert_eq!(block.id().to_hex(), check["id"], "{label}");
    assert_eq!(
        Block::decode(&built.block.encode(), &milestones).expect("the genesis block decodes"),
        block,
        "{label}"
    );
    let verification = block.verify(&milestones, 0);
    assert_eq!(
        Value::from(verification.is_verified()),
        check["verified"],
        "{label}: {verification:?}"
    );
    assert_eq!(check["errors"], json!([]));
    assert_eq!(
        block.transactions().len() as u64,
        number(check, "transactions")
    );
    assert_eq!(
        u64::from(block.header().payload_length),
        number(check, "payloadLength")
    );
    assert_eq!(json["id"], block.id().to_hex());
    assert_eq!(
        json["payloadHash"],
        hex::encode(&block.header().payload_hash)
    );
    assert_eq!(
        json["blockSignature"],
        block.header().block_signature.to_hex()
    );

    let params = milestones.at(1);
    let strictly_verified = block
        .transactions()
        .iter()
        .filter(|tx| {
            Transaction::decode_with(tx.bytes(), params, Mode::GENESIS)
                .is_ok_and(|decoded| decoded.is_verified() && decoded.id() == tx.id())
        })
        .count();
    assert_eq!(
        strictly_verified as u64,
        number(check, "strictlyVerified"),
        "{label}"
    );
}

#[test]
fn v10_genesis_matches_the_reference_generator() {
    let file = vector_file!("V10-genesis");
    let mut passed = 0;
    for record in file.op("genesis.generate") {
        let label = record.label(&file.class);
        let built = genesis::build(&record_input(record))
            .unwrap_or_else(|e| panic!("{label}: build failed: {e}"));
        check_record(&label, record, &built);
        passed += 1;
    }
    assert_eq!(passed, 3);
    assert_eq!(passed, file.records.len());
}

#[test]
fn v11_bip39_mnemonics() {
    let file = vector_file!("V11-external");
    let mut passed = 0;
    for record in file.op("bip39.entropyToMnemonic") {
        let label = record.label(&file.class);
        let entropy = hex::decode(text(&record.input, "entropy")).unwrap();
        let expected = record.expected.as_ref().expect("an output");
        let mnemonic =
            bip39::entropy_to_mnemonic(&entropy).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(mnemonic.as_str(), expected["mnemonic"], "{label}");
        passed += 1;
    }
    assert_eq!(passed, 24);
}

/// The first `V10-genesis` record's input, for the tests below that change one part of it.
fn base_input() -> GenesisInput {
    let file = vector_file!("V10-genesis");
    let record = file.op("genesis.generate").next().unwrap();
    record_input(record)
}

fn milestones_of(built: &ChainDefinition) -> Vec<Value> {
    serde_json::from_str::<Value>(&built.milestones_json)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn block_of(built: &ChainDefinition) -> Map<String, Value> {
    serde_json::from_str::<Value>(&built.genesis_block_json)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn a_build_is_deterministic_and_depends_on_the_seed() {
    let input = base_input();
    let a = genesis::build(&input).unwrap();
    let b = genesis::build(&input).unwrap();
    assert_eq!(a.genesis_block_json, b.genesis_block_json);
    assert_eq!(a.summary, b.summary);

    let other = GenesisInput {
        seed: format!("{}-other", input.seed),
        ..input.clone()
    };
    let c = genesis::build(&other).unwrap();
    assert_ne!(c.summary.nethash, a.summary.nethash);
    assert_eq!(c.summary.issued, a.summary.issued);
    assert_ne!(c.generator.address, a.generator.address);
}

#[test]
fn allocations_are_split_by_the_transfer_maximum() {
    let mut input = base_input();
    input.overrides.push(RuleOverride {
        path: "transfer.maximum".into(),
        value: json!(2),
    });
    let built = genesis::build(&input).unwrap();
    let block = block_of(&built);
    let transactions = block["transactions"].as_array().unwrap();
    let transfers: Vec<&Value> = transactions.iter().filter(|t| t["type"] == 6).collect();
    assert_eq!(transfers.len(), 2);
    assert_eq!(transfers[0]["nonce"], "1");
    assert_eq!(transfers[1]["nonce"], "2");
    assert_eq!(
        transfers[0]["asset"]["transfers"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        transfers[1]["asset"]["transfers"].as_array().unwrap().len(),
        1
    );
    let allocated: u64 = input.allocations.iter().map(|a| a.amount).sum();
    assert_eq!(built.summary.issued, allocated);
    assert_eq!(
        built.summary.transactions as usize,
        2 + 2 * input.delegates.len()
    );
    assert_eq!(milestones_of(&built)[0]["transfer"]["maximum"], 2);
}

#[test]
fn without_donations_there_is_one_milestone() {
    let mut input = base_input();
    input.donations.clear();
    input.donations_from_height = 1;
    let built = genesis::build(&input).unwrap();
    let milestones = milestones_of(&built);
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0]["donations"], json!({}));
    assert!(built.donations.is_empty());
}

#[test]
fn the_first_milestone_is_built_from_the_rules() {
    let input = base_input();
    let built = genesis::build(&input).unwrap();
    let first = milestones_of(&built)[0].as_object().unwrap().clone();
    // Height first, then the rules' keys in their order, with the epoch and the donations in place.
    let keys: Vec<&String> = first.keys().collect();
    let mut expected: Vec<&String> = vec![first.keys().next().unwrap()];
    expected.extend(input.rules.keys().filter(|k| *k != "height"));
    assert_eq!(keys, expected);
    assert_eq!(first["height"], 1);
    assert_eq!(first["epoch"], input.epoch.as_str());
    assert_eq!(first["donations"], json!({}));
    for rule in &input.overrides {
        let value = rule
            .path
            .split('.')
            .fold(&Value::Object(first.clone()), |v, k| &v[k])
            .clone();
        assert_eq!(value, rule.value, "{}", rule.path);
    }
}

#[test]
fn inconsistent_inputs_are_refused() {
    let refused = |change: &dyn Fn(&mut GenesisInput)| {
        let mut input = base_input();
        change(&mut input);
        genesis::build(&input).unwrap_err()
    };
    let is_input = |e: GenesisError| matches!(e, GenesisError::Input(_));
    assert!(is_input(refused(&|i| i.seed.clear())));
    assert!(is_input(refused(&|i| i.delegates.clear())));
    assert!(is_input(refused(&|i| i.generator_role.clear())));
    assert!(is_input(refused(&|i| i.wallet_roles.push(String::new()))));
    assert!(is_input(refused(&|i| {
        let first = i.delegates[0].clone();
        i.delegates.push(GenesisDelegate {
            username: format!("{}x", first.username),
            ..first
        });
    })));
    assert!(is_input(refused(&|i| {
        let first = i.delegates[0].clone();
        i.delegates.push(GenesisDelegate {
            role: format!("{}x", first.role),
            ..first
        });
    })));
    assert!(is_input(refused(&|i| {
        i.delegates[1].role = i.generator_role.clone();
    })));
    assert!(is_input(refused(&|i| i.donations_from_height = 1)));
    assert!(is_input(refused(&|i| {
        i.allocations[0].amount = u64::MAX;
    })));
    assert!(is_input(refused(&|i| i.allocations[1].amount = 0)));
    assert_eq!(
        refused(&|i| i.epoch = "2026-01-01T00:00:00Z".into()),
        GenesisError::Epoch("2026-01-01T00:00:00Z".into())
    );
}

#[test]
fn a_network_byte_with_two_address_characters_is_refused() {
    // Byte 29 gives addresses starting with "C" or "D", depending on the key hash.
    let mut input = base_input();
    input.network.pub_key_hash = 29;
    assert_eq!(
        genesis::build(&input).unwrap_err(),
        GenesisError::AddressCharacter {
            network: 29,
            low: 'C',
            high: 'D'
        }
    );
}

#[test]
fn rules_that_fail_the_start_up_checks_are_refused() {
    let mut input = base_input();
    input.overrides.push(RuleOverride {
        path: "burn.feeBasisPoints".into(),
        value: json!(10_001),
    });
    assert!(matches!(
        genesis::build(&input),
        Err(GenesisError::Config(_))
    ));
}
