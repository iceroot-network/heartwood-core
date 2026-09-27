//! `heartwood network generate` against the genesis vectors (`V10-genesis`): for each record's
//! seed, epoch and profile, the CLI's generator writes the chain definition into a temporary
//! directory, and its four crypto files, keys and summary must equal the record's, as the devnet
//! generator wrote them.
//!
//! The record's profile is the CLI's built-in devnet profile when it equals it (the vectors' own
//! profile, so the built-in one is what is compared), and is passed as a profile file otherwise.
//! What is compared, read back from the files written: the four files' texts and SHA-256 digests,
//! the nethash (`network.json`), the genesis id and transaction count (`genesisBlock.json`), the
//! issued amount, the seed's digest, and every derived key of `genesis-wallet.json` (the delegates,
//! the genesis, donation and test wallets, the generator), field by field and in the record's key
//! order; and `delegates.json`'s keys against the delegates' private keys. The record's `check`,
//! the reference reading its own files back, has no counterpart and is left out.

use std::fs;

use heartwood_cli::network::{DEVNET_PROFILE, generate};
use heartwood_cli::seams::{GenerateOptions, Profile};
use heartwood_kernel::SystemClock;
use serde_json::{Value, json};

use crate::compare::{Difference, diff};
use crate::errors::{Error, Result};
use crate::report::{CaseResult, Outcome, Report, describe_meta};
use crate::store::ScratchDir;
use crate::vectors::{Expected, Record, VectorSet, sha256_hex};

/// The genesis vectors' operation.
const OP: &str = "genesis.generate";

/// Compare `heartwood_cli::network::generate` with every genesis record of `vectors`.
pub fn check(vectors: &VectorSet) -> Result<Report> {
    let built_in: Value =
        serde_json::from_str(DEVNET_PROFILE).map_err(|e| Error::Fixture(e.to_string()))?;
    let mut results = Vec::new();
    for file in &vectors.files {
        for record in file.records.iter().filter(|r| r.op == OP) {
            let outcome = match &record.expected {
                Expected::Output(expected) => compare(record, expected, &built_in)?,
                Expected::Error(error) => Outcome::Skipped(format!(
                    "the reference refused the input: {}",
                    error.message
                )),
            };
            results.push(CaseResult {
                file: file.file_name(),
                line: record.line,
                op: record.op.clone(),
                label: record.label(),
                height: record.height,
                outcome,
            });
        }
    }
    Ok(Report {
        title: "network generate against the genesis vectors".into(),
        source: vectors.meta().map(describe_meta).unwrap_or_default(),
        files: vectors.files.len(),
        results,
    })
}

/// Generate the record's network and compare what was written with the record's output.
fn compare(record: &Record, expected: &Value, built_in: &Value) -> Result<Outcome> {
    let fixture = |what: &str| Error::Fixture(format!("{}: {what}", record.label()));
    let text = |value: &Value, key: &str| {
        value[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| fixture(&format!("no {key}")))
    };
    let dir = ScratchDir::new("network-generate")?;
    let profile = &record.input["profile"];
    let profile = if profile == built_in {
        Profile::Devnet
    } else {
        let path = dir.path().join("profile.json");
        fs::write(&path, profile.to_string()).map_err(|e| Error::io(&path, e))?;
        Profile::File(path)
    };
    let out = dir.path().join("out");
    let options = GenerateOptions {
        network: text(&record.input["profile"]["network"], "name")?,
        seed: text(&record.input, "seed")?,
        epoch: Some(text(&record.input, "epoch")?),
        profile,
        pub_key_hash: None,
        token: None,
        symbol: None,
        explorer: None,
        out: out.clone(),
        chain_only: true,
        force: false,
    };
    if let Err(e) = generate(&options, &SystemClock) {
        return Ok(Outcome::VerdictMismatch {
            oracle: "generated".into(),
            heartwood: e.to_string(),
        });
    }
    let read = |name: &str| {
        let path = out.join(name);
        fs::read_to_string(&path).map_err(|e| Error::io(&path, e))
    };
    let parse = |name: &str| -> Result<Value> {
        serde_json::from_str(&read(name)?).map_err(|e| fixture(&format!("{name}: {e}")))
    };
    let names = ["network", "milestones", "genesisBlock", "exceptions"]
        .map(|name| format!("crypto/{name}.json"));
    let mut files = serde_json::Map::new();
    let mut digests = serde_json::Map::new();
    for name in &names {
        let text = read(name)?;
        digests.insert(name.clone(), sha256_hex(text.as_bytes()).into());
        files.insert(name.clone(), text.into());
    }
    let (network, block) = (parse(&names[0])?, parse(&names[2])?);
    let wallets = parse("genesis-wallet.json")?;
    let transactions = block["transactions"].as_array().map_or(0, Vec::len);
    let actual = json!({
        "files": files,
        "fileSha256": digests,
        "nethash": network["nethash"],
        "genesisId": block["id"],
        "transactions": transactions,
        "issued": wallets["generator"]["issued"],
        "seedSha256": sha256_hex(text(&record.input, "seed")?.as_bytes()),
        "delegates": wallets["delegates"],
        "genesisWallets": wallets["genesis"],
        "donationWallets": wallets["donations"],
        "testWallets": wallets["wallets"],
        "generator": wallets["generator"],
    });
    let mut expected = expected.clone();
    if let Value::Object(map) = &mut expected {
        map.shift_remove("check");
    }
    let mut differences = diff(&expected, &actual);
    let keys: Vec<&Value> = expected["delegates"]
        .as_array()
        .map(|delegates| delegates.iter().map(|d| &d["privateKey"]).collect())
        .unwrap_or_default();
    let key_file = parse("delegates.json")?;
    if key_file != json!({ "keys": keys }) {
        differences.push(Difference::Value {
            path: "delegates.json".into(),
            expected: json!({ "keys": keys }),
            actual: key_file,
        });
    }
    Ok(if differences.is_empty() {
        Outcome::Pass
    } else {
        Outcome::Mismatch(differences)
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn every_genesis_vector_passes() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors");
        let report = check(&VectorSet::load(&dir).unwrap()).unwrap();
        assert_eq!(report.results.len(), 3, "{report}");
        assert_eq!(report.passed(), 3, "{report}");
    }
}
