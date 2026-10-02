//! Replay the codec fuzz targets' corpora through the oracle and Heartwood (`difftest oracle
//! replay-codec`), beside [`crate::replay`]'s replay of the binary decoders' corpora.
//!
//! - `address_decode`: each input, as text, validated as an address of the devnet's network by
//!   Heartwood and by the reference's address validation: equal verdicts and network errors, and
//!   an accepted address re-encoded to the same text.
//! - `transaction_json`: each input, as JSON text (its lossy UTF-8 reading, as the API reads a
//!   request's body), read as a transaction's data, strictly and not, by Heartwood and by the
//!   reference's transaction factory (with its `vendorField` and `asset.payments` aliases): equal
//!   acceptance or refusal, ids and bytes.
//! - `api_head`: a request's head read as Node's parser reads it; a differential replay only where
//!   the oracle has an operation that parses a head so, else the target is crash-only.

use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use heartwood_crypto::identities::Address;
use heartwood_crypto::utils::hex;
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Params, Transaction};
use serde_json::{Value, json};

use crate::cli::CodecReplayArgs;
use crate::errors::{Error, Result};
use crate::levels::l0;
use crate::oracle::OracleServer;
use crate::replay::ReplayReport;
use crate::report::{CaseResult, Outcome, Report};
use crate::subject::{self, Case, Registry, Subject, SubjectError, Verdict};
use crate::subjects::Devnet;
use crate::vectors::{Expected, VectorSet};

/// A codec fuzz target whose corpus can be replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecTarget {
    /// `address_decode`: text read as an address.
    AddressDecode,
    /// `transaction_json`: JSON text read as a transaction's data.
    TransactionJson,
    /// `api_head`: a request's head read as Node's parser reads it.
    ApiHead,
}

impl CodecTarget {
    /// Every codec target.
    pub const ALL: [CodecTarget; 3] = [
        CodecTarget::AddressDecode,
        CodecTarget::TransactionJson,
        CodecTarget::ApiHead,
    ];

    /// The fuzz target's name.
    pub const fn name(self) -> &'static str {
        match self {
            CodecTarget::AddressDecode => "address_decode",
            CodecTarget::TransactionJson => "transaction_json",
            CodecTarget::ApiHead => "api_head",
        }
    }
}

impl fmt::Display for CodecTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for CodecTarget {
    type Err = Error;

    fn from_str(text: &str) -> Result<CodecTarget> {
        CodecTarget::ALL
            .into_iter()
            .find(|target| target.name() == text)
            .ok_or_else(|| {
                Error::Usage(format!(
                    "{text} is not a codec target (address_decode, transaction_json or api_head)"
                ))
            })
    }
}

/// The rule height of every replayed case: the vectors' own.
const HEIGHT: u64 = 2;

/// The address bytes an address is validated for: the devnet's, and the reference's mainnet's, so
/// that the network error is replayed beside the verdict.
const NETWORK_BYTES: [u8; 2] = [90, 63];

/// `tx.fromData`, which the reference answers with `TransactionFactory.fromData(data, strict)`
/// after the copies `fromJson` makes: JSON text, read strictly or not, as Heartwood reads a
/// transaction's JSON. The strict reading is the registry's `tx.fromJson` subject, with its
/// documented differences; the other is the JSON reader's non-strict mode.
struct FromData {
    devnet: Devnet,
    strict: Registry,
}

impl FromData {
    fn new(devnet: Devnet) -> FromData {
        FromData {
            strict: subject::heartwood(Some(devnet.clone())),
            devnet,
        }
    }

    fn strict_verdict(
        &self,
        case: &Case<'_>,
        json: &Value,
    ) -> std::result::Result<Verdict, SubjectError> {
        let input = json!({ "json": json });
        let forwarded = Case {
            op: "tx.fromJson",
            network: case.network,
            height: case.height,
            input: &input,
        };
        self.strict
            .get("tx.fromJson")
            .ok_or(SubjectError::NotImplemented)?
            .run(&forwarded)
    }
}

/// `json` with a null `id` where it has none, which the strict reading requires of it.
fn with_id(json: &Value) -> Value {
    let mut out = json.clone();
    if let Value::Object(fields) = &mut out {
        fields.entry("id").or_insert(Value::Null);
    }
    out
}

/// `{id, hex, json, isVerified}` of a transaction, as the oracle writes it.
fn output(tx: &Transaction, params: &Params) -> Value {
    json!({
        "id": tx.id().to_hex(),
        "hex": hex::encode(tx.bytes()),
        "json": tx.to_json(params),
        "isVerified": tx.is_verified(),
    })
}

impl Subject for FromData {
    fn op(&self) -> &'static str {
        "tx.fromData"
    }

    fn run(&self, case: &Case<'_>) -> std::result::Result<Verdict, SubjectError> {
        let text = case.input["text"]
            .as_str()
            .ok_or_else(|| SubjectError::Failed("input.text is not text".into()))?;
        let strict = case.input.get("strict") != Some(&json!(false));
        let Ok(json) = serde_json::from_str::<Value>(text) else {
            return Ok(Verdict::Rejected {
                class: None,
                detail: "the text is not JSON".into(),
            });
        };
        if strict {
            return self.strict_verdict(case, &json);
        }
        let params = self
            .devnet
            .milestones
            .at(u32::try_from(case.height).unwrap_or(u32::MAX));
        match Transaction::from_json_with(&json, params, Mode::NonStrict) {
            Ok(tx) => Ok(Verdict::Output(output(&tx, params))),
            // A refusal by design (a removed type, an extended header, a multi-signature entry,
            // votes given as indices) holds in either mode; the strict reading names it, once the
            // `id` that only it requires is there.
            Err(error) => match self.strict_verdict(case, &with_id(&json))? {
                refused @ Verdict::RefusedByDesign { .. } => Ok(refused),
                _ => Ok(Verdict::Rejected {
                    class: None,
                    detail: error.to_string(),
                }),
            },
        }
    }
}

/// One line about the oracle, from its answer to `ping`.
fn describe_ping(pong: &Value) -> String {
    let short = |value: &Value| {
        value
            .as_str()
            .unwrap_or("unknown")
            .chars()
            .take(9)
            .collect::<String>()
    };
    format!(
        "reference {} {} (tree {}), Node {}",
        pong["solar"]["version"].as_str().unwrap_or("unknown"),
        short(&pong["solar"]["commit"]),
        short(&pong["solar"]["tree"]),
        pong["node"].as_str().unwrap_or("unknown"),
    )
}

/// `outcome` as this replay reports it. Where both sides refuse, the replay compares what the
/// targets promise, the verdicts, not the reference's error class: the payload decoder's
/// refusals have more than one class in the reference (a text too short to hold a checksum is a
/// RangeError, a wrong checksum an Error), and a transaction's text is refused by its JSON
/// parser, its number conversions, its type lookup or its schema, each with a class of its own,
/// where the subject answers with one.
fn refined(op: &str, outcome: Outcome) -> Outcome {
    match outcome {
        Outcome::ClassMismatch { .. } if op == "address.toBuffer" || op == "tx.fromData" => {
            Outcome::Pass
        }
        other => other,
    }
}

/// What the reference answers to `op` and what Heartwood's subject makes of it: the judged
/// outcome, and the reference's output when it gave one rather than an error.
fn judged(
    server: &mut OracleServer,
    registry: &Registry,
    op: &str,
    input: &Value,
) -> Result<(Outcome, Option<Value>)> {
    let (expected, answer) = match server.call(op, "devnet", HEIGHT, input)? {
        Ok(output) => (Expected::Output(output.clone()), Some(output)),
        Err(error) => (Expected::Error(error), None),
    };
    let case = Case {
        op,
        network: "devnet",
        height: HEIGHT,
        input,
    };
    let outcome = refined(op, l0::judge_case(&case, &expected, registry));
    Ok((outcome, answer))
}

/// An accepted address re-encodes to the text it was read from.
fn address_round_trip(text: &str) -> Option<Outcome> {
    let address = Address::from_base58(text).ok()?;
    let again = address.to_base58();
    Some(if again == text {
        Outcome::Pass
    } else {
        Outcome::HarnessError(format!(
            "the address {text:?} is written again as {again:?}"
        ))
    })
}

/// The input files of `corpus`, in name order, with their bytes.
fn inputs(corpus: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut names: Vec<String> = fs::read_dir(corpus)
        .map_err(|e| Error::io(corpus, e))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|e| Error::io(corpus, e))
        })
        .collect::<Result<_>>()?;
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let path = corpus.join(&name);
            let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
            Ok((name, bytes))
        })
        .collect()
}

/// Replay every file of `corpus`, in name order, as inputs of `target`.
///
/// `address_decode` inputs are text: the lossy UTF-8 reading, the same on both sides. Each is
/// validated as an address of the devnet's network and of the reference's mainnet
/// (`address.validate`), decoded for its payload and network complaint (`address.toBuffer`), and,
/// where Heartwood accepts it, written again. `transaction_json` inputs are JSON text, each read
/// strictly and not (`tx.fromData`), from the same lossy UTF-8 reading, which is how the API
/// reads a request's body on both sides.
/// `api_head` has no replay: the oracle has no operation that reads a request's head as Node's
/// parser does, so its fuzz target checks Heartwood's reader alone.
pub fn run(
    server: &mut OracleServer,
    registry: &Registry,
    target: CodecTarget,
    corpus: &Path,
) -> Result<ReplayReport> {
    if target == CodecTarget::ApiHead {
        return Err(no_head_replay());
    }
    let source = describe_ping(&server.ping()?);
    let corpus_name = corpus
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut results = Vec::new();
    let mut accepted = 0;
    for (index, (name, bytes)) in inputs(corpus)?.into_iter().enumerate() {
        let mut result = |op: &str, label: String, outcome: Outcome| {
            results.push(CaseResult {
                file: corpus_name.clone(),
                line: index + 1,
                op: op.to_owned(),
                label: format!("{name}: {label}"),
                height: HEIGHT,
                outcome,
            });
        };
        match target {
            CodecTarget::AddressDecode => {
                let text = String::from_utf8_lossy(&bytes);
                for byte in NETWORK_BYTES {
                    let input = json!({ "address": text, "networkByte": byte });
                    let (outcome, answer) = judged(server, registry, "address.validate", &input)?;
                    // `address.validate` answers every text; it accepts the ones that are valid.
                    let valid = answer.is_some_and(|a| a["valid"] == json!(true));
                    if byte == NETWORK_BYTES[0] && valid {
                        accepted += 1;
                    }
                    result(
                        "address.validate",
                        format!("for network byte {byte}"),
                        outcome,
                    );
                }
                let input = json!({ "address": text });
                let (outcome, _) = judged(server, registry, "address.toBuffer", &input)?;
                result(
                    "address.toBuffer",
                    "its payload and network complaint".into(),
                    outcome,
                );
                if let Some(outcome) = address_round_trip(&text) {
                    result(
                        "address.roundTrip",
                        "written again as it was read".into(),
                        outcome,
                    );
                }
            }
            CodecTarget::TransactionJson => {
                let text = String::from_utf8_lossy(&bytes);
                for strict in [true, false] {
                    let input = json!({ "text": text, "strict": strict });
                    let (outcome, answer) = judged(server, registry, "tx.fromData", &input)?;
                    if strict && answer.is_some() {
                        accepted += 1;
                    }
                    result("tx.fromData", format!("strict {strict}"), outcome);
                }
            }
            CodecTarget::ApiHead => {}
        }
    }
    Ok(ReplayReport {
        report: Report {
            title: format!("Replay of the {target} corpus {}", corpus.display()),
            source,
            files: 1,
            results,
        },
        accepted,
    })
}

/// The refusal of a replay of `api_head`.
fn no_head_replay() -> Error {
    Error::Usage(
        "api_head has no replay: the oracle has no operation that reads a request's head as \
         Node's parser does, so its fuzz target is crash-only"
            .into(),
    )
}

/// `difftest oracle replay-codec`: the corpus of `args`.
pub fn run_command(args: &CodecReplayArgs) -> Result<ReplayReport> {
    if args.target == CodecTarget::ApiHead {
        return Err(no_head_replay());
    }
    let set = VectorSet::load(&args.vectors)?;
    let devnet = Devnet::from_vectors(&set)?
        .ok_or_else(|| {
            Error::Usage(format!(
                "{} has no genesis class, so the devnet rules are unknown",
                args.vectors.display()
            ))
        })?
        .at_current_time()?;
    let oracle = args.oracle.resolve(|name| std::env::var(name).ok())?;
    let mut server = oracle.server()?;
    let mut registry = subject::heartwood(Some(devnet.clone()));
    registry.register(Box::new(FromData::new(devnet)))?;
    run(&mut server, &registry, args.target, &args.corpus)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::cli::OracleArgs;

    fn vectors() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors")
    }

    fn subject() -> FromData {
        let set = VectorSet::load(&vectors()).unwrap();
        FromData::new(Devnet::from_vectors(&set).unwrap().unwrap())
    }

    fn answer(subject: &FromData, text: &str, strict: bool) -> Verdict {
        let input = json!({ "text": text, "strict": strict });
        subject
            .run(&Case {
                op: "tx.fromData",
                network: "devnet",
                height: HEIGHT,
                input: &input,
            })
            .unwrap()
    }

    /// The first transfer of the committed transaction vectors: its JSON and the output the
    /// reference gave.
    fn transfer() -> (Value, Value) {
        let text = fs::read_to_string(vectors().join("V02-transactions.jsonl")).unwrap();
        text.lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|r| r["op"] == "tx.fromJson" && r["name"] == "transfer-1-memo")
            .map(|r| (r["input"]["json"].clone(), r["output"].clone()))
            .unwrap()
    }

    #[test]
    fn a_codec_target_reads_its_own_name_only() {
        for target in CodecTarget::ALL {
            assert_eq!(target.name().parse::<CodecTarget>().unwrap(), target);
        }
        assert!("api_query".parse::<CodecTarget>().is_err());
    }

    #[test]
    fn the_head_target_has_no_replay_and_says_why() {
        let args = CodecReplayArgs {
            oracle: OracleArgs::default(),
            target: CodecTarget::ApiHead,
            corpus: PathBuf::from("none"),
            vectors: PathBuf::from("none"),
        };
        let refused = run_command(&args).unwrap_err();
        assert!(
            matches!(&refused, Error::Usage(m) if m.contains("crash-only")),
            "{refused}"
        );
    }

    #[test]
    fn the_json_subject_reads_a_transaction_as_the_reference_does_with_both_aliases() {
        let subject = subject();
        let (json, output) = transfer();
        let text = json.to_string();
        // Strictly and not, the reference's own JSON gives its id and bytes.
        for strict in [true, false] {
            let Verdict::Output(got) = answer(&subject, &text, strict) else {
                panic!("a refusal, strict {strict}");
            };
            assert_eq!(got["id"], output["id"]);
            assert_eq!(got["hex"], output["hex"]);
        }
        // The old names the factory renames give the same transaction.
        let mut old = json.clone();
        old["vendorField"] = old["memo"].take();
        old["memo"] = Value::Null;
        old.as_object_mut().unwrap().remove("memo");
        // The factory renames `payments` for transactions of type group 1 only.
        old["typeGroup"] = json!(1);
        old["asset"]["payments"] = old["asset"]["transfers"].take();
        old["asset"].as_object_mut().unwrap().remove("transfers");
        let renamed = answer(&subject, &old.to_string(), true);
        let Verdict::Output(renamed) = renamed else {
            panic!("the old names are refused: {renamed:?}");
        };
        assert_eq!(renamed["hex"], output["hex"]);
        // Text that is not a transaction is refused, strictly and not.
        for refused in ["", "{", "[]", "null", "{\"type\":6}"] {
            for strict in [true, false] {
                assert!(
                    matches!(answer(&subject, refused, strict), Verdict::Rejected { .. }),
                    "{refused:?} strict {strict}"
                );
            }
        }
        // Without its id a transaction is read by the non-strict rule only.
        let mut bare = json;
        bare.as_object_mut().unwrap().remove("id");
        assert!(matches!(
            answer(&subject, &bare.to_string(), true),
            Verdict::Rejected { .. }
        ));
        assert!(matches!(
            answer(&subject, &bare.to_string(), false),
            Verdict::Output(_)
        ));
    }

    #[test]
    fn an_accepted_address_is_written_as_it_was_read_and_other_text_is_not_an_address() {
        let text = "dEHxjxZybRiykTqqZUgfQoMqZZ3RxVj1dt";
        assert_eq!(address_round_trip(text), Some(Outcome::Pass));
        for other in [
            "",
            "d",
            "dEHxjxZybRiykTqqZUgfQoMqZZ3RxVj1dT",
            " dEHxjxZybRiykTqqZUgfQoMqZZ3RxVj1dt",
            "0OIl",
        ] {
            assert_eq!(address_round_trip(other), None, "{other:?}");
        }
    }

    #[test]
    fn two_refusals_agree_whatever_their_classes_and_no_verdict_difference_is_excused() {
        let classes = Outcome::ClassMismatch {
            oracle: "SyntaxError".into(),
            heartwood: "TransactionSchemaError".into(),
        };
        for op in ["address.toBuffer", "tx.fromData"] {
            assert_eq!(refined(op, classes.clone()), Outcome::Pass);
        }
        // Another operation's class mismatch stays a failure.
        assert!(refined("address.validate", classes).is_failure());
        // Heartwood accepting what the reference refuses, or refusing what it accepts, is a
        // difference in either reading: a malformed id included, which both readings refuse.
        for (oracle, heartwood) in [("rejected", "accepted"), ("accepted", "rejected")] {
            let verdicts = Outcome::VerdictMismatch {
                oracle: oracle.into(),
                heartwood: heartwood.into(),
            };
            assert!(refined("tx.fromData", verdicts).is_failure());
        }
    }

    #[test]
    fn the_inputs_of_a_corpus_come_in_name_order() {
        let scratch = crate::store::ScratchDir::new("codec-inputs").unwrap();
        for (name, bytes) in [("b", &b"2"[..]), ("a", b"1"), ("c", b"")] {
            fs::write(scratch.path().join(name), bytes).unwrap();
        }
        let names: Vec<String> = inputs(scratch.path())
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(inputs(&scratch.path().join("missing")).is_err());
    }

    /// The replay against the reference: the committed vectors' addresses and transactions, and
    /// what the targets' corpora hold of the shapes they refuse, agree on both sides.
    #[test]
    #[ignore = "integration"]
    fn the_replay_agrees_with_the_reference_on_addresses_and_transaction_json() {
        let oracle = if let Some(dir) = std::env::var_os("DIFFTEST_TEST_SOLAR_DIR") {
            OracleArgs {
                solar_dir: Some(PathBuf::from(dir)),
                ..OracleArgs::default()
            }
        } else if let Ok(image) = std::env::var("DIFFTEST_TEST_IMAGE") {
            OracleArgs {
                image: Some(image),
                ..OracleArgs::default()
            }
        } else {
            assert!(
                std::env::var_os("DIFFTEST_TEST_NO_ORACLE").is_some(),
                "set DIFFTEST_TEST_IMAGE or DIFFTEST_TEST_SOLAR_DIR, or DIFFTEST_TEST_NO_ORACLE to \
                 skip this test"
            );
            return;
        };
        let scratch = crate::store::ScratchDir::new("codec-replay").unwrap();
        let (json, _) = transfer();
        let mut old = json.clone();
        old["vendorField"] = old["memo"].clone();
        old["typeGroup"] = json!(1);
        old["asset"]["payments"] = old["asset"]["transfers"].clone();
        old["asset"].as_object_mut().unwrap().remove("transfers");
        old.as_object_mut().unwrap().remove("memo");
        let mut cases = vec![
            (
                "address_decode",
                "dEHxjxZybRiykTqqZUgfQoMqZZ3RxVj1dt".to_owned(),
            ),
            (
                "address_decode",
                "SNAgA2XCRZDKfm5Vu9h4KR1bZw5xn9EiC3".to_owned(),
            ),
            (
                "address_decode",
                "dEHxjxZybRiykTqqZUgfQoMqZZ3RxVj1dT".to_owned(),
            ),
            ("address_decode", String::new()),
            ("address_decode", "0OIl".to_owned()),
            ("transaction_json", json.to_string()),
            ("transaction_json", old.to_string()),
            ("transaction_json", "{".to_owned()),
            ("transaction_json", "[]".to_owned()),
            ("transaction_json", "{\"type\":6,\"fee\":\"1\"}".to_owned()),
        ];
        // A malformed id: both readings refuse it on both sides.
        let mut malformed_id = json.clone();
        malformed_id["id"] = json!("zz");
        let malformed_id = malformed_id.to_string();
        let mut bare = json;
        bare.as_object_mut().unwrap().remove("id");
        cases.push(("transaction_json", bare.to_string()));
        // The same transaction with a byte in its memo that is not UTF-8: read as the API reads a
        // body, with U+FFFD in its place, on both sides.
        let mut odd = bare;
        odd["memo"] = json!("0MARK0");
        let text = odd.to_string();
        let at = text.find("0MARK0").unwrap();
        let mut odd_memo = text.as_bytes()[..at].to_vec();
        odd_memo.extend_from_slice(b"a\xffb");
        odd_memo.extend_from_slice(&text.as_bytes()[at + 6..]);
        for target in ["address_decode", "transaction_json"] {
            let dir = scratch.path().join(target);
            fs::create_dir_all(&dir).unwrap();
            for (i, (t, text)) in cases.iter().enumerate() {
                if *t == target {
                    fs::write(dir.join(format!("{i:02}")), text).unwrap();
                }
            }
            // Inputs that are not UTF-8 text are compared too, read with U+FFFD on both sides.
            fs::write(dir.join("zz-binary"), [0xff, 0xfe, 0x7b]).unwrap();
            if target == "transaction_json" {
                fs::write(dir.join("zz-memo"), &odd_memo).unwrap();
                fs::write(dir.join("id-malformed"), &malformed_id).unwrap();
            }
            let args = CodecReplayArgs {
                oracle: oracle.clone(),
                target: target.parse().unwrap(),
                corpus: dir,
                vectors: vectors(),
            };
            let replayed = run_command(&args).unwrap();
            assert!(
                replayed.report.passes(true),
                "{target}: {}",
                replayed.report
            );
            assert!(
                replayed.accepted >= 1,
                "{target}: the reference accepted nothing"
            );
            if target == "address_decode" {
                // Of the six inputs, only the devnet's address validates for its network byte.
                assert_eq!(replayed.accepted, 1, "{target}: the valid addresses");
            }
            let odd: Vec<_> = replayed
                .report
                .results
                .iter()
                .filter(|r| r.label.starts_with("zz-"))
                .collect();
            assert!(!odd.is_empty(), "{target}: the inputs that are not UTF-8");
            if target == "transaction_json" {
                let id: Vec<_> = replayed
                    .report
                    .results
                    .iter()
                    .filter(|r| r.label.starts_with("id-malformed"))
                    .collect();
                assert_eq!(id.len(), 2, "the malformed id, strictly and not");
                for result in id {
                    assert_eq!(result.outcome, Outcome::Pass, "{}", result.label);
                }
            }
            for result in odd {
                assert_eq!(
                    result.outcome,
                    Outcome::Pass,
                    "{target}: {} is compared",
                    result.label
                );
            }
        }
    }
}
