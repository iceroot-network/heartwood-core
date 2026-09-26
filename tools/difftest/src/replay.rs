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
        "Solar {} {} (tree {}), Node {}, bcrypto {}",
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
