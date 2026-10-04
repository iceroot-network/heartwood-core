//! Replay a corpus of inputs through the oracle and Heartwood.
//!
//! `difftest oracle replay OP CORPUS` sends each file of a corpus directory (raw bytes, for
//! example a fuzz corpus) as the `hex` input of an operation such as `tx.fromBytes` or
//! `block.fromBytes` to `oracle-server.js`, runs Heartwood's subject for the operation on the same
//! input, and judges each case as L0 judges a golden-vector record ([`l0::judge_case`]). A
//! milestone file given with the corpus is passed to both sides as the case's milestones.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::errors::{Error, Result};
use crate::levels::l0;
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Report};
use crate::subject::{Case, Registry};
use crate::vectors::Expected;

/// What to replay.
#[derive(Debug, Clone, Copy)]
pub struct Replay<'a> {
    /// The oracle operation, which takes `{"hex": ...}` as its input.
    pub op: &'a str,
    /// The directory of input files; each file is one case.
    pub corpus: &'a Path,
    /// The rule height of every case.
    pub height: u64,
    /// A milestone file (JSON text) the cases run under, instead of the devnet's.
    pub milestones: Option<&'a str>,
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
        "reference {} {} (tree {}), Node {}, bcrypto {}",
        pong["solar"]["version"].as_str().unwrap_or("unknown"),
        short(&pong["solar"]["commit"]),
        short(&pong["solar"]["tree"]),
        pong["node"].as_str().unwrap_or("unknown"),
        pong["bcrypto"].as_str().unwrap_or("unknown"),
    )
}

/// The judged cases of a replay, and how many inputs the reference accepted.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    /// The judged cases.
    pub report: Report,
    /// How many inputs the reference accepted.
    pub accepted: usize,
}

/// Replay every file of `replay.corpus`, in name order.
pub fn run(
    server: &mut OracleServer,
    registry: &Registry,
    replay: &Replay<'_>,
) -> Result<ReplayReport> {
    let mut names: Vec<String> = fs::read_dir(replay.corpus)
        .map_err(|e| Error::io(replay.corpus, e))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|e| Error::io(replay.corpus, e))
        })
        .collect::<Result<_>>()?;
    names.sort();
    let source = describe_ping(&server.ping()?);
    let corpus_name = replay
        .corpus
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut results = Vec::with_capacity(names.len());
    let mut accepted = 0;
    for (index, name) in names.iter().enumerate() {
        let path = replay.corpus.join(name);
        let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let mut input = json!({ "hex": heartwood_crypto::utils::hex::encode(&bytes) });
        if let Some(milestones) = replay.milestones {
            input["milestones"] = json!(milestones);
        }
        let expected = match server.call(replay.op, "devnet", replay.height, &input)? {
            Ok(output) => {
                accepted += 1;
                Expected::Output(output)
            }
            Err(error) => Expected::Error(error),
        };
        let case = Case {
            op: replay.op,
            network: "devnet",
            height: replay.height,
            input: &input,
        };
        results.push(CaseResult {
            file: corpus_name.clone(),
            line: index + 1,
            op: replay.op.to_owned(),
            label: name.clone(),
            height: replay.height,
            outcome: l0::judge_case(&case, &expected, registry),
        });
    }
    Ok(ReplayReport {
        report: Report {
            title: format!("Replay of {} over {}", replay.op, replay.corpus.display()),
            source,
            files: 1,
            results,
        },
        accepted,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::cli::OracleArgs;
    use crate::report::Outcome;
    use crate::store::ScratchDir;
    use crate::subject;
    use crate::subjects::Devnet;
    use crate::vectors::VectorSet;

    fn manifest_dir() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    fn vectors() -> PathBuf {
        manifest_dir().join("../../packages/crypto/tests/vectors")
    }

    /// The inputs kept from the long fuzz corpora's replays, by target.
    fn kept(target: &str) -> PathBuf {
        manifest_dir()
            .join("tests/fixtures/corpus-replay")
            .join(target)
    }

    /// The oracle of the tests that need the reference: the local checkout or the image the
    /// environment names, or `None` (the test is skipped) when DIFFTEST_TEST_NO_ORACLE is set.
    fn test_oracle() -> Option<OracleServer> {
        let args = if let Some(dir) = std::env::var_os("DIFFTEST_TEST_SOLAR_DIR") {
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
            return None;
        };
        let oracle = args.resolve(|name| std::env::var(name).ok()).unwrap();
        Some(oracle.server().unwrap())
    }

    fn registry() -> Registry {
        let set = VectorSet::load(&vectors()).unwrap();
        let devnet = Devnet::from_vectors(&set)
            .unwrap()
            .unwrap()
            .at_current_time()
            .unwrap();
        subject::heartwood(Some(devnet))
    }

    /// The bytes of the reference's transaction vector `name` in `file`.
    fn vector_bytes(file: &str, name: &str) -> Vec<u8> {
        let text = fs::read_to_string(vectors().join(file)).unwrap();
        text.lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|record| record["name"] == name)
            .and_then(|record| {
                heartwood_crypto::utils::hex::decode(record["input"]["hex"].as_str()?).ok()
            })
            .unwrap()
    }

    /// A multi-signature entry of participant `index`.
    fn entry(index: u8) -> Vec<u8> {
        let mut entry = vec![0x11; 65];
        entry[0] = index;
        entry
    }

    /// A query that no route's handler reads may be any query the plugins write: the oracle
    /// answers 200 without writing it out, as the reference's server does.
    #[test]
    #[ignore = "integration"]
    fn the_oracle_answers_a_query_a_route_does_not_read_whatever_it_holds() {
        let Some(mut server) = test_oracle() else {
            return;
        };
        let mut names: Vec<_> = fs::read_dir(kept("api_query"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        names.sort();
        assert!(!names.is_empty());
        for path in names {
            // The route of the delegate lookup (choice 255), with a key whose dotted part is an
            // index of hundreds of millions: a list of that many holes in the query.
            let bytes = fs::read(&path).unwrap();
            let input = json!({ "hex": heartwood_crypto::utils::hex::encode(&bytes) });
            let answer = server.call("api.fuzzQuery", "devnet", 2, &input).unwrap();
            assert_eq!(answer, Ok(json!({ "status": 200 })), "{}", path.display());
        }
        // A route that reads its query writes it out as before.
        let input = json!({ "hex": heartwood_crypto::utils::hex::encode(b"\x00page=2") });
        let answer = server
            .call("api.fuzzQuery", "devnet", 2, &input)
            .unwrap()
            .unwrap();
        assert_eq!(answer["status"], 200);
        assert_eq!(answer["query"]["page"], 2, "{answer}");
    }

    /// The inputs that failed the long corpora's replays, and the forms around them, replay
    /// strictly with no failure: the query the oracle could not write out, and the transactions of
    /// a version the reference refuses with multi-signature entries after them, alone and in
    /// blocks.
    #[test]
    #[ignore = "integration"]
    fn the_inputs_kept_from_the_long_corpora_replay_with_no_failure() {
        let Some(mut server) = test_oracle() else {
            return;
        };
        let registry = registry();
        let scratch = ScratchDir::new("corpus-replay").unwrap();
        let copy = |target: &str| {
            let dir = scratch.path().join(target);
            fs::create_dir_all(&dir).unwrap();
            for entry in fs::read_dir(kept(target)).unwrap() {
                let path = entry.unwrap().path();
                fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
            }
            dir
        };
        let api = copy("api_query");
        let transactions = copy("transaction_decode");
        let blocks = copy("block_decode");
        // A transaction the reference accepts, with an entry after its signature (the documented
        // difference); of version 1 with one and two entries (the version refusal); of version 1
        // with two entries of one participant (the participant refusal).
        let valid = vector_bytes("V03-noncanonical-removed.jsonl", "version-0");
        let mut version_1 = valid.clone();
        version_1[1] = 1;
        let formed = [
            ("formed-documented", [valid, entry(0)].concat()),
            ("formed-version-1", [version_1.clone(), entry(0)].concat()),
            (
                "formed-version-2",
                [version_1.clone(), entry(0), entry(1)].concat(),
            ),
            (
                "formed-participant",
                [version_1, entry(7), entry(7)].concat(),
            ),
        ];
        for (name, bytes) in &formed {
            fs::write(transactions.join(name), bytes).unwrap();
        }
        let milestones = |target: &str| {
            fs::read_to_string(manifest_dir().join(format!("../../fuzz/milestones/{target}.json")))
                .unwrap()
        };
        let tx_milestones = milestones("transaction_decode");
        let block_milestones = milestones("block_decode");
        let replays = [
            ("api.fuzzQuery", &api, None),
            ("tx.fromBytes", &transactions, Some(tx_milestones.as_str())),
            ("block.fromBytes", &blocks, Some(block_milestones.as_str())),
        ];
        for (op, corpus, milestones) in replays {
            let replayed = run(
                &mut server,
                &registry,
                &Replay {
                    op,
                    corpus,
                    height: 2,
                    milestones,
                },
            )
            .unwrap();
            assert!(replayed.report.passes(true), "{op}: {}", replayed.report);
            for result in &replayed.report.results {
                let documented = result.label == "formed-documented";
                assert!(
                    if documented {
                        matches!(result.outcome, Outcome::Documented(_))
                    } else {
                        result.outcome == Outcome::Pass
                    },
                    "{op}: {}: {:?}",
                    result.label,
                    result.outcome
                );
            }
        }
    }
}
