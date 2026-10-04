//! The gate record: what one gate command ran, on which head and paths, and what it found. The
//! Makefile's gate targets write one per run through `tools/gate-record.sh` and `difftest gate
//! record`, as `<started>-<target>.json` in the records' directory. A record counts for the baseline
//! record at a later head when its head is an ancestor of it and none of its tested paths changed
//! since.
//!
//! ```json
//! {"format": "heartwood-gate-record/1", "target": "l1", "command": "target/debug/difftest run l1 ...",
//!  "variables": "L1_PROPERTY=P21 L1_CASES=256", "head": "<40 hex>", "dirty": false,
//!  "testedPaths": ["packages/api", "tools/difftest/src/strategies/api_state.rs"],
//!  "image": "heartwood-oracle:...@sha256:...", "machine": "host-1",
//!  "counts": {"compared": 256, "passed": 256, "failed": 0, "documented": 0, "skipped": 0},
//!  "exitStatus": 0, "result": "pass", "hits": {"Documented::ReplayIndexOrder": 25},
//!  "started": "2026-10-01T10:00:00Z", "finished": "2026-10-01T10:04:10Z"}
//! ```
//!
//! `hits` counts the documented differences the run met by their code names
//! (`Documented::*`, `ApiDifference::*`), never by register ids: the criteria file maps them.
//! A record of a scenario run or comparison also keeps each scenario's verdict (`scenarios`), and
//! a record of a live run's check the devnet tooling each run was made with (`toolingRuns`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

/// The format a record names.
pub const RECORD_FORMAT: &str = "heartwood-gate-record/1";

/// A gate's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateResult {
    /// The command exited 0.
    Pass,
    /// It did not.
    Fail,
}

/// The cases a gate command counted, as its report gives them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Counts {
    /// Cases compared with the reference.
    pub compared: u64,
    /// Cases that passed.
    pub passed: u64,
    /// Cases that failed.
    pub failed: u64,
    /// Cases that met a documented difference.
    pub documented: u64,
    /// Cases skipped or without a subject.
    pub skipped: u64,
}

/// One gate command's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateRecord {
    /// [`RECORD_FORMAT`].
    pub format: String,
    /// The make target.
    pub target: String,
    /// The command as it ran.
    pub command: String,
    /// The make command line's variables.
    #[serde(default)]
    pub variables: String,
    /// The head of the tree it ran on (40 hex).
    pub head: String,
    /// Whether the tree had changes beside its head.
    pub dirty: bool,
    /// The paths it tests, relative to the tree's root.
    pub tested_paths: Vec<String>,
    /// The oracle image (tag and id) it used, if any.
    #[serde(default)]
    pub image: Option<String>,
    /// The machine it ran on.
    pub machine: String,
    /// Its counts, when its report gave them.
    #[serde(default)]
    pub counts: Option<Counts>,
    /// The command's exit status.
    pub exit_status: i32,
    /// Whether the recorder was asked to stop the command (an interrupt, a terminate or a hang-up
    /// it passed on), which a run killed by anything else (the kernel's out-of-memory killer, a
    /// crash) never is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// Its verdict.
    pub result: GateResult,
    /// The documented differences it met, by code name.
    #[serde(default)]
    pub hits: BTreeMap<String, u64>,
    /// The scenarios it ran or compared, by id (`suite/id` in a comparison), with each one's
    /// verdict as its report gives it (`pass`, `fail` or `skipped`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scenarios: BTreeMap<String, String>,
    /// The devnet tooling the runs it judged were made with, all runs together: each file's path
    /// under the tooling's root, and the SHA-256 of each version of it those runs used. Records
    /// made before each run's tooling was kept apart hold this; newer ones hold `tooling_runs`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tooling: BTreeMap<String, Vec<String>>,
    /// The devnet tooling each run it judged was made with, by the run's directory (its name, or
    /// its parent's and its own): each file's path under the tooling's root and its SHA-256. A run
    /// directory without a list of its tooling is here with none.
    #[serde(
        default,
        rename = "toolingRuns",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub tooling_runs: BTreeMap<String, BTreeMap<String, String>>,
    /// When it started, in UTC (`2026-10-01T10:00:00Z`).
    pub started: String,
    /// When it finished, in UTC.
    pub finished: String,
}

impl GateRecord {
    /// The record's file name: its start and its target, safe as a file name.
    pub fn file_name(&self) -> String {
        let safe = |text: &str| -> String {
            let mapped = text.chars().map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
                _ => '-',
            });
            mapped.collect()
        };
        format!("{}-{}.json", safe(&self.started), safe(&self.target))
    }

    /// Write the record into `dir`, as [`GateRecord::file_name`]; its path.
    pub fn write(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let path = dir.join(self.file_name());
        let text = serde_json::to_string_pretty(self).map_err(|e| Error::Corpus {
            path: path.clone(),
            message: e.to_string(),
        })?;
        std::fs::write(&path, text + "\n").map_err(|e| Error::io(&path, e))?;
        Ok(path)
    }

    /// Read the record at `path`.
    pub fn read(path: &Path) -> Result<GateRecord> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let record: GateRecord = serde_json::from_str(&text).map_err(|e| Error::Corpus {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        if record.format != RECORD_FORMAT {
            return Err(Error::Corpus {
                path: path.to_path_buf(),
                message: format!("the format {:?} is not {RECORD_FORMAT}", record.format),
            });
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_as_written() {
        let record = GateRecord {
            format: RECORD_FORMAT.to_owned(),
            target: "l1".to_owned(),
            command: "target/debug/difftest run l1 --property P21".to_owned(),
            variables: "L1_PROPERTY=P21".to_owned(),
            head: "0".repeat(40),
            dirty: false,
            tested_paths: vec!["packages/api".to_owned()],
            image: None,
            machine: "host-1".to_owned(),
            counts: Some(Counts {
                compared: 256,
                passed: 256,
                ..Counts::default()
            }),
            exit_status: 0,
            interrupted: false,
            result: GateResult::Pass,
            hits: BTreeMap::from([("Documented::ReplayIndexOrder".to_owned(), 25)]),
            scenarios: BTreeMap::from([("transactions/burns".to_owned(), "pass".to_owned())]),
            tooling: BTreeMap::new(),
            tooling_runs: BTreeMap::from([(
                "suites/run-a".to_owned(),
                BTreeMap::from([("scripts/run.sh".to_owned(), "ab".repeat(32))]),
            )]),
            started: "2026-10-01T10:00:00Z".to_owned(),
            finished: "2026-10-01T10:04:10Z".to_owned(),
        };
        let dir = std::env::temp_dir().join(format!("difftest-gate-{}", std::process::id()));
        let path = record.write(&dir).unwrap();
        assert!(path.ends_with("2026-10-01T10-00-00Z-l1.json"));
        assert_eq!(GateRecord::read(&path).unwrap(), record);
        std::fs::write(&path, "{\"format\": \"heartwood-gate-record/9\"}").unwrap();
        assert!(GateRecord::read(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
