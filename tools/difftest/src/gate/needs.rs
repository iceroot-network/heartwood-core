//! Evaluating needs: what a row, a coverage level or a scenario level reads from the records and
//! the ledger, and the verdict on each.

use std::cell::RefCell;
use std::collections::BTreeMap;

use super::criteria::{Criteria, Need, Scenario};
use super::git::Validity;
use super::record::{GateRecord, GateResult};
use super::store::{Evidence, Item};
use super::totals::{self, Totals, group};

/// Whether a record names `term`: one of the words of its variables is `term` (so `RUN=mixed`
/// does not match `RUN=mixed-short`), or its command contains it.
pub(super) fn mentions(record: &GateRecord, term: &str) -> bool {
    record.variables.split_whitespace().any(|word| word == term) || record.command.contains(term)
}

/// A need's status, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Evidence exists and it is failed, stale or from a dirty or foreign tree.
    Fail,
    /// No evidence yet.
    Missing,
    /// Only a person can show it.
    Pending,
    /// Shown.
    Pass,
}

/// One need's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The status.
    pub status: Status,
    /// What was looked for and what was found.
    pub detail: String,
}

/// A row's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    /// Every need is met.
    Pass,
    /// Nothing failed; something is missing that the row waits on, or only a person can show.
    Pending,
    /// Something is missing, failed or stale.
    Fail,
}

/// A row's verdict with the verdicts of its needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowVerdict {
    /// The row's status.
    pub status: RowStatus,
    /// One verdict per need (a shared list or the scenarios give several).
    pub needs: Vec<Verdict>,
}

/// Judges needs against one set of evidence.
pub struct Judge<'a> {
    criteria: &'a Criteria,
    evidence: &'a Evidence,
    totals: Totals,
    rows: RefCell<BTreeMap<String, Option<RowVerdict>>>,
}

impl<'a> Judge<'a> {
    /// A judge over `evidence`.
    pub fn new(criteria: &'a Criteria, evidence: &'a Evidence) -> Judge<'a> {
        Judge {
            criteria,
            evidence,
            totals: totals::totals(criteria, evidence),
            rows: RefCell::default(),
        }
    }

    /// The ledger's sums this judge reads.
    pub fn totals(&self) -> &Totals {
        &self.totals
    }

    /// The verdict on row `id` (each row is judged once).
    pub fn row(&self, id: &str) -> RowVerdict {
        if let Some(found) = self.rows.borrow().get(id) {
            return found.clone().unwrap_or(RowVerdict {
                status: RowStatus::Fail,
                needs: vec![Verdict {
                    status: Status::Fail,
                    detail: format!("row {id} needs itself"),
                }],
            });
        }
        let Some(row) = self.criteria.row(id) else {
            return RowVerdict {
                status: RowStatus::Fail,
                needs: vec![Verdict {
                    status: Status::Fail,
                    detail: format!("no row {id}"),
                }],
            };
        };
        self.rows.borrow_mut().insert(id.to_owned(), None);
        let needs = self.needs(&row.needs);
        let worst = needs.iter().map(|v| v.status).min().unwrap_or(Status::Pass);
        let status = match worst {
            Status::Pass => RowStatus::Pass,
            Status::Pending => RowStatus::Pending,
            Status::Missing if row.waits.is_some() => RowStatus::Pending,
            Status::Missing | Status::Fail => RowStatus::Fail,
        };
        let verdict = RowVerdict { status, needs };
        self.rows
            .borrow_mut()
            .insert(id.to_owned(), Some(verdict.clone()));
        verdict
    }

    /// The verdicts of a list of needs, expanded.
    pub fn needs(&self, needs: &[Need]) -> Vec<Verdict> {
        needs.iter().flat_map(|n| self.need(n)).collect()
    }

    /// The verdicts of one need.
    pub fn need(&self, need: &Need) -> Vec<Verdict> {
        match need {
            Need::Record {
                target,
                contains,
                min,
                distinct,
            } => vec![self.record(target, contains, *min, *distinct)],
            Need::Properties { ids, cases, runs } => vec![self.properties(ids, *cases, *runs)],
            Need::Fuzz { ids, hours } => vec![self.fuzz(ids, *hours)],
            Need::Rows { ids } => ids
                .iter()
                .map(|id| {
                    let verdict = self.row(id);
                    let status = match verdict.status {
                        RowStatus::Pass => Status::Pass,
                        RowStatus::Pending => Status::Pending,
                        RowStatus::Fail => Status::Fail,
                    };
                    Verdict {
                        status,
                        detail: format!("row {id}"),
                    }
                })
                .collect(),
            Need::Evidence { name } => self
                .criteria
                .evidence
                .get(name)
                .map(|list| self.needs(list))
                .unwrap_or_default(),
            Need::Scenarios => self
                .criteria
                .scenarios
                .iter()
                .flat_map(|s| self.scenario(s))
                .collect(),
            Need::Scope => vec![self.scope()],
            Need::Manual { what } => vec![Verdict {
                status: Status::Pending,
                detail: format!("to be shown by a person: {what}"),
            }],
        }
    }

    /// The verdicts of each level of a scenario, named by the scenario and the level.
    pub fn scenario(&self, scenario: &Scenario) -> Vec<Verdict> {
        let mut out = Vec::new();
        for (level, needs) in &scenario.levels {
            for mut verdict in self.needs(needs) {
                verdict.detail = format!("{} {level}: {}", scenario.id, verdict.detail);
                out.push(verdict);
            }
        }
        out
    }

    fn scope(&self) -> Verdict {
        let Some(scope) = &self.criteria.scope else {
            return Verdict {
                status: Status::Fail,
                detail: "the criteria name no scope audit".to_owned(),
            };
        };
        let Ok(text) = std::fs::read_to_string(&scope.file) else {
            return Verdict {
                status: Status::Missing,
                detail: format!("the scope audit {} is not there", scope.file),
            };
        };
        let mut firsts: BTreeMap<&str, usize> = BTreeMap::new();
        for line in text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix('|')
                && let Some((cell, _)) = rest.split_once('|')
            {
                *firsts.entry(cell.trim()).or_insert(0) += 1;
            }
        }
        let missing: Vec<&str> = scope
            .row_ids
            .iter()
            .filter(|id| !firsts.contains_key(id.as_str()))
            .map(String::as_str)
            .collect();
        let twice: Vec<&str> = scope
            .row_ids
            .iter()
            .filter(|id| firsts.get(id.as_str()).copied().unwrap_or(0) > 1)
            .map(String::as_str)
            .collect();
        let shorten = |ids: &[&str]| -> String {
            let shown: Vec<&str> = ids.iter().take(5).copied().collect();
            if ids.len() > 5 {
                format!("{} and {} more", shown.join(", "), ids.len() - 5)
            } else {
                shown.join(", ")
            }
        };
        if missing.is_empty() && twice.is_empty() {
            Verdict {
                status: Status::Pass,
                detail: format!(
                    "scope audit: each of {} rows appears once",
                    scope.row_ids.len()
                ),
            }
        } else {
            Verdict {
                status: Status::Fail,
                detail: format!(
                    "scope audit: {} rows absent ({}), {} listed twice ({})",
                    missing.len(),
                    shorten(&missing),
                    twice.len(),
                    shorten(&twice)
                ),
            }
        }
    }

    fn record(&self, target: &str, contains: &[String], min: u32, distinct: bool) -> Verdict {
        let label = if contains.is_empty() {
            target.to_owned()
        } else {
            format!("{target} [{}]", contains.join(" "))
        };
        let matching: Vec<&Item<GateRecord>> = self
            .evidence
            .records
            .iter()
            .filter(|i| i.value.target == target)
            .filter(|i| contains.iter().all(|c| mentions(&i.value, c)))
            .collect();
        let mut counted: Vec<&Item<GateRecord>> = matching
            .iter()
            .copied()
            .filter(|i| i.validity.counts() && i.value.result == GateResult::Pass)
            .collect();
        if distinct {
            let mut seen = std::collections::BTreeSet::new();
            counted.retain(|i| seen.insert(i.value.variables.clone()));
        }
        let have = counted.len();
        if have >= min as usize {
            return Verdict {
                status: Status::Pass,
                detail: format!("{label}: {have} passing record{}", plural(have)),
            };
        }
        // A failed record blocks unless a later record with the same variables passed.
        let blocking: Vec<&&Item<GateRecord>> = matching
            .iter()
            .filter(|i| i.validity.counts() && i.value.result == GateResult::Fail)
            .filter(|failed| {
                !matching.iter().any(|ok| {
                    ok.validity.counts()
                        && ok.value.result == GateResult::Pass
                        && ok.value.variables == failed.value.variables
                        && ok.value.started > failed.value.started
                })
            })
            .collect();
        if let Some(first) = blocking.first() {
            return Verdict {
                status: Status::Fail,
                detail: format!(
                    "{label}: {} failed record{}, such as {}",
                    blocking.len(),
                    plural(blocking.len()),
                    first.path.display()
                ),
            };
        }
        let stale: Vec<&&Item<GateRecord>> =
            matching.iter().filter(|i| !i.validity.counts()).collect();
        if let Some(first) = stale.first() {
            return Verdict {
                status: Status::Fail,
                detail: format!(
                    "{label}: {have} of {min} counted; {} record{} no longer count, such as {} ({})",
                    stale.len(),
                    plural(stale.len()),
                    first.path.display(),
                    describe(&first.validity)
                ),
            };
        }
        Verdict {
            status: Status::Missing,
            detail: format!("{label}: {have} of {min} passing records"),
        }
    }

    fn properties(
        &self,
        ids: &Option<Vec<String>>,
        cases: Option<u64>,
        runs: Option<u32>,
    ) -> Verdict {
        let need_cases = cases.unwrap_or(self.criteria.totals.property_cases);
        let need_runs = runs.unwrap_or(self.criteria.totals.property_runs);
        let mut short = Vec::new();
        let mut failed = Vec::new();
        let mut stale = false;
        let mut checked = 0usize;
        for total in &self.totals.properties {
            if ids.as_ref().is_some_and(|ids| !ids.contains(&total.id)) {
                continue;
            }
            checked += 1;
            if !total.failed.is_empty() {
                failed.push(format!(
                    "{} has {} failed run{}",
                    total.id,
                    total.failed.len(),
                    plural(total.failed.len())
                ));
            } else if !total.meets(need_cases, need_runs) {
                stale |= !total.stale.is_empty() || !total.unreached.is_empty();
                short.push(format!(
                    "{} {} of {} cases, {} of {} runs",
                    total.id,
                    group(total.cases),
                    group(need_cases),
                    total.runs,
                    need_runs
                ));
            }
        }
        let label = format!(
            "{checked} propert{} at {} cases in {need_runs} runs",
            if checked == 1 { "y" } else { "ies" },
            group(need_cases)
        );
        if !failed.is_empty() {
            Verdict {
                status: Status::Fail,
                detail: format!("{label}: {}", failed.join("; ")),
            }
        } else if short.is_empty() {
            Verdict {
                status: Status::Pass,
                detail: label,
            }
        } else {
            Verdict {
                status: if stale { Status::Fail } else { Status::Missing },
                detail: format!("{label}: short: {}", short.join("; ")),
            }
        }
    }

    fn fuzz(&self, ids: &Option<Vec<String>>, hours: Option<f64>) -> Verdict {
        let need = hours.unwrap_or(self.criteria.totals.target_cpu_hours);
        let mut short = Vec::new();
        let mut failed = Vec::new();
        let mut stale = false;
        let mut checked = 0usize;
        for total in &self.totals.targets {
            if ids.as_ref().is_some_and(|ids| !ids.contains(&total.id)) {
                continue;
            }
            checked += 1;
            if !total.failed.is_empty() {
                failed.push(format!(
                    "{} has {} failed run{}",
                    total.id,
                    total.failed.len(),
                    plural(total.failed.len())
                ));
            } else if !total.meets(need) {
                stale |= !total.stale.is_empty();
                short.push(format!("{} {:.2} of {need} hours", total.id, total.hours));
            }
        }
        let label = format!(
            "{checked} fuzz target{} at {need} CPU hours",
            plural(checked)
        );
        if !failed.is_empty() {
            Verdict {
                status: Status::Fail,
                detail: format!("{label}: {}", failed.join("; ")),
            }
        } else if short.is_empty() {
            Verdict {
                status: Status::Pass,
                detail: label,
            }
        } else {
            Verdict {
                status: if stale { Status::Fail } else { Status::Missing },
                detail: format!("{label}: short: {}", short.join("; ")),
            }
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn describe(validity: &Validity) -> String {
    validity.describe()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(variables: &str, command: &str) -> GateRecord {
        serde_json::from_value(serde_json::json!({
            "format": "heartwood-gate-record/1", "target": "live-check", "command": command,
            "variables": variables, "head": "0".repeat(40), "dirty": false,
            "testedPaths": [], "machine": "host-1", "exitStatus": 0, "result": "pass",
            "started": "2026-10-01T10:00:00Z", "finished": "2026-10-01T10:04:10Z"
        }))
        .unwrap()
    }

    #[test]
    fn a_variable_is_named_by_its_whole_word_and_a_command_by_its_text() {
        let short = record(
            "RUN=mixed-short SEED=3",
            "difftest live-check --run mixed-short",
        );
        assert!(mentions(&short, "RUN=mixed-short"));
        assert!(mentions(&short, "SEED=3"));
        assert!(!mentions(&short, "RUN=mixed"));
        assert!(!mentions(&short, "SEED=30"));
        assert!(mentions(&short, "--run mixed-short"));
    }
}
