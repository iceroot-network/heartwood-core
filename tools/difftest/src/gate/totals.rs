//! The ledger's sums per property and per target, and the commands over them: `ledger show`
//! (the totals), `ledger plan` (what the criteria still need) and `ledger merge` (another
//! machine's entries into the ledger).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::criteria::Criteria;
use super::flags::Flags;
use super::ledger::{LEDGER_FORMAT, LedgerEntry, LedgerKind};
use super::record::GateResult;
use super::store::{self, Evidence, Item, Sources};
use crate::errors::{Error, Result};

/// One property's totals.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyTotal {
    /// `P1` to `P23`.
    pub id: String,
    /// Cumulative cases of the counted runs, a seed's cases once (its longest run).
    pub cases: u64,
    /// Distinct seeds among the counted runs.
    pub runs: u32,
    /// The seeds of the counted runs.
    pub seeds: Vec<String>,
    /// Counted runs.
    pub counted: usize,
    /// Valid runs that failed (each one fails the property).
    pub failed: Vec<String>,
    /// Valid, passing runs left out for a reach below the criteria's.
    pub unreached: Vec<String>,
    /// Runs that no longer count, with the reason.
    pub stale: Vec<String>,
}

/// One fuzz target's totals.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetTotal {
    /// The target's name.
    pub id: String,
    /// CPU hours of the counted runs.
    pub hours: f64,
    /// Counted runs.
    pub counted: usize,
    /// Valid runs that failed (a crash, timeout or out-of-memory stop).
    pub failed: Vec<String>,
    /// Runs that no longer count, with the reason.
    pub stale: Vec<String>,
}

/// The sums of the ledger for every property and target of the criteria.
#[derive(Debug, Clone, PartialEq)]
pub struct Totals {
    /// Per property, in numeric order.
    pub properties: Vec<PropertyTotal>,
    /// Per target, in name order.
    pub targets: Vec<TargetTotal>,
    /// Entries whose property or target the criteria do not list.
    pub unlisted: Vec<String>,
}

/// The number in `P13`, for ordering.
fn property_order(id: &str) -> (u64, String) {
    let digits: String = id.chars().filter(char::is_ascii_digit).collect();
    (digits.parse().unwrap_or(u64::MAX), id.to_owned())
}

fn describe(entry: &LedgerEntry) -> String {
    let what = match entry.kind {
        LedgerKind::Property => format!(
            "{} cases, seed {}",
            group(entry.cases.unwrap_or(0)),
            entry.seed.as_deref().unwrap_or("?")
        ),
        LedgerKind::Target => format!(
            "{} s x {} forks",
            entry.seconds.unwrap_or(0),
            entry.forks.unwrap_or(1)
        ),
    };
    let head: String = entry.head.chars().take(9).collect();
    format!("{} {what} at {head} on {}", entry.id, entry.machine)
}

/// Sum the entries of `evidence` for the properties and targets of `criteria`.
pub fn totals(criteria: &Criteria, evidence: &Evidence) -> Totals {
    let mut properties: Vec<PropertyTotal> = Vec::new();
    let mut ids: Vec<&String> = criteria.properties.keys().collect();
    ids.sort_by_key(|id| property_order(id));
    for id in ids {
        let spec = &criteria.properties[id];
        let mut total = PropertyTotal {
            id: id.clone(),
            cases: 0,
            runs: 0,
            seeds: Vec::new(),
            counted: 0,
            failed: Vec::new(),
            unreached: Vec::new(),
            stale: Vec::new(),
        };
        // One seed draws the same cases in the same order, so its runs overlap: the longest
        // holds every shorter one, and only it counts.
        let mut seeds: BTreeMap<String, u64> = BTreeMap::new();
        let mut unseeded = 0u32;
        for item in evidence
            .entries
            .iter()
            .filter(|i| i.value.kind == LedgerKind::Property && &i.value.id == id)
        {
            let entry = &item.value;
            if !item.validity.counts() {
                total
                    .stale
                    .push(format!("{}: {}", describe(entry), item.validity.describe()));
            } else if entry.result != GateResult::Pass {
                total.failed.push(describe(entry));
            } else if spec
                .reach
                .iter()
                .any(|(key, need)| entry.reach.get(key).copied().unwrap_or(0) < *need)
            {
                total.unreached.push(describe(entry));
            } else {
                total.counted += 1;
                let cases = entry.cases.unwrap_or(0);
                match &entry.seed {
                    Some(seed) => {
                        let longest = seeds.entry(seed.clone()).or_insert(0);
                        *longest = (*longest).max(cases);
                    }
                    None => {
                        unseeded += 1;
                        total.cases += cases;
                    }
                }
            }
        }
        total.cases += seeds.values().sum::<u64>();
        total.seeds = seeds.into_keys().collect();
        total.runs = u32::try_from(total.seeds.len())
            .unwrap_or(u32::MAX)
            .saturating_add(unseeded);
        properties.push(total);
    }
    let mut targets = Vec::new();
    for id in criteria.targets.keys() {
        let mut total = TargetTotal {
            id: id.clone(),
            hours: 0.0,
            counted: 0,
            failed: Vec::new(),
            stale: Vec::new(),
        };
        for item in evidence
            .entries
            .iter()
            .filter(|i| i.value.kind == LedgerKind::Target && &i.value.id == id)
        {
            let entry = &item.value;
            if !item.validity.counts() {
                total
                    .stale
                    .push(format!("{}: {}", describe(entry), item.validity.describe()));
            } else if entry.result != GateResult::Pass {
                total.failed.push(describe(entry));
            } else {
                total.counted += 1;
                total.hours += entry.cpu_hours();
            }
        }
        targets.push(total);
    }
    let unlisted = evidence
        .entries
        .iter()
        .filter(|i| match i.value.kind {
            LedgerKind::Property => !criteria.properties.contains_key(&i.value.id),
            LedgerKind::Target => !criteria.targets.contains_key(&i.value.id),
        })
        .map(|i| describe(&i.value))
        .collect();
    Totals {
        properties,
        targets,
        unlisted,
    }
}

impl PropertyTotal {
    /// Whether the property has what the criteria's totals ask.
    pub fn meets(&self, cases: u64, runs: u32) -> bool {
        self.failed.is_empty() && self.cases >= cases && self.runs >= runs
    }
}

impl TargetTotal {
    /// Whether the target has the hours asked.
    pub fn meets(&self, hours: f64) -> bool {
        self.failed.is_empty() && self.hours + 1e-9 >= hours
    }
}

/// `1234567` as `1,234,567`.
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The text of `ledger show`.
pub fn render_show(criteria: &Criteria, totals: &Totals) -> String {
    let need = &criteria.totals;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "properties: each needs {} cases in {} independently seeded runs, none failed",
        group(need.property_cases),
        need.property_runs
    );
    for p in &totals.properties {
        let name = criteria
            .properties
            .get(&p.id)
            .map_or("", |s| s.name.as_str());
        let verdict = if p.meets(need.property_cases, need.property_runs) {
            "met"
        } else {
            "short"
        };
        let _ = writeln!(
            out,
            "  {:<4} {:<16} {:>12} cases {:>3} runs  {} failed  {} stale  {} reach-short  {verdict}",
            p.id,
            name,
            group(p.cases),
            p.runs,
            p.failed.len(),
            p.stale.len(),
            p.unreached.len()
        );
    }
    let _ = writeln!(
        out,
        "targets: each needs {} CPU hours, no crash, timeout or out-of-memory stop",
        need.target_cpu_hours
    );
    for t in &totals.targets {
        let verdict = if t.meets(need.target_cpu_hours) {
            "met"
        } else {
            "short"
        };
        let _ = writeln!(
            out,
            "  {:<20} {:>8.2} hours  {} failed  {} stale  {verdict}",
            t.id,
            t.hours,
            t.failed.len(),
            t.stale.len()
        );
    }
    let mut problems = String::new();
    for p in &totals.properties {
        for line in p
            .failed
            .iter()
            .map(|l| ("failed", l))
            .chain(
                p.unreached
                    .iter()
                    .map(|l| ("reach below the criteria's", l)),
            )
            .chain(p.stale.iter().map(|l| ("not counted", l)))
        {
            let _ = writeln!(problems, "  {}: {}: {}", p.id, line.0, line.1);
        }
    }
    for t in &totals.targets {
        for line in t
            .failed
            .iter()
            .map(|l| ("failed", l))
            .chain(t.stale.iter().map(|l| ("not counted", l)))
        {
            let _ = writeln!(problems, "  {}: {}: {}", t.id, line.0, line.1);
        }
    }
    for line in &totals.unlisted {
        let _ = writeln!(problems, "  not in the criteria: {line}");
    }
    if !problems.is_empty() {
        out.push_str("entries that do not count:\n");
        out.push_str(&problems);
    }
    out
}

/// The text of `ledger plan`: what each property and target still needs. `forks`, when given,
/// turns the hours of a target into seconds for that many forks.
pub fn render_plan(criteria: &Criteria, totals: &Totals, forks: Option<u32>) -> String {
    let need = &criteria.totals;
    let mut out = String::new();
    let mut open = 0usize;
    let _ = writeln!(
        out,
        "properties still to run (make l1-long L1_PROPERTY=P L1_CASES=N L1_SEEDS=...):"
    );
    for p in &totals.properties {
        let cases_left = need.property_cases.saturating_sub(p.cases);
        let runs_left = need.property_runs.saturating_sub(p.runs);
        if p.meets(need.property_cases, need.property_runs) {
            continue;
        }
        open += 1;
        let runs_to_do = if cases_left > 0 {
            runs_left.max(1)
        } else {
            runs_left
        };
        let per_run = if runs_to_do > 0 {
            cases_left.div_ceil(u64::from(runs_to_do))
        } else {
            0
        };
        let fixed = if p.failed.is_empty() {
            ""
        } else {
            "  (a counted run failed: it needs a fix and new runs)"
        };
        let used = if p.seeds.is_empty() {
            String::new()
        } else {
            format!("; new seeds only, not {}", p.seeds.join(", "))
        };
        let _ = writeln!(
            out,
            "  {:<4} {:>12} cases and {} seeded runs still needed: {runs_to_do} runs of {} cases{used}{fixed}",
            p.id,
            group(cases_left),
            runs_left,
            group(per_run)
        );
    }
    let _ = writeln!(
        out,
        "targets still to fuzz (make fuzz-long FUZZ_TARGETS=T FUZZ_SECONDS=S FUZZ_FORK=F; CPU hours = seconds x forks):"
    );
    for t in &totals.targets {
        if t.meets(need.target_cpu_hours) {
            continue;
        }
        open += 1;
        let left = (need.target_cpu_hours - t.hours).max(0.0);
        let seconds = forks.map(|f| {
            format!(
                ", {} s at {f} forks",
                (left * 3600.0 / f64::from(f.max(1))).ceil()
            )
        });
        let fixed = if t.failed.is_empty() {
            ""
        } else {
            "  (a counted run failed: it needs a fix and new runs)"
        };
        let _ = writeln!(
            out,
            "  {:<20} {left:>8.2} hours still needed{}{fixed}",
            t.id,
            seconds.unwrap_or_default()
        );
    }
    if open == 0 {
        out.push_str("nothing is left: every property and target has its totals\n");
    }
    out
}

/// Where the commands read the criteria and the ledger, and the tree they judge in.
struct Common {
    criteria: Criteria,
    sources: Sources,
}

fn common(flags: &mut Flags) -> Result<Common> {
    let criteria_path = flags
        .one("criteria")?
        .or_else(|| std::env::var("S1_CRITERIA").ok())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| Error::Usage("--criteria is required (or S1_CRITERIA)".to_owned()))?;
    let mut ledger = flags.many("ledger");
    if ledger.is_empty()
        && let Ok(dir) = std::env::var("LEDGER_DIR")
        && !dir.is_empty()
    {
        ledger.push(dir);
    }
    let sources = Sources {
        records: Vec::new(),
        ledger: ledger.into_iter().map(PathBuf::from).collect(),
        repo: PathBuf::from(flags.one("repo")?.unwrap_or_else(|| ".".to_owned())),
        head: flags.one("head")?.unwrap_or_else(|| "HEAD".to_owned()),
        allow_dirty: flags.switch("allow-dirty"),
    };
    Ok(Common {
        criteria: Criteria::read(Path::new(&criteria_path))?,
        sources,
    })
}

/// Run `gate ledger merge|show|plan`.
pub fn run(args: &[String]) -> Result<bool> {
    let Some((verb, rest)) = args.split_first() else {
        return Err(Error::Usage(
            "gate ledger needs merge, show or plan".to_owned(),
        ));
    };
    match verb.as_str() {
        "merge" => merge(rest),
        "show" => {
            let mut flags = Flags::parse(rest, &["allow-dirty", "strict"])?;
            let strict = flags.switch("strict");
            let common = common(&mut flags)?;
            flags.finish()?;
            let evidence = Evidence::load(&common.criteria, &common.sources)?;
            let totals = totals(&common.criteria, &evidence);
            print!("{}", render_show(&common.criteria, &totals));
            Ok(!strict || everything_met(&common.criteria, &totals))
        }
        "plan" => {
            let mut flags = Flags::parse(rest, &["allow-dirty", "strict"])?;
            let strict = flags.switch("strict");
            let forks: Option<u32> = flags.number("forks")?;
            let common = common(&mut flags)?;
            flags.finish()?;
            let evidence = Evidence::load(&common.criteria, &common.sources)?;
            let totals = totals(&common.criteria, &evidence);
            print!("{}", render_plan(&common.criteria, &totals, forks));
            Ok(!strict || everything_met(&common.criteria, &totals))
        }
        other => Err(Error::Usage(format!(
            "gate ledger {other}: expected merge, show or plan"
        ))),
    }
}

/// Whether every property and target has its totals.
pub fn everything_met(criteria: &Criteria, totals: &Totals) -> bool {
    let need = &criteria.totals;
    totals
        .properties
        .iter()
        .all(|p| p.meets(need.property_cases, need.property_runs))
        && totals
            .targets
            .iter()
            .all(|t| t.meets(need.target_cpu_hours))
}

/// `gate ledger merge --from DIR --machine NAME --ledger DIR`.
fn merge(args: &[String]) -> Result<bool> {
    let mut flags = Flags::parse(args, &[])?;
    let from = PathBuf::from(flags.required("from")?);
    let machine = flags.required("machine")?;
    let ledger = PathBuf::from(flags.required("ledger")?);
    flags.finish()?;
    if machine.trim().is_empty() {
        return Err(Error::Usage("--machine is empty".to_owned()));
    }
    let found = store::load_all::<LedgerEntry>(std::slice::from_ref(&from), LEDGER_FORMAT)?;
    if found.is_empty() {
        return Err(Error::Usage(format!(
            "no ledger entries under {} (a server's ledger directory, not its records)",
            from.display()
        )));
    }
    // The runs the ledger holds, whatever machine they are named for: a copy of one of them in
    // FROM (a server that was given the ledger) is the same run, not another.
    let mut known: BTreeSet<RunKey> =
        store::load_all::<LedgerEntry>(std::slice::from_ref(&ledger), LEDGER_FORMAT)?
            .iter()
            .map(|(_, entry)| RunKey::of(entry))
            .collect();
    let (mut added, mut same) = (0usize, 0usize);
    for (path, mut entry) in found {
        check_entry(&path, &entry)?;
        if !known.insert(RunKey::of(&entry)) {
            same += 1;
            continue;
        }
        entry.machine.clone_from(&machine);
        if store::put_entry(&entry, &ledger)?.1 {
            added += 1;
        } else {
            same += 1;
        }
    }
    println!(
        "ledger merge: {added} entries added from {}, {same} already there, as machine {machine}",
        from.display()
    );
    Ok(true)
}

/// What makes a run the same run, whichever machine name it carries.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RunKey {
    kind: bool,
    id: String,
    seed: Option<String>,
    cases: Option<u64>,
    seconds: Option<u64>,
    forks: Option<u32>,
    head: String,
    started: String,
    finished: String,
}

impl RunKey {
    fn of(entry: &LedgerEntry) -> RunKey {
        RunKey {
            kind: entry.kind == LedgerKind::Property,
            id: entry.id.clone(),
            seed: entry.seed.clone(),
            cases: entry.cases,
            seconds: entry.seconds,
            forks: entry.forks,
            head: entry.head.clone(),
            started: entry.started.clone(),
            finished: entry.finished.clone(),
        }
    }
}

/// An entry the ledger takes: a property run has cases and a seed, a target run seconds.
fn check_entry(path: &Path, entry: &LedgerEntry) -> Result<()> {
    let bad = |message: &str| Error::Corpus {
        path: path.to_path_buf(),
        message: message.to_owned(),
    };
    if entry.id.is_empty() {
        return Err(bad("an entry without an id"));
    }
    match entry.kind {
        LedgerKind::Property if entry.cases.is_none() => Err(bad("a property entry without cases")),
        LedgerKind::Target if entry.seconds.is_none() || entry.forks.is_none() => {
            Err(bad("a target entry without seconds and forks"))
        }
        _ => Ok(()),
    }
}

/// Entries of `items` that count, for the callers that list them.
pub fn counted(items: &[Item<LedgerEntry>]) -> impl Iterator<Item = &Item<LedgerEntry>> {
    items.iter().filter(|i| i.validity.counts())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_grouped_and_properties_ordered() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(1_234_567), "1,234,567");
        let mut ids = vec!["P10", "P2", "P1"];
        ids.sort_by_key(|id| property_order(id));
        assert_eq!(ids, ["P1", "P2", "P10"]);
    }
}
