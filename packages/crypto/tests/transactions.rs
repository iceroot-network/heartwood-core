//! Golden vectors of the transaction codec and the transaction rules:
//!
//! - V02: the six types built, decoded and read from JSON;
//! - V03: non-canonical wire forms, refused forms and the removed types;
//! - V07: the vote rules.
//!
//! Every record either matches the reference (bytes, ids, JSON and verdicts, and the error class of
//! a refusal) or is one of the documented differences listed in [`divergence`], whose outcome is
//! asserted too. Each class ends by checking how many records took each path.

#[macro_use]
mod common;

use common::Record;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::errors::{BuildError, DecodeError, EncodeError, MemoError};
use heartwood_crypto::identities::{Address, KeyPair, PublicKeyBytes};
use heartwood_crypto::transactions::codec::HEADER_SIZE;
use heartwood_crypto::transactions::serialiser::{SerialiseOptions, serialise};
use heartwood_crypto::transactions::types::{Asset, Memo, ResignationType, TransferItem, Votes};
use heartwood_crypto::transactions::{TransactionBuilder, TransactionType};
use heartwood_crypto::utils::hex;
use heartwood_crypto::utils::sort_votes::sort_votes;
use heartwood_crypto::validation::{Mode, vote_basis_points, votes_from_json};
use heartwood_crypto::{Aux, Milestones, Network, Params, Transaction, TransactionData};
use serde_json::{Map, Value, json};

/// The fixed aux the reference's oracle signs every transaction with.
const AUX: Aux = Aux::fixed([0x42; 32]);

/// The devnet network of the vector files: network byte 90.
fn network() -> Network {
    Network {
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
    }
}

/// The devnet rules of the vector files at heights 1 and 2: the reference's merged mainnet
/// milestone with the devnet overrides. Height 2 adds donations only, which no transaction rule
/// reads, so the milestone of height 1 stands for both.
fn milestones() -> Milestones {
    let tier = |rank: u32| match rank {
        1..=10 => 180_000_000u64,
        11..=21 => 190_000_000,
        22..=32 => 200_000_000,
        33..=43 => 210_000_000,
        _ => 220_000_000,
    };
    let ranks: Map<String, Value> = (1..=53)
        .map(|rank| (rank.to_string(), json!(tier(rank))))
        .collect();
    let milestone = json!({
        "height": 1,
        "activeDelegates": 53,
        "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 },
        "blocksToRevokeDelegateResignation": 106,
        "blockTime": 8,
        "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 },
        "dynamicFees": {
            "enabled": true,
            "addonBytes": {
                "burn": 0, "delegateRegistration": 1_214_968, "delegateResignation": 0,
                "htlcClaim": 0, "htlcLock": 82, "htlcRefund": 0, "ipfs": 98, "legacyTransfer": 99,
                "legacyVote": 98, "multiSignature": 16, "secondSignature": 99, "transfer": 85,
                "vote": 98
            },
            "minFee": 6173
        },
        "epoch": "2026-01-01T00:00:00.000Z",
        "fees": {
            "staticFees": {
                "burn": 0, "delegateRegistration": 7_500_000_000u64, "delegateResignation": 0,
                "htlcClaim": 0, "htlcLock": 5_000_000, "htlcRefund": 0, "ipfs": 5_000_000,
                "legacyTransfer": 5_000_000, "legacyVote": 5_000_000, "multiSignature": 5_000_000,
                "secondSignature": 5_000_000, "transfer": 50_000_000, "vote": 9_000_000
            }
        },
        "legacyTransfer": false,
        "legacyVote": false,
        "p2p": { "minimumVersions": [">=4.3.0"] },
        "transfer": { "maximum": 256, "minimum": 1 },
        "reward": 1_000_000_000,
        "dynamicReward": { "enabled": true, "ranks": ranks, "secondaryReward": 180_000_000 },
        "acceptLegacySchnorrTransactions": false,
        "bip340": true,
        "donations": {}
    });
    Milestones::load(&json!([milestone]).to_string(), &network())
        .expect("the devnet milestones load")
}

fn params<'a>(milestones: &'a Milestones, record: &Record) -> &'a Params {
    milestones.at(u32::try_from(record.height).expect("a 32-bit height"))
}

/// The outcome of one operation: the reference's output, or the class of the error it raises.
type Outcome = Result<Value, String>;

/// The reference's error class for a decode refusal.
fn decode_class(error: &DecodeError) -> String {
    match error {
        DecodeError::Schema(_) => "TransactionSchemaError",
        DecodeError::UnsupportedVersion(_) => "TransactionVersionError",
        _ => "InvalidTransactionBytesError",
    }
    .into()
}

/// The reference's error class for a build or JSON refusal.
fn build_class(error: &BuildError) -> String {
    match error {
        BuildError::Schema(_) => "TransactionSchemaError".into(),
        BuildError::Decode(error) => decode_class(error),
        BuildError::Memo(MemoError::TooLong { .. }) => "MemoLengthExceededError".into(),
        BuildError::TooManyTransfers { .. } => "MaximumTransferCountExceededError".into(),
        BuildError::Encode(EncodeError::VoteAssetTooLarge { .. }) => {
            "VoteAssetTooLargeError".into()
        }
        other => format!("unexpected: {other}"),
    }
}

/// `{id, hex, json, isVerified}` of a transaction, as the oracle writes it.
fn tx_output(tx: &Transaction, params: &Params) -> Value {
    json!({
        "id": tx.id().to_hex(),
        "hex": hex::encode(tx.bytes()),
        "json": tx.to_json(params),
        "isVerified": tx.is_verified(),
    })
}

/// The strict decode verdict of `bytes`, as the oracle records it for a built transaction.
fn strict_verdict(bytes: &[u8], params: &Params) -> Value {
    match Transaction::decode(bytes, params) {
        Ok(tx) => json!({ "ok": true, "id": tx.id().to_hex(), "isVerified": tx.is_verified() }),
        Err(error) => json!({ "ok": false, "error": { "class": decode_class(&error) } }),
    }
}

/// Whether `ours` is the reference's outcome: the same output (compared as JSON text, so key
/// order and number forms count) or the same error class.
fn same_outcome(expected: &Outcome, ours: &Outcome) -> bool {
    match (expected, ours) {
        (Ok(expected), Ok(ours)) => json_text(&without_error_messages(expected)) == json_text(ours),
        (Err(expected), Err(ours)) => expected == ours,
        _ => false,
    }
}

/// The JSON text of `value`, keys in their order.
fn json_text(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON text")
}

/// `value` without the `message` of nested errors, which are never compared.
fn without_error_messages(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, value)| {
                    !(key.as_str() == "message"
                        && value.is_string()
                        && object.contains_key("class"))
                })
                .map(|(key, value)| (key.clone(), without_error_messages(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_error_messages).collect()),
        other => other.clone(),
    }
}

/// What happened to the records of a class.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tally {
    /// Records whose outcome equals the reference's.
    matched: usize,
    /// Records with a documented difference whose outcome was checked.
    divergent: usize,
    /// Records with no Heartwood counterpart.
    skipped: usize,
}

/// A documented difference from the reference, keyed by operation and record name.
enum Divergence {
    /// The reference accepts or refuses these bytes differently; Heartwood refuses them at decode
    /// with an error that the function recognises.
    Refused(&'static str, fn(&DecodeError) -> bool),
    /// The operation has no counterpart in Heartwood.
    Skipped(&'static str),
}

fn divergence(op: &str, name: &str) -> Option<Divergence> {
    const REMOVED: &str = "the type is removed: the reference decodes it and refuses it when it \
        is applied (its handler is inactive); Heartwood refuses it at decode, so a block holding \
        it is refused either way";
    const EXTENDED: &str = "the extended header: the reference decodes it and refuses it when it \
        is applied; Heartwood refuses it at decode";
    const MEMO: &str = "a memo that is not valid UTF-8: the reference replaces the bad bytes, \
        which changes the bytes it signs and hashes; Heartwood refuses it at decode";
    const MULTI: &str = "multi-signature entries after the signatures: no sender can hold a \
        multi-signature wallet (its registration type is removed), so the reference refuses such \
        a transaction when it is applied; Heartwood refuses it at decode";
    let removed = |error: &DecodeError| matches!(error, DecodeError::RemovedType { .. });
    let extended = |error: &DecodeError| matches!(error, DecodeError::UnsupportedHeaderType(1));
    let memo = |error: &DecodeError| matches!(error, DecodeError::InvalidMemo);
    let multi = |error: &DecodeError| matches!(error, DecodeError::MultiSignature);
    match (op, name) {
        (
            "tx.build",
            "ipfs-1-5" | "htlc-lock-1-8" | "htlc-claim-1-9" | "htlc-refund-1-10"
            | "multisignature-1-4",
        ) => Some(Divergence::Skipped("no builder for a removed type")),
        ("tx.build", "extended-header-other-sender") => Some(Divergence::Skipped(
            "the builder has no sender address; the extended header is refused at decode",
        )),
        (
            "tx.fromBytes",
            "ipfs-1-5"
            | "htlc-lock-1-8"
            | "htlc-claim-1-9"
            | "htlc-refund-1-10"
            | "multisignature-1-4"
            | "legacy-transfer-1-0"
            | "legacy-vote-1-3",
        ) => Some(Divergence::Refused(REMOVED, removed)),
        (
            "tx.fromBytes",
            "extended-header-other-sender"
            | "extended-header-byte-63-sender"
            | "extended-header-own-sender",
        ) => Some(Divergence::Refused(EXTENDED, extended)),
        (
            "tx.fromBytes",
            "memo-invalid-utf8-wire-signed"
            | "memo-overlong-nul-wire-signed"
            | "memo-surrogate-wire-signed"
            | "memo-truncated-wire-signed"
            | "memo-invalid-utf8-canonical-signed",
        ) => Some(Divergence::Refused(MEMO, memo)),
        // trailing-130 is refused by the reference too, as a duplicate participant.
        (
            "tx.fromBytes",
            "trailing-1" | "trailing-65" | "trailing-66" | "trailing-130" | "resignation-0f",
        ) => Some(Divergence::Refused(MULTI, multi)),
        _ => None,
    }
}

/// Run every record of `file` through `run` and check it against the reference, allowing the
/// documented differences. Panics with every mismatch listed.
fn check_class(
    file: &common::VectorFile,
    milestones: &Milestones,
    mut run: impl FnMut(&Record, &Params) -> Result<Outcome, Skip>,
) -> Tally {
    let mut tally = Tally::default();
    let mut failures = Vec::new();
    for record in &file.records {
        let params = params(milestones, record);
        let name = record.name.as_deref().unwrap_or_default();
        match (divergence(&record.op, name), run(record, params)) {
            (None, Ok(ours)) => {
                if same_outcome(&record.expected, &ours) {
                    tally.matched += 1;
                } else {
                    failures.push(format!(
                        "{}\n    expected {:?}\n    got      {:?}",
                        record.label(&file.class),
                        record.expected,
                        ours
                    ));
                }
            }
            (None, Err(Skip::Unsupported(reason))) => {
                failures.push(format!("{}: not run: {reason}", record.label(&file.class)));
            }
            (Some(Divergence::Skipped(reason)), Err(Skip::Unsupported(_))) => {
                eprintln!("{}: skipped: {reason}", record.label(&file.class));
                tally.skipped += 1;
            }
            (Some(Divergence::Refused(reason, recognise)), Err(Skip::Decode(error))) => {
                if recognise(&error) {
                    eprintln!(
                        "{}: refused as {error:?}: {reason}",
                        record.label(&file.class)
                    );
                    tally.divergent += 1;
                } else {
                    failures.push(format!(
                        "{}: refused with another error: {error:?}",
                        record.label(&file.class)
                    ));
                }
            }
            (Some(_), other) => {
                failures.push(format!(
                    "{}: documented difference, but got {other:?}",
                    record.label(&file.class)
                ));
            }
            (None, Err(other)) => {
                failures.push(format!(
                    "{}: unexpected {other:?}",
                    record.label(&file.class)
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} records failed:\n{}",
        failures.len(),
        file.records.len(),
        failures.join("\n")
    );
    tally
}

/// Why an operation did not give an outcome to compare.
#[derive(Debug)]
enum Skip {
    /// The operation has no counterpart here.
    Unsupported(&'static str),
    /// A decode refusal to check against a documented difference.
    Decode(DecodeError),
}

/// `tx.fromBytes`: strict decode unless the record says `strict: false`.
fn from_bytes(record: &Record, params: &Params) -> Result<Outcome, Skip> {
    let bytes = hex::decode(record.input["hex"].as_str().expect("hex")).expect("hex");
    let mode = if record.input["strict"] == json!(false) {
        Mode::NonStrict
    } else {
        Mode::Strict
    };
    match Transaction::decode_with(&bytes, params, mode) {
        Ok(tx) => Ok(Ok(tx_output(&tx, params))),
        Err(error)
            if divergence(&record.op, record.name.as_deref().unwrap_or_default()).is_some() =>
        {
            Err(Skip::Decode(error))
        }
        Err(error) => Ok(Err(decode_class(&error))),
    }
}

/// `tx.fromJson`.
fn from_json(record: &Record, params: &Params) -> Result<Outcome, Skip> {
    match Transaction::from_json(&record.input["json"], params) {
        Ok(tx) => Ok(Ok(tx_output(&tx, params))),
        Err(error) => Ok(Err(build_class(&error))),
    }
}

/// `tx.build`: the reference's builder calls, translated to [`TransactionBuilder`].
fn build(record: &Record, params: &Params) -> Result<Outcome, Skip> {
    let input = &record.input;
    let builder = input["builder"].as_str().expect("builder");
    if !matches!(
        builder,
        "transfer"
            | "secondSignature"
            | "delegateRegistration"
            | "delegateResignation"
            | "burn"
            | "vote"
    ) {
        return Err(Skip::Unsupported("no builder for a removed type"));
    }
    let text = |value: &Value| value.as_str().expect("a text argument").to_owned();
    let amount = |value: &Value| text(value).parse::<u64>().expect("a decimal amount");
    let mut transfers: Vec<TransferItem> = Vec::new();
    let (mut nonce, mut fee, mut memo) = (None, None, None);
    let mut asset: Option<Asset> = None;
    let maximum = match params.transfer_maximum() {
        0 => 256,
        maximum => usize::try_from(maximum).expect("a small maximum"),
    };
    let recipient = |text: &str| Address::from_base58(text).expect("a recipient address");
    for call in input["calls"].as_array().expect("calls") {
        let method = call[0].as_str().expect("a method name");
        let argument = &call[1];
        match method {
            "nonce" => nonce = Some(amount(argument)),
            "fee" => fee = Some(amount(argument)),
            "memo" => {
                // The reference's builder ignores an empty memo.
                if !text(argument).is_empty() {
                    match Memo::new(&text(argument)) {
                        Ok(value) => memo = Some(value),
                        Err(error) => return Ok(Err(build_class(&BuildError::Memo(error)))),
                    }
                }
            }
            // `recipientId` and `amount` set the one item of a single transfer.
            "recipientId" => {
                let amount = transfers.first().map_or(0, |item| item.amount);
                transfers = vec![TransferItem {
                    amount,
                    recipient: recipient(&text(argument)),
                }];
            }
            "amount" if input["builder"] == "burn" => {
                asset = Some(Asset::Burn {
                    amount: amount(argument),
                })
            }
            "amount" => {
                let item = transfers.first().expect("recipientId before amount");
                transfers = vec![TransferItem {
                    amount: amount(argument),
                    recipient: item.recipient,
                }];
            }
            "addTransfer" => {
                // The reference's builder refuses the item that goes over the maximum when it is
                // added, before anything is signed.
                if transfers.len() >= maximum {
                    return Ok(Err("MaximumTransferCountExceededError".into()));
                }
                transfers.push(TransferItem {
                    amount: amount(&call[2]),
                    recipient: recipient(&text(argument)),
                });
            }
            "signatureAsset" => {
                let key = KeyPair::from_passphrase(&text(argument)).expect("keys");
                let bytes: [u8; 33] = key
                    .public_key()
                    .as_bytes()
                    .try_into()
                    .expect("a compressed key");
                asset = Some(Asset::SecondSignature {
                    public_key: PublicKeyBytes::from_array(bytes),
                });
            }
            "usernameAsset" => {
                asset = Some(Asset::DelegateRegistration {
                    username: text(argument),
                })
            }
            "resignationTypeAsset" => {
                let number =
                    u8::try_from(argument.as_u64().expect("a type")).expect("a small type");
                asset = Some(Asset::DelegateResignation {
                    resignation_type: Some(
                        ResignationType::from_number(number).expect("0, 1 or 2"),
                    ),
                });
            }
            "votesAsset" => {
                asset = Some(Asset::Vote {
                    votes: builder_votes(argument),
                })
            }
            "senderId" => return Err(Skip::Unsupported("sender address")),
            other => panic!("builder method {other} is not translated"),
        }
    }
    let mut builder = match (builder, asset) {
        ("transfer", _) => TransactionBuilder::transfer(transfers),
        (_, Some(Asset::SecondSignature { public_key })) => {
            TransactionBuilder::second_signature(public_key)
        }
        (_, Some(Asset::DelegateRegistration { username })) => {
            TransactionBuilder::delegate_registration(&username)
        }
        (_, Some(Asset::DelegateResignation { resignation_type })) => {
            TransactionBuilder::delegate_resignation(resignation_type)
        }
        (_, Some(Asset::Burn { amount })) => TransactionBuilder::burn(amount),
        (_, Some(Asset::Vote { votes })) => TransactionBuilder::vote(votes),
        (other, asset) => panic!("builder {other} with asset {asset:?}"),
    };
    if let Some(nonce) = nonce {
        builder = builder.nonce(nonce);
    }
    if let Some(fee) = fee {
        builder = builder.fee(fee);
    }
    if let Some(memo) = memo {
        builder = builder.memo(memo);
    }
    let keys = KeyPair::from_passphrase(input["sign"].as_str().expect("sign")).expect("keys");
    let built = match input.get("secondSign").and_then(Value::as_str) {
        Some(second) => {
            let second = KeyPair::from_passphrase(second).expect("keys");
            builder.build_second_signed(&keys, AUX, &second, AUX, params)
        }
        None => builder.build(&keys, AUX, params),
    };
    match built {
        Ok(tx) => {
            let mut output = tx_output(&tx, params);
            output["strict"] = strict_verdict(tx.bytes(), params);
            Ok(Ok(output))
        }
        Err(error) => Ok(Err(build_class(&error))),
    }
}

/// The votes the reference's `votesAsset` makes of its argument: an object of percentages, or a
/// list of names that share 100 % (`floor(10000 / n)` basis points each, one more for each of the
/// first `10000 mod n`).
fn builder_votes(argument: &Value) -> Votes {
    match argument {
        Value::Object(object) => object
            .iter()
            .map(|(name, percent)| {
                let basis_points = (percent.as_f64().expect("a percentage") * 100.0).round();
                (name.clone(), basis_points as u16)
            })
            .collect(),
        Value::Array(names) => {
            let names: Vec<&str> = names
                .iter()
                .map(|name| name.as_str().expect("a name"))
                .collect();
            let count = u16::try_from(names.len()).expect("a few names");
            let remainder = usize::from(10_000 % count);
            names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    (
                        (*name).to_owned(),
                        10_000 / count + u16::from(index < remainder),
                    )
                })
                .collect()
        }
        other => panic!("votesAsset of {other}"),
    }
}

/// The reference's vote object as the oracle writes it: `[[name, percentage], ...]`.
fn vote_entries(votes: &Votes) -> Value {
    Value::Array(
        votes
            .iter()
            .map(|(name, &basis_points)| {
                let percent = if basis_points.is_multiple_of(100) {
                    json!(basis_points / 100)
                } else {
                    json!(f64::from(basis_points) / 100.0)
                };
                json!([name, percent])
            })
            .collect(),
    )
}

/// The vote asset bytes of `votes`, as the vote codec writes them.
fn vote_asset(votes: Votes, params: &Params) -> Vec<u8> {
    let keys = KeyPair::from_passphrase("vote asset").expect("keys");
    let data = TransactionData {
        version: 3,
        network: params.network(),
        kind: TransactionType::Vote,
        nonce: 1,
        sender_public_key: keys.public_key().clone(),
        fee: 0,
        memo: None,
        asset: Asset::Vote { votes },
        signature: None,
        second_signature: None,
    };
    let bytes = serialise(&data, SerialiseOptions::UNSIGNED, params).expect("a vote serialises");
    bytes.get(HEADER_SIZE + 1..).expect("the asset").to_vec()
}

/// `vote.splits`: every two-way split of 100 % in cents.
fn vote_splits(record: &Record, params: &Params) -> Outcome {
    let input = &record.input;
    let names: Vec<&str> = input["names"]
        .as_array()
        .expect("names")
        .iter()
        .map(|name| name.as_str().expect("a name"))
        .collect();
    let (from, to) = (
        input["from"].as_u64().expect("from"),
        input["to"].as_u64().expect("to"),
    );
    let mut verdicts = Vec::new();
    let mut assets = Vec::new();
    let mut refused = Vec::new();
    for a in from..=to {
        let first = a as f64 / 100.0;
        let second = (10_000 - a) as f64 / 100.0;
        let object: Map<String, Value> = [
            (names[0].to_owned(), json!(first)),
            (names[1].to_owned(), json!(second)),
        ]
        .into_iter()
        .collect();
        let accepted = votes_from_json(&object).is_ok();
        if !accepted {
            refused.push(a);
        }
        verdicts.push(if accepted { b'1' } else { b'0' });
        let votes: Votes = [
            (names[0].to_owned(), (first * 100.0).round() as u16),
            (names[1].to_owned(), (second * 100.0).round() as u16),
        ]
        .into_iter()
        .collect();
        assets.extend(vote_asset(votes, params));
    }
    Ok(json!({
        "splits": to - from + 1,
        "accepted": to - from + 1 - refused.len() as u64,
        "refused": refused,
        "verdictsSha256": hex::encode(&sha256(&verdicts)),
        "assetsSha256": hex::encode(&sha256(&assets)),
    }))
}

#[test]
fn v02_transactions() {
    let file = vector_file!("V02-transactions");
    let milestones = milestones();
    let tally = check_class(&file, &milestones, |record, params| {
        match record.op.as_str() {
            "tx.build" => build(record, params),
            "tx.fromBytes" => from_bytes(record, params),
            "tx.fromJson" => from_json(record, params),
            other => panic!("unexpected op {other}"),
        }
    });
    assert_eq!(
        tally,
        Tally {
            matched: 248,
            divergent: 0,
            skipped: 0
        }
    );
}

#[test]
fn v03_noncanonical_and_removed() {
    let file = vector_file!("V03-noncanonical-removed");
    let milestones = milestones();
    let tally = check_class(&file, &milestones, |record, params| {
        match record.op.as_str() {
            "tx.build" => build(record, params),
            "tx.fromBytes" => from_bytes(record, params),
            "handler.activation" => {
                let group = u32::try_from(record.input["typeGroup"].as_u64().expect("typeGroup"))
                    .expect("u32");
                let kind =
                    u16::try_from(record.input["type"].as_u64().expect("type")).expect("u16");
                Ok(match TransactionType::from_wire(group, kind) {
                    Ok(kind) => Ok(json!({ "handler": handler_name(kind) })),
                    Err(DecodeError::RemovedType { .. }) => {
                        Err("DeactivatedTransactionHandlerError".into())
                    }
                    Err(DecodeError::UnknownType { .. }) => {
                        Err("InvalidTransactionTypeError".into())
                    }
                    Err(other) => Err(format!("unexpected: {other}")),
                })
            }
            other => panic!("unexpected op {other}"),
        }
    });
    assert_eq!(
        tally,
        Tally {
            matched: 50,
            divergent: 20,
            skipped: 6
        }
    );
}

/// The reference's handler class for a type this implementation carries.
fn handler_name(kind: TransactionType) -> &'static str {
    match kind {
        TransactionType::Transfer => "TransferTransactionHandler",
        TransactionType::SecondSignature => "SecondSignatureRegistrationTransactionHandler",
        TransactionType::DelegateRegistration => "DelegateRegistrationTransactionHandler",
        TransactionType::DelegateResignation => "DelegateResignationTransactionHandler",
        TransactionType::Burn => "BurnTransactionHandler",
        TransactionType::Vote => "VoteTransactionHandler",
    }
}

#[test]
fn v07_vote_rules() {
    let file = vector_file!("V07-vote-rules");
    let milestones = milestones();
    let tally = check_class(&file, &milestones, |record, params| {
        match record.op.as_str() {
            "vote.splits" => Ok(vote_splits(record, params)),
            "vote.validate" => Ok(
                match votes_from_json(record.input["votes"].as_object().expect("votes")) {
                    Ok(votes) => Ok(json!({ "votes": vote_entries(&votes) })),
                    Err(_) => Err("TransactionSchemaError".into()),
                },
            ),
            "vote.sort" => {
                let votes = builder_votes(&record.input["votes"]);
                Ok(Ok(json!({ "votes": vote_entries(&sort_votes(&votes)) })))
            }
            "tx.build" => build(record, params),
            "tx.fromBytes" => from_bytes(record, params),
            other => panic!("unexpected op {other}"),
        }
    });
    assert_eq!(
        tally,
        Tally {
            matched: 34,
            divergent: 0,
            skipped: 0
        }
    );
}

#[test]
fn vote_shares_of_the_splits() {
    for a in 1..10_000u16 {
        assert_eq!(vote_basis_points(f64::from(a) / 100.0), Ok(a));
    }
}
