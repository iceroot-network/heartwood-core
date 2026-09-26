//! Drivers shared by the block and milestone vector tests: the devnet configuration every record
//! runs under, the oracle's `block.make`, `block.fromBytes` and `tx.fromBytes` operations, and
//! their outputs in the shape the vector files record them.

#![allow(dead_code)]

use std::sync::LazyLock;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::crypto::hash::{sha256, sha256_concat};
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::utils::{hex, js};
use heartwood_crypto::{Aux, Block, BlockHeader, KeyPair, Signature, Transaction};
use serde_json::{Map, Value, json};

use crate::common::{self, Record, VectorFile};

/// A time, in seconds since the devnet epoch, after every block timestamp in the vectors. The
/// oracle checked the timestamps against its own clock, which was later still.
pub const NOW: i64 = 1_000_000;

/// The devnet configuration of the vector files: the network description and the milestones of
/// the first chain definition in V10, whose milestone file is the one the oracle ran under (the
/// meta record of every class names its SHA-256).
pub struct Devnet {
    /// The network description.
    pub network: Network,
    /// The milestones, as loaded.
    pub milestones: Milestones,
    /// SHA-256 of the milestone file, in hex.
    pub milestones_sha256: String,
}

pub static DEVNET: LazyLock<Devnet> = LazyLock::new(|| {
    let file = vector_file!("V10-genesis");
    let record = file.records.first().expect("V10 has a chain definition");
    let files = &record.expected.as_ref().expect("the chain definition")["files"];
    let network_text = files["crypto/network.json"].as_str().expect("network.json");
    let milestones_text = files["crypto/milestones.json"]
        .as_str()
        .expect("milestones.json");
    let network = Network::load(network_text).expect("the devnet network loads");
    let milestones =
        Milestones::load(milestones_text, &network).expect("the devnet milestones load");
    Devnet {
        network,
        milestones,
        milestones_sha256: hex::encode(&sha256(milestones_text.as_bytes())),
    }
});

/// Check that `file` was generated under the devnet milestones of [`DEVNET`].
pub fn check_devnet(file: &VectorFile) {
    assert_eq!(
        file.meta["network"]["milestonesSha256"].as_str(),
        Some(DEVNET.milestones_sha256.as_str()),
        "{}: generated under other devnet milestones",
        file.class
    );
    assert_eq!(
        file.meta["network"]["pubKeyHash"],
        json!(DEVNET.network.pub_key_hash)
    );
}

/// The milestones a record runs under: its own milestone file, or the devnet's.
pub fn milestones(record: &Record) -> Result<Milestones, String> {
    match record.input.get("milestones") {
        None => Ok(DEVNET.milestones.clone()),
        Some(text) => {
            let text = text.as_str().ok_or("milestones is not text")?;
            Milestones::load(text, &DEVNET.network).map_err(|error| error.to_string())
        }
    }
}

/// The record's input field `key` as a string.
pub fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .ok_or_else(|| format!("{key} is not a string"))
}

/// The record's input field `key` as a 32-bit integer.
pub fn integer(input: &Value, key: &str) -> Result<u32, String> {
    input[key]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("{key} is not a 32-bit integer"))
}

/// The record's input field `key`, 32 bytes in hex.
pub fn bytes32(input: &Value, key: &str) -> Result<[u8; 32], String> {
    hex::decode_array(text(input, key)?).map_err(|error| error.to_string())
}

/// The oracle's `block.make`: a block shaped as the forger makes it, from transactions read
/// without checks at the block's height, signed with BIP340 by the factory or with the legacy
/// scheme over the same hash, then assembled as the reference's `BlockFactory.fromData` does.
///
/// Without the `legacy-schnorr` feature a legacy signature cannot be made here, so it is taken
/// from `expected_signature`, the signature the record shows; everything else is still checked.
pub fn make_block(
    input: &Value,
    milestones: &Milestones,
    expected_signature: Option<&str>,
) -> Result<Block, String> {
    let height = integer(input, "height")?;
    let params = milestones.at(height);
    let transactions = input["transactions"]
        .as_array()
        .ok_or("transactions is not a list")?
        .iter()
        .map(|item| {
            let bytes = hex::decode(item.as_str().ok_or("a transaction is not hex")?)
                .map_err(|error| error.to_string())?;
            Transaction::decode_trusted(&bytes, params).map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<Transaction>, String>>()?;

    let overrides = input.get("overrides").cloned().unwrap_or_else(|| json!({}));
    let count = u32::try_from(transactions.len()).map_err(|error| error.to_string())?;
    let total_amount = match overrides.get("totalAmount") {
        Some(amount) => amount
            .as_str()
            .and_then(|text| text.parse().ok())
            .ok_or("totalAmount is not an amount")?,
        None => transactions
            .iter()
            .try_fold(0_u64, |sum, transaction| {
                sum.checked_add(transaction.amount())
            })
            .ok_or("the total amount overflows")?,
    };
    let total_fee = transactions
        .iter()
        .try_fold(0_u64, |sum, transaction| sum.checked_add(transaction.fee()))
        .ok_or("the total fee overflows")?;
    let draft = BlockDraft {
        version: match overrides.get("version") {
            Some(_) => integer(&overrides, "version")?,
            None => 0,
        },
        timestamp: integer(input, "timestamp")?,
        height,
        previous_block: bytes32(input, "previousBlock")?,
        number_of_transactions: count,
        total_amount,
        total_fee,
        reward: text(input, "reward")?
            .parse()
            .map_err(|_| "reward is not an amount")?,
        payload_length: match overrides.get("payloadLength") {
            Some(_) => integer(&overrides, "payloadLength")?,
            None => 32 * count,
        },
        payload_hash: sha256_concat(
            transactions
                .iter()
                .map(|transaction| transaction.id().as_bytes().as_slice()),
        ),
    };
    let keys = KeyPair::from_passphrase(text(input, "passphrase")?).map_err(|e| e.to_string())?;
    let aux = Aux::fixed(bytes32(input, "aux")?);

    match input
        .get("scheme")
        .and_then(Value::as_str)
        .unwrap_or("bip340")
    {
        "bip340" => {
            factory::make(draft, transactions, &keys, aux, milestones).map_err(|e| e.to_string())
        }
        "legacy" => {
            let mut header = BlockHeader {
                version: draft.version,
                timestamp: draft.timestamp,
                height: draft.height,
                previous_block: draft.previous_block,
                number_of_transactions: draft.number_of_transactions,
                total_amount: draft.total_amount,
                total_fee: draft.total_fee,
                reward: draft.reward,
                payload_length: draft.payload_length,
                payload_hash: draft.payload_hash,
                generator_public_key: keys.public_key().to_compressed(),
                block_signature: Signature::from_array([0; 64]),
            };
            header.block_signature = legacy_signature(&header, &keys, expected_signature)?;
            factory::from_data(header, &transactions, milestones).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown scheme {other}")),
    }
}

#[cfg(feature = "legacy-schnorr")]
fn legacy_signature(
    header: &BlockHeader,
    keys: &KeyPair,
    expected: Option<&str>,
) -> Result<Signature, String> {
    use heartwood_crypto::crypto::sig::{self, SchemeId, SigningDomain};
    let signature = sig::sign(
        SchemeId::Secp256k1Legacy,
        SigningDomain::Block,
        &serialiser::signing_hash(header),
        keys.secret_key(),
        // Not used: the legacy scheme derives its nonce from the key and the message.
        Aux::random(),
    )
    .map_err(|error| error.to_string())?;
    if let Some(expected) = expected {
        assert_eq!(signature.to_hex(), expected, "the legacy block signature");
    }
    Ok(signature)
}

#[cfg(not(feature = "legacy-schnorr"))]
fn legacy_signature(
    _header: &BlockHeader,
    _keys: &KeyPair,
    expected: Option<&str>,
) -> Result<Signature, String> {
    let expected = expected.ok_or("a legacy signature needs the legacy-schnorr feature")?;
    Signature::from_hex(expected).map_err(|error| error.to_string())
}

/// The oracle's `block.fromBytes`.
pub fn decode_block(input: &Value, milestones: &Milestones) -> Result<Block, String> {
    let bytes = hex::decode(text(input, "hex")?).map_err(|error| error.to_string())?;
    Block::decode(&bytes, milestones).map_err(|error| error.to_string())
}

/// A block as the oracle records it: id, signed header, bytes and JSON (or, with `summary`, the
/// SHA-256 and length of the bytes and of the canonical JSON text), and the verification.
pub fn block_output(block: &Block, milestones: &Milestones, summary: bool) -> Value {
    let bytes = block.encode();
    let json = block.to_json(milestones);
    let mut out = Map::new();
    out.insert("id".into(), json!(block.id().to_hex()));
    out.insert(
        "headerHex".into(),
        json!(hex::encode(&serialiser::serialise_header(
            block.header(),
            true
        ))),
    );
    if summary {
        out.insert("bytes".into(), digest(&bytes));
        out.insert("jsonText".into(), digest(canonical_json(&json).as_bytes()));
        out.insert("transactions".into(), json!(block.transactions().len()));
    } else {
        out.insert("hex".into(), json!(hex::encode(&bytes)));
        out.insert("json".into(), json);
    }
    out.insert("verification".into(), verification(block, milestones));
    Value::Object(out)
}

/// The verification as the reference reports it: its error texts in check order.
pub fn verification(block: &Block, milestones: &Milestones) -> Value {
    let verification = block.verify(milestones, NOW);
    let mut errors = Vec::new();
    for failure in &verification.failures {
        match failure {
            heartwood_crypto::blocks::VerifyFailure::UnverifiedTransactions(_) => {
                errors.push("One or more transactions are not verified:".to_owned());
                for transaction in block.transactions() {
                    if !transaction.is_verified() {
                        errors.push(format!("=> {}", hex::encode(transaction.bytes())));
                    }
                }
            }
            other => errors.push(other.to_string()),
        }
    }
    json!({
        "verified": verification.is_verified(),
        "containsMultiSignatures": false,
        "errors": errors,
    })
}

/// A transaction as the oracle records it after `tx.fromBytes`.
pub fn transaction_output(
    transaction: &Transaction,
    milestones: &Milestones,
    height: u32,
) -> Value {
    json!({
        "id": transaction.id().to_hex(),
        "hex": hex::encode(transaction.bytes()),
        "json": transaction.to_json(milestones.at(height)),
        "isVerified": transaction.is_verified(),
    })
}

/// The oracle's strict `tx.fromBytes` at the record's height.
pub fn decode_transaction(record: &Record, milestones: &Milestones) -> Result<Value, String> {
    let height = u32::try_from(record.height).map_err(|error| error.to_string())?;
    let bytes = hex::decode(text(&record.input, "hex")?).map_err(|error| error.to_string())?;
    let transaction =
        Transaction::decode(&bytes, milestones.at(height)).map_err(|error| error.to_string())?;
    Ok(transaction_output(&transaction, milestones, height))
}

fn digest(bytes: &[u8]) -> Value {
    json!({ "sha256": hex::encode(&sha256(bytes)), "length": bytes.len() })
}

/// JSON text with the keys of every object sorted by UTF-16 code units and no whitespace, as the
/// oracle writes it for its summaries.
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            let entries: Vec<String> = entries
                .into_iter()
                .map(|(key, item)| {
                    format!("{}:{}", js::stringify(&json!(key), 0), canonical_json(item))
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        other => js::stringify(other, 0),
    }
}

/// Whether two JSON values print the same with `JSON.stringify`: the same keys in the same order,
/// and numbers equal as JavaScript prints them.
pub fn same_json(actual: &Value, expected: &Value) -> bool {
    js::stringify(actual, 0) == js::stringify(expected, 0)
}

/// Pass and skip counts of a vector class, and every mismatch found.
#[derive(Debug, Default)]
pub struct Tally {
    pub passed: usize,
    pub skipped: usize,
    pub failures: Vec<String>,
}

impl Tally {
    /// Record the outcome of one record.
    pub fn check(&mut self, label: String, outcome: Result<(), String>) {
        match outcome {
            Ok(()) => self.passed += 1,
            Err(message) => self.failures.push(format!("{label}: {message}")),
        }
    }

    /// Record a documented skip.
    pub fn skip(&mut self) {
        self.skipped += 1;
    }

    /// Fail the test on any mismatch, and check that every record was counted.
    pub fn finish(self, file: &VectorFile) {
        println!(
            "{}: {} passed, {} skipped, {} failed",
            file.class,
            self.passed,
            self.skipped,
            self.failures.len()
        );
        assert!(
            self.failures.is_empty(),
            "{} of {} records of {} fail:\n{}",
            self.failures.len(),
            file.records.len(),
            file.class,
            self.failures.join("\n")
        );
        assert_eq!(
            self.passed + self.skipped,
            file.records.len(),
            "{}: every record is a pass or a skip",
            file.class
        );
    }
}

/// Compare an operation's result with the record: the same output, or an error where the
/// reference raised one.
pub fn compare(record: &Record, actual: Result<Value, String>) -> Result<(), String> {
    match (&record.expected, actual) {
        (Ok(expected), Ok(actual)) => {
            if same_json(&actual, expected) {
                Ok(())
            } else {
                Err(format!(
                    "output differs\n  expected {}\n  actual   {}",
                    abbreviate(&js::stringify(expected, 0)),
                    abbreviate(&js::stringify(&actual, 0))
                ))
            }
        }
        (Err(_), Err(_)) => Ok(()),
        (Ok(_), Err(error)) => Err(format!("refused, but the reference accepts: {error}")),
        (Err(class), Ok(actual)) => Err(format!(
            "accepted, but the reference raises {class}: {}",
            abbreviate(&js::stringify(&actual, 0))
        )),
    }
}

fn abbreviate(text: &str) -> String {
    const LIMIT: usize = 2_000;
    match text.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}... ({} bytes)", &text[..end], text.len()),
        None => text.to_owned(),
    }
}

/// Run a `block.make` record.
pub fn make_record(record: &Record) -> Result<(), String> {
    let milestones = milestones(record)?;
    let summary = record.input["summary"].as_bool().unwrap_or(false);
    let expected_signature = record
        .expected
        .as_ref()
        .ok()
        .and_then(|output| output["json"]["blockSignature"].as_str());
    let actual = make_block(&record.input, &milestones, expected_signature)
        .map(|block| block_output(&block, &milestones, summary));
    compare(record, actual)
}

/// Run a `block.fromBytes` record.
pub fn decode_record(record: &Record) -> Result<(), String> {
    let milestones = milestones(record)?;
    let summary = record.input["summary"].as_bool().unwrap_or(false);
    let actual = decode_block(&record.input, &milestones)
        .map(|block| block_output(&block, &milestones, summary));
    compare(record, actual)
}

/// Run a `tx.fromBytes` record.
pub fn transaction_record(record: &Record) -> Result<(), String> {
    let actual = milestones(record).and_then(|milestones| decode_transaction(record, &milestones));
    compare(record, actual)
}
