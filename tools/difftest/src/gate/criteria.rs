//! The criteria file: the baseline gate's rows, the ledger's totals, the paths each property and
//! target tests, the coverage rows, the scenario levels and the register's entries.
//!
//! The file is JSON (`heartwood-s1-criteria/1`); every field is checked when it loads, and an
//! unknown field is an error. A row's `needs` say what evidence it reads (see [`Need`]); the
//! named lists in `evidence` are shared by rows that need the same records.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

/// The format the criteria file names.
pub const CRITERIA_FORMAT: &str = "heartwood-s1-criteria/1";

/// The criteria file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Criteria {
    /// [`CRITERIA_FORMAT`].
    pub format: String,
    /// The ledger's totals every property and target must reach.
    pub totals: Totals,
    /// The L1 properties, by name (`P1` to `P23`).
    pub properties: BTreeMap<String, PropertySpec>,
    /// The fuzz targets, by name.
    pub targets: BTreeMap<String, TargetSpec>,
    /// Paths a record of a make target tests when it names no property or target of its own.
    #[serde(default)]
    pub narrowing: BTreeMap<String, Vec<String>>,
    /// Named lists of needs, which rows and coverage levels refer to.
    #[serde(default)]
    pub evidence: BTreeMap<String, Vec<Need>>,
    /// The baseline gate's rows, in order.
    pub rows: Vec<Row>,
    /// The coverage rows.
    pub coverage: Vec<CoverageRow>,
    /// The scenarios and the evidence of each level they list.
    pub scenarios: Vec<Scenario>,
    /// The register's entries and how records are classified.
    pub register: RegisterSpec,
    /// The scope audit's document, when the criteria check one.
    #[serde(default)]
    pub scope: Option<ScopeSpec>,
    /// The names the repository and the records use, by kind (runs, suites, implementations,
    /// make targets, code names), each mapped onto the name the plans and the register give it.
    /// For people reading the table; the gate itself reads the names in the needs.
    #[serde(default)]
    pub names: BTreeMap<String, BTreeMap<String, String>>,
}

/// The totals of the long compute.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Totals {
    /// Cumulative cases each property needs.
    pub property_cases: u64,
    /// Independently seeded runs each property needs.
    pub property_runs: u32,
    /// Cumulative CPU hours each fuzz target needs.
    pub target_cpu_hours: f64,
}

/// One L1 property.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PropertySpec {
    /// A short name for tables.
    pub name: String,
    /// The paths its runs test.
    pub paths: Vec<String>,
    /// The reach a counted run must have met (`minReach`, `decisionReach`, `servedReach`).
    #[serde(default)]
    pub reach: BTreeMap<String, u64>,
}

/// One fuzz target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetSpec {
    /// The paths its runs test.
    pub paths: Vec<String>,
}

/// What a row or a coverage level reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Need {
    /// Valid, passing gate records of a make target.
    Record {
        /// The make target.
        target: String,
        /// Text every counted record's variables or command must contain.
        #[serde(default)]
        contains: Vec<String>,
        /// How many records are needed (1 when left out).
        #[serde(default = "one")]
        min: u32,
        /// Count records with the same variables once (independent runs).
        #[serde(default)]
        distinct: bool,
    },
    /// Ledger sums of L1 properties (every property when `ids` is left out).
    Properties {
        /// The properties.
        #[serde(default)]
        ids: Option<Vec<String>>,
        /// Cumulative cases each needs (the totals' when left out).
        #[serde(default)]
        cases: Option<u64>,
        /// Independently seeded runs each needs (the totals' when left out).
        #[serde(default)]
        runs: Option<u32>,
    },
    /// Ledger sums of fuzz targets (every target when `ids` is left out).
    Fuzz {
        /// The targets.
        #[serde(default)]
        ids: Option<Vec<String>>,
        /// CPU hours each needs (the totals' when left out).
        #[serde(default)]
        hours: Option<f64>,
    },
    /// Other rows of the table.
    Rows {
        /// The rows' ids.
        ids: Vec<String>,
    },
    /// Every scenario's evidence at each level it lists (the criteria's `scenarios`).
    Scenarios,
    /// The scope audit names each row of the scope table once (the criteria's `scope`).
    Scope,
    /// A named list of needs.
    Evidence {
        /// Its name in `evidence`.
        name: String,
    },
    /// Something only a person can show; the row stays pending.
    Manual {
        /// What is waiting.
        what: String,
    },
}

const fn one() -> u32 {
    1
}

/// One row of the baseline table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Row {
    /// The row's id (a short label).
    pub id: String,
    /// Where the criterion comes from.
    pub criterion: String,
    /// What the row requires.
    pub requires: String,
    /// How the gate shows it.
    pub shown_by: String,
    /// The tags `--part` selects rows by.
    #[serde(default)]
    pub parts: Vec<String>,
    /// What the row waits on, when its evidence is not all there yet: a row with nothing
    /// failed and evidence missing is pending, not failed.
    #[serde(default)]
    pub waits: Option<String>,
    /// The evidence.
    pub needs: Vec<Need>,
}

/// One of the 24 rows of the coverage plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageRow {
    /// The row's number.
    pub row: u32,
    /// The brief item.
    pub item: String,
    /// The needs of each level the row lists, by level name (`L0`, `L1`, `L2`, `L3`, `L4`,
    /// `fuzz`).
    pub levels: BTreeMap<String, Vec<Need>>,
}

/// One scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scenario {
    /// Its id (`S-01`).
    pub id: String,
    /// Its title.
    pub title: String,
    /// The needs of each level it lists, by level name (`L2`, `L3`, `L4`).
    pub levels: BTreeMap<String, Vec<Need>>,
}

/// The register's entries and the rules that classify records.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterSpec {
    /// The entries, active and deferred.
    pub entries: Vec<Entry>,
    /// The entries that were retired and need no test.
    #[serde(default)]
    pub retired: Vec<String>,
    /// How each kind of record counts its hits.
    pub records: Vec<RecordRule>,
}

/// One register entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    /// The entry's id.
    pub id: String,
    /// Whether it is active or deferred.
    pub status: EntryStatus,
    /// When it applies: the predicate that tells a run whether it may hit it.
    pub trigger: String,
    /// The code names (`Documented::*`, `ApiDifference::*`) whose hits count as this entry's.
    #[serde(default)]
    pub code_names: Vec<String>,
    /// The tests that show it.
    pub tests: Vec<TestRef>,
}

/// An entry's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryStatus {
    /// Built and in force.
    Active,
    /// Named, not built.
    Deferred,
}

/// A test that shows an entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TestRef {
    /// A `cargo test` function in a package's test file or library.
    Cargo {
        /// The package.
        package: String,
        /// The integration test file (a `--test` target) it lives in, when it is in one.
        #[serde(default)]
        test: Option<String>,
        /// The source file holding the function, relative to the tree.
        file: String,
        /// The test function's name.
        name: String,
        /// The cargo features its test file needs (`required-features` in the manifest).
        #[serde(default)]
        features: Vec<String>,
    },
    /// A valid passing gate record of a make target.
    Record {
        /// The make target.
        target: String,
        /// Text the record's variables or command must contain.
        #[serde(default)]
        contains: Vec<String>,
        /// What the record waits on, when a later run makes it: with no such record yet (none at
        /// all, or only stale ones), the entry is pending, not failed.
        #[serde(default)]
        waits: Option<String>,
    },
}

/// How the records of one make target count their hits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordRule {
    /// The make target.
    pub target: String,
    /// Text the record's variables or command must contain (all of it).
    #[serde(default)]
    pub contains: Vec<String>,
    /// The kind of run.
    pub class: RunClass,
    /// For an honest run, the code names it may hit.
    #[serde(default)]
    pub allow: Vec<String>,
}

/// What a kind of run may hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunClass {
    /// A run of the product as it is meant to run: no hit except the `allow`ed names.
    Honest,
    /// A run built to hit entries (a configuration or attack suite): its hits are counted.
    Intended,
    /// A run that shows the differences (the vectors, the properties): its hits are counted.
    Shows,
}

/// The scope audit's document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeSpec {
    /// The audit's file, relative to the criteria file (made absolute when the file loads).
    pub file: String,
    /// The row ids of the scope table, each of which the audit must name once.
    pub row_ids: Vec<String>,
}

impl Criteria {
    /// Read and check the criteria file at `path`.
    pub fn read(path: &Path) -> Result<Criteria> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let criteria: Criteria = serde_json::from_str(&text).map_err(|e| Error::Corpus {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        criteria.check().map_err(|message| Error::Corpus {
            path: path.to_path_buf(),
            message,
        })?;
        let mut criteria = criteria;
        if let Some(scope) = criteria.scope.as_mut() {
            let base = path.parent().unwrap_or_else(|| Path::new("."));
            scope.file = base.join(&scope.file).to_string_lossy().into_owned();
        }
        Ok(criteria)
    }

    /// The row `id`.
    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    fn check(&self) -> std::result::Result<(), String> {
        if self.format != CRITERIA_FORMAT {
            return Err(format!(
                "the format {:?} is not {CRITERIA_FORMAT}",
                self.format
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for row in &self.rows {
            if !seen.insert(row.id.as_str()) {
                return Err(format!("row {} is listed twice", row.id));
            }
        }
        let mut needs: Vec<&Need> = Vec::new();
        for row in &self.rows {
            needs.extend(&row.needs);
        }
        for row in &self.coverage {
            row.levels.values().for_each(|l| needs.extend(l));
        }
        for scenario in &self.scenarios {
            scenario.levels.values().for_each(|l| needs.extend(l));
        }
        for list in self.evidence.values() {
            needs.extend(list);
        }
        for need in needs {
            match need {
                Need::Evidence { name } if !self.evidence.contains_key(name) => {
                    return Err(format!("a need names the unknown evidence {name:?}"));
                }
                Need::Rows { ids } => {
                    if let Some(id) = ids.iter().find(|id| self.row(id).is_none()) {
                        return Err(format!("a need names the unknown row {id:?}"));
                    }
                }
                Need::Properties { ids: Some(ids), .. } => {
                    if let Some(id) = ids.iter().find(|id| !self.properties.contains_key(*id)) {
                        return Err(format!("a need names the unknown property {id:?}"));
                    }
                }
                Need::Fuzz { ids: Some(ids), .. } => {
                    if let Some(id) = ids.iter().find(|id| !self.targets.contains_key(*id)) {
                        return Err(format!("a need names the unknown target {id:?}"));
                    }
                }
                _ => {}
            }
        }
        for name in self.evidence.keys() {
            self.no_cycle(name, &mut Vec::new())?;
        }
        Ok(())
    }

    /// Refuse a named list that needs itself, through any chain of lists.
    fn no_cycle<'a>(
        &'a self,
        name: &'a str,
        path: &mut Vec<&'a str>,
    ) -> std::result::Result<(), String> {
        if path.contains(&name) {
            return Err(format!(
                "the evidence {name:?} needs itself ({} -> {name})",
                path.join(" -> ")
            ));
        }
        path.push(name);
        for need in self.evidence.get(name).into_iter().flatten() {
            if let Need::Evidence { name: inner } = need {
                self.no_cycle(inner, path)?;
            }
        }
        path.pop();
        Ok(())
    }

    /// The paths a record of `target`, run as `command`, tests, when the criteria narrow them:
    /// its property's (`--property P`), its fuzz target's (`fuzz run T`), or the make target's
    /// own list.
    pub fn narrowed_paths(&self, target: &str, command: &str) -> Option<Vec<String>> {
        let words: Vec<&str> = command.split_whitespace().collect();
        if let Some(at) = words.iter().position(|w| *w == "--property")
            && let Some(name) = words.get(at + 1)
            && !name.contains(',')
            && let Some(spec) = self.properties.get(*name)
        {
            return Some(spec.paths.clone());
        }
        if let Some(at) = words.windows(2).position(|w| w == ["fuzz", "run"])
            && let Some(name) = words.get(at + 2)
            && let Some(spec) = self.targets.get(*name)
        {
            return Some(spec.paths.clone());
        }
        self.narrowing.get(target).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tiny() -> Criteria {
        serde_json::from_value(serde_json::json!({
            "format": CRITERIA_FORMAT,
            "totals": {"propertyCases": 1000, "propertyRuns": 2, "targetCpuHours": 1.0},
            "properties": {"P1": {"name": "fee-floor", "paths": ["a/p1"]}},
            "targets": {"t1": {"paths": ["a/t1"]}},
            "narrowing": {"l0": ["a/l0"]},
            "rows": [
                {"id": "R1", "criterion": "c", "requires": "r", "shownBy": "s",
                 "needs": [{"kind": "record", "target": "l0"}]},
                {"id": "R2", "criterion": "c", "requires": "r", "shownBy": "s",
                 "needs": [{"kind": "rows", "ids": ["R1"]}]}
            ],
            "coverage": [],
            "scenarios": [],
            "register": {"entries": [], "records": []}
        }))
        .unwrap()
    }

    #[test]
    fn commands_narrow_their_paths() {
        let criteria = tiny();
        assert_eq!(
            criteria.narrowed_paths("l1", "target/debug/difftest run l1 --property P1 --cases 5"),
            Some(vec!["a/p1".to_owned()])
        );
        assert_eq!(
            criteria.narrowed_paths("fuzz-long", "cargo +nightly fuzz run t1 -- -fork=8"),
            Some(vec!["a/t1".to_owned()])
        );
        assert_eq!(
            criteria.narrowed_paths("l0", "target/debug/difftest run l0"),
            Some(vec!["a/l0".to_owned()])
        );
        assert_eq!(criteria.narrowed_paths("l2", "difftest run l2 x"), None);
        assert_eq!(
            criteria.narrowed_paths("l1", "difftest run l1 --property P1,P2"),
            None
        );
    }

    #[test]
    fn a_criteria_file_is_checked_when_it_loads() {
        assert!(tiny().check().is_ok());
        let mut bad = tiny();
        bad.rows[1].needs = vec![Need::Evidence {
            name: "nothing".to_owned(),
        }];
        assert!(bad.check().unwrap_err().contains("unknown evidence"));
        let mut bad = tiny();
        bad.rows[1].id = "R1".to_owned();
        assert!(bad.check().unwrap_err().contains("twice"));
    }

    #[test]
    fn a_list_of_evidence_that_needs_itself_is_refused() {
        let mut bad = tiny();
        let needs = |name: &str| {
            vec![Need::Evidence {
                name: name.to_owned(),
            }]
        };
        bad.evidence.insert("a".to_owned(), needs("b"));
        bad.evidence.insert("b".to_owned(), needs("a"));
        assert!(bad.check().unwrap_err().contains("needs itself"));
        let mut good = tiny();
        good.evidence.insert("a".to_owned(), needs("b"));
        good.evidence.insert("b".to_owned(), Vec::new());
        assert!(good.check().is_ok());
    }
}
