//! Evaluating needs: what a row, a coverage level or a scenario level reads from the records and
//! the ledger, and the verdict on each.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use super::criteria::{Criteria, Need, Scenario, SeedsOf};
use super::git::Validity;
use super::record::{GateRecord, GateResult};
use super::store::{Evidence, Item};
use super::totals::{self, Totals, group};

/// Whether a record names `term`: one of the words of its variables is `term` (so `RUN=mixed`
/// does not match `RUN=mixed-short`), or its command contains it. A term with `{n}` in it is a
/// pattern for a variable's whole word, `{n}` standing for a number (`NAME=live-mixed-{n}` names
/// `NAME=live-mixed-3`, not `NAME=live-mixed-short-3`).
pub(super) fn mentions(record: &GateRecord, term: &str) -> bool {
    if term.contains(NUMBER) {
        return record
            .variables
            .split_whitespace()
            .any(|word| matches_pattern(word, term));
    }
    record.variables.split_whitespace().any(|word| word == term) || record.command.contains(term)
}

/// The place of a number in a term's pattern.
const NUMBER: &str = "{n}";

/// The variables a record's command line set, by name: its `KEY=value` words. A value with spaces
/// is cut at its first space, as make keeps it, which no run's identity needs.
pub(super) fn assignments(variables: &str) -> BTreeMap<&str, &str> {
    let name = |key: &str| {
        !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    };
    variables
        .split_whitespace()
        .filter_map(|word| word.split_once('='))
        .filter(|(key, _)| name(key))
        .collect()
}

/// The variables that say how this machine ran a command, not which run it was: the recorder's,
/// the loopback block's, the oracle's and the toolchain's, the tree's places and strictness (a
/// need asks for `--strict` in the command itself).
const INCIDENTAL: [&str; 17] = [
    "DEVNET_PREFIX",
    "HEARTWOOD_DEVNET_PREFIX",
    "HEARTWOOD_TEST_LOOPBACK",
    "IMAGE",
    "ORACLE_ARGS",
    "STRICT",
    "TOOLCHAIN_ENV",
    "SOLAR_REPO",
    "LOCAL_SOLAR",
    "LEDGER_DIR",
    "S1_CRITERIA",
    "FORGING_RESULTS",
    "DIFFTEST",
    "DIFFTEST_DOCKER",
    "GATED",
    "REPLACE",
    "CARGO_BUILD_JOBS",
];

/// The run a record is of. With `keys`, the values its command line gave them, in their order
/// (none when one of them is not set: such a record names no run); without, every variable of
/// its command line that is not incidental, in name order, so that the order of the words and
/// the machine's own settings do not make two runs of one.
pub(super) fn identity(record: &GateRecord, keys: &[String]) -> Option<String> {
    let set = assignments(&record.variables);
    if keys.is_empty() {
        let kept: Vec<String> = set
            .iter()
            .filter(|(key, value)| {
                !value.is_empty() && !key.starts_with("GATE_") && !INCIDENTAL.contains(key)
            })
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        return Some(kept.join(" "));
    }
    let mut named = Vec::new();
    for key in keys {
        let value = set.get(key.as_str()).filter(|value| !value.is_empty())?;
        named.push(format!("{key}={value}"));
    }
    Some(named.join(" "))
}

/// Whether a record's command line set `variable` to a value.
fn sets(record: &GateRecord, variable: &str) -> bool {
    assignments(&record.variables)
        .get(variable)
        .is_some_and(|value| !value.is_empty())
}

/// What a comparison of a suite's two runs judges for every scenario of the suite at once, by
/// the verdict names it gives them (`suite/chain`, `suite/steps`): each run's chain held to the
/// reference's, and each run's scripted steps. A scenario of a comparison shows nothing unless
/// both passed.
const SHARED_VERDICTS: [&str; 2] = ["chain", "steps"];

/// The verdict a record gives scenario `id` (a scenario file's id; a comparison names it after
/// its suite, `suite/id`), when the record ran or compared it. A comparison's verdict of a
/// scenario is the worst of the scenario's own and of its suite's shared verdicts
/// ([`SHARED_VERDICTS`]), one that is not there counting as `skipped`.
fn scenario_verdict<'r>(record: &'r GateRecord, id: &str) -> Option<&'r str> {
    let suffix = format!("/{id}");
    let (name, own) = record
        .scenarios
        .iter()
        .find(|(name, _)| *name == id || name.ends_with(&suffix))?;
    let Some(suite) = name.strip_suffix(&suffix) else {
        return Some(own.as_str());
    };
    let rank = |verdict: &str| match verdict {
        "pass" => 0,
        "skipped" => 1,
        _ => 2,
    };
    let mut worst = own.as_str();
    for shared in SHARED_VERDICTS {
        let verdict = record
            .scenarios
            .get(&format!("{suite}/{shared}"))
            .map_or("skipped", String::as_str);
        if rank(verdict) > rank(worst) {
            worst = verdict;
        }
    }
    Some(worst)
}

/// What a record need asks of the records.
#[derive(Debug, Clone, Copy)]
struct Asked<'n> {
    target: &'n str,
    contains: &'n [String],
    without: &'n [String],
    min: u32,
    distinct: bool,
    distinct_by: &'n [String],
    scenario: Option<&'n str>,
    seeds_of: Option<&'n SeedsOf>,
}

/// The number a record's variables give the first `{n}` of the numbered term `term` (the digits
/// of the variable's word that the pattern matches), if a word matches it.
fn number_in<'r>(record: &'r GateRecord, term: &str) -> Option<&'r str> {
    let (before, _) = term.split_once(NUMBER)?;
    record
        .variables
        .split_whitespace()
        .filter(|word| matches_pattern(word, term))
        .find_map(|word| {
            let rest = word.strip_prefix(before)?;
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            rest.get(..digits)
        })
}

/// Whether `word` is `pattern` with each `{n}` replaced by one or more digits.
fn matches_pattern(word: &str, pattern: &str) -> bool {
    let mut parts = pattern.split(NUMBER);
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(mut rest) = word.strip_prefix(first) else {
        return false;
    };
    for part in parts {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return false;
        }
        let Some(after) = rest[digits..].strip_prefix(part) else {
            return false;
        };
        rest = after;
    }
    rest.is_empty()
}

/// A need's status, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Evidence exists and it is failed, or from a dirty or foreign tree.
    Fail,
    /// No evidence yet, or only evidence that no longer counts (stale by the path rule, or a
    /// run below its reach): such a run is no evidence, and it is not a failed run either.
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
    /// The rows that are pending on nothing but what a person shows.
    persons: RefCell<BTreeSet<String>>,
    /// The final gate: what a row waits on is due, so missing evidence fails it.
    last: bool,
}

/// The start of a manual need's detail.
const BY_A_PERSON: &str = "to be shown by a person: ";

/// A scope row's status when its KEEP or ADD row has code and a named test.
const SETTLED_MAPPED: &str = "mapped";
/// A scope row's status when its REMOVE or not-present row is shown absent.
const SETTLED_ABSENT: &str = "absent";
/// A scope row's status when its DEFER row is shown unbuilt.
const SETTLED_UNBUILT: &str = "unbuilt";
/// A scope row's status when the code departs from the row's class.
const DEVIATION: &str = "deviation";

impl<'a> Judge<'a> {
    /// A judge over `evidence`.
    pub fn new(criteria: &'a Criteria, evidence: &'a Evidence) -> Judge<'a> {
        Judge {
            criteria,
            evidence,
            totals: totals::totals(criteria, evidence),
            rows: RefCell::default(),
            persons: RefCell::default(),
            last: false,
        }
    }

    /// The same judge for the final gate, when `last`: a row's missing evidence fails it,
    /// whatever the row waits on.
    pub fn at_the_end(self, last: bool) -> Judge<'a> {
        Judge { last, ..self }
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
            Status::Missing if row.waits.is_some() && !self.last => RowStatus::Pending,
            Status::Missing | Status::Fail => RowStatus::Fail,
        };
        if status == RowStatus::Pending && self.only_a_person_left(&needs) {
            self.persons.borrow_mut().insert(id.to_owned());
        }
        let verdict = RowVerdict { status, needs };
        self.rows
            .borrow_mut()
            .insert(id.to_owned(), Some(verdict.clone()));
        verdict
    }

    /// Whether every need of `needs` that is not met is one a person shows, or a row that waits
    /// only on a person.
    fn only_a_person_left(&self, needs: &[Verdict]) -> bool {
        let persons = self.persons.borrow();
        needs.iter().filter(|v| v.status != Status::Pass).all(|v| {
            v.detail.starts_with(BY_A_PERSON)
                || (v.status == Status::Pending
                    && v.detail
                        .strip_prefix("row ")
                        .is_some_and(|id| persons.contains(id)))
        })
    }

    /// Whether row `id`, judged already, is pending on nothing but what a person shows.
    pub fn waits_only_on_a_person(&self, id: &str) -> bool {
        self.persons.borrow().contains(id)
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
                without,
                min,
                distinct,
                distinct_by,
                scenario,
                seeds_of,
            } => vec![self.record(&Asked {
                target,
                contains,
                without,
                min: *min,
                distinct: *distinct,
                distinct_by,
                scenario: scenario.as_deref(),
                seeds_of: seeds_of.as_ref(),
            })],
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
                detail: format!("{BY_A_PERSON}{what}"),
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

    /// The scope audit: each of the criteria's row ids the first cell of exactly one row of the
    /// audit's tables, and each such row's status (its last cell) settled: `mapped`, `absent` or
    /// `unbuilt`, or a `deviation` the criteria's scope accepts with the decision that accepts it.
    /// A row missing or named twice fails; a row not settled waits on a person, named.
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
        let mut statuses: BTreeMap<&str, &str> = BTreeMap::new();
        for line in text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix('|')
                && let Some((cell, _)) = rest.split_once('|')
            {
                let id = cell.trim();
                *firsts.entry(id).or_insert(0) += 1;
                let status = (rest.trim_end().trim_end_matches('|').rsplit('|').next())
                    .unwrap_or_default()
                    .trim();
                statuses.insert(id, status);
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
        if !missing.is_empty() || !twice.is_empty() {
            return Verdict {
                status: Status::Fail,
                detail: format!(
                    "scope audit: {} rows absent ({}), {} listed twice ({})",
                    missing.len(),
                    shorten(&missing),
                    twice.len(),
                    shorten(&twice)
                ),
            };
        }
        let mut accepted = Vec::new();
        let mut unsettled = Vec::new();
        for id in &scope.row_ids {
            let status = statuses.get(id.as_str()).copied().unwrap_or_default();
            match (status, scope.accepted.get(id)) {
                (SETTLED_MAPPED | SETTLED_ABSENT | SETTLED_UNBUILT, _) => {}
                (DEVIATION, Some(by)) if !by.trim().is_empty() => {
                    accepted.push(format!("{id} (a deviation accepted by {})", by.trim()));
                }
                _ => unsettled.push(format!("{id} ({status})")),
            }
        }
        if !unsettled.is_empty() {
            let named: Vec<&str> = unsettled.iter().map(String::as_str).collect();
            return Verdict {
                status: Status::Pending,
                detail: format!(
                    "{BY_A_PERSON}the scope audit's rows whose status is not mapped, absent or \
                     unbuilt, each settled in the audit or, a deviation, accepted in the criteria's \
                     scope with the decision that accepts it: {}",
                    shorten(&named)
                ),
            };
        }
        let mut detail = format!(
            "scope audit: each of {} rows appears once, every row settled",
            scope.row_ids.len()
        );
        if !accepted.is_empty() {
            detail.push_str(&format!("; {}", accepted.join(", ")));
        }
        Verdict {
            status: Status::Pass,
            detail,
        }
    }

    fn record(&self, asked: &Asked<'_>) -> Verdict {
        let mut label = if asked.contains.is_empty() {
            asked.target.to_owned()
        } else {
            format!("{} [{}]", asked.target, asked.contains.join(" "))
        };
        if !asked.without.is_empty() {
            label.push_str(&format!(" without {}", asked.without.join(", ")));
        }
        if let Some(id) = asked.scenario {
            label.push_str(&format!(" scenario {id}"));
        }
        // A scenario's need reads the records that ran or compared that scenario, and its verdict
        // there; any other need, the record's own result.
        let verdict_of = |i: &Item<GateRecord>| -> Option<GateResult> {
            match asked.scenario {
                None => Some(i.value.result),
                Some(id) => match scenario_verdict(&i.value, id)? {
                    "pass" => Some(GateResult::Pass),
                    "fail" => Some(GateResult::Fail),
                    _ => None,
                },
            }
        };
        // The runs a numbered need follows: the seeds of the passing records of its query; a
        // record of another number is not read.
        let numbered = asked.contains.iter().find(|term| term.contains(NUMBER));
        let seeds: Option<BTreeSet<String>> = asked.seeds_of.map(|query| {
            (self.evidence.records.iter())
                .filter(|i| i.value.target == query.target)
                .filter(|i| query.contains.iter().all(|c| mentions(&i.value, c)))
                .filter(|i| i.validity.counts() && i.value.result == GateResult::Pass)
                .filter_map(|i| {
                    assignments(&i.value.variables)
                        .get("SEED")
                        .map(|s| (*s).to_owned())
                })
                .filter(|seed| !seed.is_empty())
                .collect()
        });
        let followed = |i: &Item<GateRecord>| match (&seeds, numbered) {
            (Some(seeds), Some(term)) => {
                number_in(&i.value, term).is_some_and(|n| seeds.contains(n))
            }
            _ => true,
        };
        let matching: Vec<&Item<GateRecord>> = self
            .evidence
            .records
            .iter()
            .filter(|i| i.value.target == asked.target)
            .filter(|i| asked.contains.iter().all(|c| mentions(&i.value, c)))
            .filter(|i| !asked.without.iter().any(|v| sets(&i.value, v)))
            .filter(|i| {
                asked
                    .scenario
                    .is_none_or(|id| scenario_verdict(&i.value, id).is_some())
            })
            .filter(|i| followed(i))
            .collect();
        let valid = |result: GateResult| {
            matching
                .iter()
                .copied()
                .filter(move |i| i.validity.counts() && verdict_of(i) == Some(result))
        };
        // Failed runs, grouped by the run they are of. A failed run never counts. One that a
        // person explained with a cause outside the tree's code may be repeated once: a later
        // passing record of the same run supersedes it. An unexplained failure, a run that failed
        // twice, and an explained one not yet repeated block the need.
        let run_of = |i: &Item<GateRecord>| {
            identity(&i.value, asked.distinct_by)
                .or_else(|| identity(&i.value, &[]))
                .unwrap_or_default()
        };
        let mut failures: BTreeMap<String, Vec<&Item<GateRecord>>> = BTreeMap::new();
        for failed in valid(GateResult::Fail) {
            failures.entry(run_of(failed)).or_default().push(failed);
        }
        let (mut blocking, mut superseded) = (Vec::new(), Vec::new());
        for (run, failed) in &failures {
            let [only] = failed.as_slice() else {
                blocking.extend(failed.iter().map(|f| (*f, "the run failed more than once")));
                continue;
            };
            let name = only
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            let explained = name.and_then(|n| self.criteria.explained.get(&n));
            let repeated = valid(GateResult::Pass)
                .any(|ok| run_of(ok) == *run && ok.value.started > only.value.started);
            match (explained, repeated) {
                (None, _) => blocking.push((*only, "no explanation")),
                (Some(_), false) => blocking.push((*only, "explained, not repeated yet")),
                (Some(why), true) => superseded.push((*only, why)),
            }
        }
        if let Some((first, why)) = blocking.first() {
            return Verdict {
                status: Status::Fail,
                detail: format!(
                    "{label}: {} failed record{}, such as {} ({why})",
                    blocking.len(),
                    plural(blocking.len()),
                    first.path.display()
                ),
            };
        }
        let mut counted: Vec<&Item<GateRecord>> = valid(GateResult::Pass).collect();
        let mut unnamed = 0usize;
        if asked.distinct {
            let mut seen = BTreeSet::new();
            counted.retain(|i| match identity(&i.value, asked.distinct_by) {
                Some(run) => seen.insert(run),
                None => {
                    unnamed += 1;
                    false
                }
            });
        }
        let mut notes = String::new();
        if unnamed > 0 {
            notes.push_str(&format!(
                "; {unnamed} record{} without {} not counted",
                plural(unnamed),
                asked.distinct_by.join(" and ")
            ));
        }
        if let Some((first, why)) = superseded.first() {
            notes.push_str(&format!(
                "; {} explained failure{} repeated, such as {} ({})",
                superseded.len(),
                plural(superseded.len()),
                first.path.display(),
                why.cause
            ));
        }
        let have = counted.len();
        let what = if asked.distinct { "run" } else { "record" };
        // Each run the need follows needs a passing record of its own.
        let unjudged: Vec<String> = match (&seeds, numbered) {
            (Some(seeds), Some(term)) => {
                let judged: BTreeSet<&str> = (counted.iter())
                    .filter_map(|i| number_in(&i.value, term))
                    .collect();
                (seeds.iter())
                    .filter(|seed| !judged.contains(seed.as_str()))
                    .map(|seed| term.replace(NUMBER, seed))
                    .collect()
            }
            _ => Vec::new(),
        };
        if let Some(query) = asked.seeds_of {
            notes.push_str(&format!(
                "; the runs of {} [{}]: {}",
                query.target,
                query.contains.join(" "),
                seeds.as_ref().map_or(0, BTreeSet::len)
            ));
        }
        if have >= asked.min as usize && unjudged.is_empty() {
            return Verdict {
                status: Status::Pass,
                detail: format!("{label}: {have} passing {what}{}{notes}", plural(have)),
            };
        }
        if !unjudged.is_empty() {
            notes.push_str(&format!("; no passing record of {}", unjudged.join(", ")));
        }
        let stale: Vec<&&Item<GateRecord>> =
            matching.iter().filter(|i| !i.validity.counts()).collect();
        if let Some(first) = stale.first() {
            // A record that went stale is no longer evidence, but it is no failure: the need is
            // missing again. One from a dirty tree or from outside this history is a fault, and
            // the detail names it first.
            let fault = stale.iter().find(|i| !i.validity.is_superseded());
            let only_stale = fault.is_none();
            let first = fault.unwrap_or(first);
            return Verdict {
                status: if only_stale {
                    Status::Missing
                } else {
                    Status::Fail
                },
                detail: format!(
                    "{label}: {have} of {} counted; {} record{} no longer count, such as {} ({}){notes}",
                    asked.min,
                    stale.len(),
                    plural(stale.len()),
                    first.path.display(),
                    describe(&first.validity)
                ),
            };
        }
        Verdict {
            status: Status::Missing,
            detail: format!("{label}: {have} of {} passing {what}s{notes}", asked.min),
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
        let (mut stale, mut unreached) = (0usize, 0usize);
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
                stale += total.stale.len();
                unreached += total.unreached.len();
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
            // A short total is missing evidence, whatever runs no longer count: a failed run is
            // the failure (above), a stale or reach-short one is simply not counted.
            Verdict {
                status: Status::Missing,
                detail: format!(
                    "{label}: short: {}{}",
                    short.join("; "),
                    not_counted(stale, unreached)
                ),
            }
        }
    }

    fn fuzz(&self, ids: &Option<Vec<String>>, hours: Option<f64>) -> Verdict {
        let need = hours.unwrap_or(self.criteria.totals.target_cpu_hours);
        let mut short = Vec::new();
        let mut failed = Vec::new();
        let mut stale = 0usize;
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
                stale += total.stale.len();
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
                status: Status::Missing,
                detail: format!(
                    "{label}: short: {}{}",
                    short.join("; "),
                    not_counted(stale, 0)
                ),
            }
        }
    }
}

/// The runs of a short total that were not counted, for its detail: stale ones and ones below
/// their reach.
fn not_counted(stale: usize, unreached: usize) -> String {
    let mut parts = Vec::new();
    if stale > 0 {
        parts.push(format!("{stale} stale run{}", plural(stale)));
    }
    if unreached > 0 {
        parts.push(format!(
            "{unreached} run{} below the reach",
            plural(unreached)
        ));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({} not counted)", parts.join(", "))
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

    #[test]
    fn a_run_is_named_by_its_variables_in_any_order_without_the_machine_s_settings() {
        let one = record("SEED=1 RUN=mixed-short", "x");
        let other = record(
            "IMAGE=img DEVNET_PREFIX=127.3.0 GATE_X=y RUN=mixed-short SEED=1",
            "x",
        );
        assert_eq!(identity(&one, &[]), identity(&other, &[]));
        assert_eq!(identity(&one, &[]).unwrap(), "RUN=mixed-short SEED=1");
        let by_seed = ["SEED".to_owned()];
        assert_eq!(identity(&other, &by_seed).unwrap(), "SEED=1");
        assert_eq!(identity(&record("RUN=mixed-short", "x"), &by_seed), None);
        assert_eq!(identity(&record("SEED= RUN=x", "x"), &by_seed), None);
        assert!(sets(&record("SUITE=network SEED=1", "x"), "SUITE"));
        assert!(!sets(&record("SUITE= SEED=1", "x"), "SUITE"));
    }

    #[test]
    fn a_numbered_term_names_one_run_name_and_not_a_longer_one() {
        let named = |variables: &str, term: &str| mentions(&record(variables, "x"), term);
        assert!(named("NAME=live-mixed-3", "NAME=live-mixed-{n}"));
        assert!(named(
            "NAME=live-mixed-17 PROCESSOR=1",
            "NAME=live-mixed-{n}"
        ));
        assert!(!named("NAME=live-mixed-short-3", "NAME=live-mixed-{n}"));
        assert!(!named("NAME=live-mixed-split-3", "NAME=live-mixed-{n}"));
        assert!(!named("NAME=live-mixed-", "NAME=live-mixed-{n}"));
        assert!(!named("NAME=live-mixed-3x", "NAME=live-mixed-{n}"));
        assert!(named(
            "NAME=live-mixed-split-4-minority",
            "NAME=live-mixed-split-{n}-minority"
        ));
        assert!(!named(
            "NAME=live-mixed-split-4",
            "NAME=live-mixed-split-{n}-minority"
        ));
        // A pattern is never looked for in the command.
        assert!(!mentions(&record("", "run live-mixed-3"), "live-mixed-{n}"));
    }

    #[test]
    fn a_numbered_term_gives_the_number_of_the_word_it_names() {
        let named = |variables: &str, term: &str| {
            number_in(&record(variables, "x"), term).map(str::to_owned)
        };
        assert_eq!(
            named("NAME=live-mixed-31 PROCESSOR=1", "NAME=live-mixed-{n}").as_deref(),
            Some("31")
        );
        assert_eq!(
            named(
                "NAME=live-mixed-split-4-minority",
                "NAME=live-mixed-split-{n}-minority"
            )
            .as_deref(),
            Some("4")
        );
        assert_eq!(
            named("NAME=live-mixed-short-3", "NAME=live-mixed-{n}"),
            None
        );
        assert_eq!(named("RUN=mixed SEED=3", "NAME=live-mixed-{n}"), None);
    }
}
