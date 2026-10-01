//! The cumulative ledger: one entry per long run, an L1 property's cases or a fuzz target's time,
//! on any machine, the earlier runs' counts entered by hand from the documents that recorded
//! them. The baseline record sums them per property (cases and independently seeded runs) and per
//! target (CPU hours, seconds times forks), each entry counting at the final head by the gate
//! records' path rule. A failed run never counts and fails its property or target; a run that a
//! signal stopped before it judged its cases counts for none of them and fails nothing.
//!
//! ```json
//! {"format": "heartwood-ledger-entry/1", "kind": "property", "id": "P21", "cases": 1000000,
//!  "seed": "20261001", "head": "<40 hex>", "image": "heartwood-oracle:...@sha256:...",
//!  "machine": "host-1", "started": "...", "finished": "...", "result": "pass",
//!  "reach": {"P13:accepted": 412}, "source": "the run's gate record"}
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::record::GateResult;

/// The format an entry names.
pub const LEDGER_FORMAT: &str = "heartwood-ledger-entry/1";

/// What an entry counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LedgerKind {
    /// An L1 property's cases.
    Property,
    /// A fuzz target's time.
    Target,
}

/// One long run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerEntry {
    /// [`LEDGER_FORMAT`].
    pub format: String,
    /// What it counts.
    pub kind: LedgerKind,
    /// The property (`P1` to `P23`) or the target's name.
    pub id: String,
    /// A property run's cases.
    #[serde(default)]
    pub cases: Option<u64>,
    /// A target run's seconds.
    #[serde(default)]
    pub seconds: Option<u64>,
    /// A target run's forks.
    #[serde(default)]
    pub forks: Option<u32>,
    /// A property run's seed.
    #[serde(default)]
    pub seed: Option<String>,
    /// The head it ran on.
    pub head: String,
    /// The oracle image it used, if any.
    #[serde(default)]
    pub image: Option<String>,
    /// The machine.
    pub machine: String,
    /// When it started, in UTC.
    pub started: String,
    /// When it finished, in UTC.
    pub finished: String,
    /// Its verdict: no failure, no crash, timeout or out-of-memory stop, nothing unexplained.
    pub result: GateResult,
    /// How often it reached each result its reach check counts (P13's and P19's).
    #[serde(default)]
    pub reach: BTreeMap<String, u64>,
    /// Whether a signal ended the run before it judged its cases (no case failed): it counts for
    /// none of its cases or time, and it is no failed run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// Where the entry comes from: a gate record, or the document its counts were entered from.
    pub source: String,
}

impl LedgerEntry {
    /// The CPU hours of a target run: its seconds times its forks, in hours.
    pub fn cpu_hours(&self) -> f64 {
        let seconds = self.seconds.unwrap_or(0) as f64;
        seconds * f64::from(self.forks.unwrap_or(1).max(1)) / 3600.0
    }
}
