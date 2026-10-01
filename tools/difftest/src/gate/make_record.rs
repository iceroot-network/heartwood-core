//! `gate record`: write one gate record, and the ledger entry of a long run.
//!
//! ```text
//! difftest gate record --target T --command C --exit N --started S --finished F --out DIR
//!     [--variables V] [--image I] [--criteria FILE] [--tested PATH]... [--output FILE]
//!     [--tooling DIR]... [--head REV --dirty-before 0|1]
//!     [--ledger "kind=property id=P13 cases=1000000 seed=7" --ledger-dir DIR]
//!     [--repo DIR] [--machine NAME] [--allow-dirty]
//! ```
//!
//! The head and whether the tree is dirty come from the tree in `--repo`; `--head` and
//! `--dirty-before` give them as they were when the command started, and a tree that changed
//! while it ran (another head, or changes beside it) makes the record dirty. The tested paths are
//! the criteria file's narrowing of the command when it has one, else the `--tested` paths.
//! `--output` names a file holding what the command printed, which gives the record its counts,
//! hits and scenario verdicts. `--tooling` names a run directory (or a directory of run
//! directories) whose `tooling.sha256` lists the devnet tooling the run was made with.
//!
//! A long run's ledger entry is written only for a run whose cases all count: a property run that
//! was not strict, or that skipped cases, gets none. A run that a signal ended before it judged
//! its cases (no failed case in its counts) is entered as stopped: it counts for none of its
//! cases or time, and it is not a failed run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::criteria::Criteria;
use super::flags::Flags;
use super::git::Repo;
use super::ledger::{LEDGER_FORMAT, LedgerEntry, LedgerKind};
use super::output;
use super::record::{Counts, GateRecord, GateResult, RECORD_FORMAT};
use super::store;
use super::time;
use crate::errors::{Error, Result};

/// The reach a property's runs check, as the `l1` command's flag and the ledger's key.
const REACH: [(&str, &str, &str); 3] = [
    ("P13", "--min-reach", "minReach"),
    ("P19", "--decision-reach", "decisionReach"),
    ("P20", "--served-reach", "servedReach"),
];

/// Run `gate record`; whether it wrote what it was asked to.
pub fn run(args: &[String]) -> Result<bool> {
    let mut flags = Flags::parse(args, &["allow-dirty"])?;
    let head_before = flags.one("head")?;
    let dirty_before = flags.one("dirty-before")?;
    let tooling_dirs = flags.many("tooling");
    let target = flags.required("target")?;
    let command = flags.required("command")?;
    let exit: i32 = flags
        .number("exit")?
        .ok_or_else(|| Error::Usage("--exit is required".to_owned()))?;
    let started = flags.required("started")?;
    let finished = flags.required("finished")?;
    let out = PathBuf::from(flags.required("out")?);
    let variables = flags.one("variables")?.unwrap_or_default();
    let image = flags.one("image")?.filter(|i| !i.is_empty());
    let criteria = flags.one("criteria")?.filter(|c| !c.is_empty());
    let tested = flags.many("tested");
    let output_file = flags.one("output")?;
    let ledger = flags.one("ledger")?;
    let ledger_dir = flags.one("ledger-dir")?;
    let repo_dir = flags.one("repo")?.unwrap_or_else(|| ".".to_owned());
    let machine = machine_name(flags.one("machine")?);
    let allow_dirty = flags.switch("allow-dirty");
    flags.finish()?;

    for stamp in [&started, &finished] {
        if !time::is_utc_stamp(stamp) {
            return Err(Error::Usage(format!(
                "{stamp:?} is not a UTC time stamp such as 2026-10-01T10:00:00Z"
            )));
        }
    }
    let repo = Repo::open(Path::new(&repo_dir));
    let head_now = repo.resolve("HEAD")?;
    let head = match &head_before {
        Some(rev) => repo.resolve(rev)?,
        None => head_now.clone(),
    };
    let changed_meanwhile = match dirty_before.as_deref() {
        None | Some("0") => false,
        Some("1") => true,
        Some(other) => {
            return Err(Error::Usage(format!(
                "--dirty-before is 0 or 1, not {other:?}"
            )));
        }
    };
    // A tree that changed while the command ran is no tree the command ran on alone.
    let dirty = changed_meanwhile || head != head_now || repo.dirty()?;

    let narrowed = match &criteria {
        Some(path) => Criteria::read(Path::new(path))?.narrowed_paths(&target, &command),
        None => None,
    };
    let tested_paths = tested_paths(narrowed.unwrap_or(tested))?;

    let parsed = match &output_file {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| Error::io(Path::new(path), e))?;
            output::parse(&String::from_utf8_lossy(&bytes))
        }
        None => output::Parsed::default(),
    };
    let tooling = tooling_of(&tooling_dirs)?;

    let record = GateRecord {
        format: RECORD_FORMAT.to_owned(),
        target,
        command,
        variables,
        head: head.clone(),
        dirty,
        tested_paths,
        image: image.clone(),
        machine: machine.clone(),
        counts: parsed.counts,
        exit_status: exit,
        result: if exit == 0 {
            GateResult::Pass
        } else {
            GateResult::Fail
        },
        hits: parsed.hits,
        scenarios: parsed.scenarios,
        tooling,
        started,
        finished,
    };
    let path = store::write_record(&record, &out)?;
    println!("gate record: {}", path.display());

    match (ledger, ledger_dir) {
        (None, _) => Ok(true),
        (Some(_), None) => Err(Error::Usage("--ledger needs --ledger-dir".to_owned())),
        (Some(spec), Some(dir)) => {
            if dirty && !allow_dirty {
                eprintln!(
                    "gate record: the tree has changes beside its head, so the ledger entry is not written (--allow-dirty writes it)"
                );
                return Ok(false);
            }
            let written_name = path
                .file_name()
                .map_or_else(|| record.file_name(), |n| n.to_string_lossy().into_owned());
            let source = format!("gate record {written_name}");
            let entry = entry_from(&spec, &record, &source)?;
            if let Some(why) = not_counted(&entry, &record) {
                println!("gate record: no ledger entry: {why}");
                return Ok(true);
            }
            let written = store::write_entry(&entry, Path::new(&dir))?;
            println!("gate record: ledger entry {}", written.display());
            Ok(true)
        }
    }
}

/// What `gate coverage` and `gate register` say about their own run.
pub struct OwnRecord<'a> {
    /// The record's target (`coverage`, `register`).
    pub target: &'a str,
    /// The command as it ran.
    pub command: String,
    /// Whether every check passed.
    pub pass: bool,
    /// When it started.
    pub started: String,
    /// Its counts.
    pub counts: Counts,
    /// The documented differences its records showed, by code name.
    pub hits: BTreeMap<String, u64>,
}

/// Write the record of a command that judged others' records, into `out`; its path. It tests the
/// criteria's narrowing for its target, which the criteria name as the paths the records it read
/// test.
pub fn write_own(
    out: &Path,
    repo: &Path,
    criteria: &Criteria,
    own: OwnRecord<'_>,
) -> Result<PathBuf> {
    let repo = Repo::open(repo);
    let record = GateRecord {
        format: RECORD_FORMAT.to_owned(),
        target: own.target.to_owned(),
        command: own.command,
        variables: String::new(),
        head: repo.resolve("HEAD")?,
        dirty: repo.dirty()?,
        tested_paths: tested_paths(
            criteria
                .narrowing
                .get(own.target)
                .cloned()
                .unwrap_or_default(),
        )?,
        image: None,
        machine: machine_name(None),
        counts: Some(own.counts),
        exit_status: i32::from(!own.pass),
        result: if own.pass {
            GateResult::Pass
        } else {
            GateResult::Fail
        },
        hits: own.hits,
        scenarios: BTreeMap::new(),
        tooling: BTreeMap::new(),
        started: own.started,
        finished: time::now_utc(),
    };
    store::write_record(&record, out)
}

/// The machine's name: the flag, `GATE_MACHINE`, the host name.
pub fn machine_name(flag: Option<String>) -> String {
    let named = flag
        .or_else(|| std::env::var("GATE_MACHINE").ok())
        .filter(|n| !n.trim().is_empty());
    if let Some(name) = named {
        return name.trim().to_owned();
    }
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty());
    host.or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The paths as the records keep them: relative to the tree's root, without `./` or a trailing
/// slash, sorted, each once.
fn tested_paths(paths: Vec<String>) -> Result<Vec<String>> {
    let mut clean = Vec::new();
    for path in paths {
        let trimmed = path.trim_start_matches("./").trim_end_matches('/');
        if trimmed.is_empty()
            || trimmed.starts_with('/')
            || trimmed.split('/').any(|part| part == "..")
        {
            return Err(Error::Usage(format!(
                "the tested path {path:?} is not a path inside the tree"
            )));
        }
        clean.push(trimmed.to_owned());
    }
    clean.sort();
    clean.dedup();
    Ok(clean)
}

/// The file each run directory keeps of the devnet tooling it was made with.
const TOOLING_FILE: &str = "tooling.sha256";

/// The devnet tooling the runs in `dirs` were made with: each `tooling.sha256` of a directory, or
/// of the directories right under it, read as `<sha256>  <path>` lines, the paths relative to the
/// tooling's root (an absolute path names a build of the tree or the toolchain, not the tooling).
fn tooling_of(dirs: &[String]) -> Result<BTreeMap<String, Vec<String>>> {
    let mut files = Vec::new();
    for dir in dirs.iter().map(Path::new) {
        let own = dir.join(TOOLING_FILE);
        if own.is_file() {
            files.push(own);
            continue;
        }
        let listing = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
        let mut found: Vec<PathBuf> = listing
            .filter_map(|e| e.ok().map(|e| e.path().join(TOOLING_FILE)))
            .filter(|p| p.is_file())
            .collect();
        found.sort();
        files.extend(found);
    }
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|e| Error::io(&file, e))?;
        for line in text.lines() {
            let Some((sum, path)) = line.split_once("  ") else {
                continue;
            };
            let sum_ok = sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit());
            if !sum_ok || path.starts_with('/') {
                continue;
            }
            let sums = out.entry(path.to_owned()).or_default();
            if !sums.iter().any(|s| s == sum) {
                sums.push(sum.to_owned());
                sums.sort();
            }
        }
    }
    Ok(out)
}

/// Why a long run's entry would count for nothing, when it would not: a passing property run that
/// was not strict, or that skipped cases (its cases were not all compared).
fn not_counted(entry: &LedgerEntry, record: &GateRecord) -> Option<String> {
    // A failed run is entered whatever its strictness: it fails its property.
    if entry.kind != LedgerKind::Property || entry.stopped || entry.result != GateResult::Pass {
        return None;
    }
    let strict = record.command.split_whitespace().any(|w| w == "--strict");
    let skipped = record.counts.map_or(0, |c| c.skipped);
    if !strict {
        Some("the run was not strict, so its cases do not count".to_owned())
    } else if skipped > 0 {
        Some(format!(
            "the run skipped {skipped} cases, so its cases do not count"
        ))
    } else {
        None
    }
}

/// Whether a run's record says a signal ended it before it judged its cases: an exit status of a
/// signal (above 128) and no failed case in its counts.
pub(super) fn stopped(record: &GateRecord) -> bool {
    record.exit_status > 128 && record.counts.is_none_or(|c| c.failed == 0)
}

/// The ledger entry of a long run, from the `k=v` words of `--ledger` and its record.
fn entry_from(spec: &str, record: &GateRecord, source: &str) -> Result<LedgerEntry> {
    let mut words: BTreeMap<&str, &str> = BTreeMap::new();
    for word in spec.split_whitespace() {
        let (key, value) = word
            .split_once('=')
            .ok_or_else(|| Error::Usage(format!("--ledger: {word:?} is not key=value")))?;
        if !["kind", "id", "cases", "seconds", "forks", "seed"].contains(&key) {
            return Err(Error::Usage(format!("--ledger: unknown key {key:?}")));
        }
        if words.insert(key, value).is_some() {
            return Err(Error::Usage(format!("--ledger: {key} is given twice")));
        }
    }
    let need = |key: &str| -> Result<&str> {
        words
            .get(key)
            .copied()
            .ok_or_else(|| Error::Usage(format!("--ledger: {key}= is missing")))
    };
    let number = |key: &str| -> Result<u64> {
        need(key)?
            .parse()
            .map_err(|_| Error::Usage(format!("--ledger: {key} is not a number")))
    };
    let kind = match need("kind")? {
        "property" => LedgerKind::Property,
        "target" => LedgerKind::Target,
        other => {
            return Err(Error::Usage(format!(
                "--ledger: kind {other:?} is not property or target"
            )));
        }
    };
    let id = need("id")?.to_owned();
    let mut entry = LedgerEntry {
        format: LEDGER_FORMAT.to_owned(),
        kind,
        id: id.clone(),
        cases: None,
        seconds: None,
        forks: None,
        seed: words.get("seed").map(|s| (*s).to_owned()),
        head: record.head.clone(),
        image: record.image.clone(),
        machine: record.machine.clone(),
        started: record.started.clone(),
        finished: record.finished.clone(),
        result: record.result,
        reach: BTreeMap::new(),
        stopped: stopped(record),
        source: source.to_owned(),
    };
    match kind {
        LedgerKind::Property => {
            entry.cases = Some(number("cases")?);
            if entry.seed.is_none() {
                return Err(Error::Usage(
                    "--ledger: a property run needs seed=".to_owned(),
                ));
            }
            entry.reach = reach_of(&id, &record.command);
        }
        LedgerKind::Target => {
            entry.seconds = Some(number("seconds")?);
            entry.forks = Some(
                u32::try_from(number("forks")?)
                    .map_err(|_| Error::Usage("--ledger: forks is too large".to_owned()))?,
            );
        }
    }
    Ok(entry)
}

/// The reach a passing run of `property` checked, from the flags of its command.
fn reach_of(property: &str, command: &str) -> BTreeMap<String, u64> {
    let mut reach = BTreeMap::new();
    let words: Vec<&str> = command.split_whitespace().collect();
    for (name, flag, key) in REACH {
        if name != property {
            continue;
        }
        let value = words
            .iter()
            .position(|w| *w == flag)
            .and_then(|at| words.get(at + 1))
            .and_then(|v| v.parse::<u64>().ok());
        if let Some(value) = value {
            reach.insert(key.to_owned(), value);
        }
    }
    reach
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(command: &str) -> GateRecord {
        GateRecord {
            format: RECORD_FORMAT.to_owned(),
            target: "l1".to_owned(),
            command: command.to_owned(),
            variables: String::new(),
            head: "a".repeat(40),
            dirty: false,
            tested_paths: Vec::new(),
            image: None,
            machine: "m".to_owned(),
            counts: None,
            exit_status: 0,
            result: GateResult::Pass,
            hits: BTreeMap::new(),
            scenarios: BTreeMap::new(),
            tooling: BTreeMap::new(),
            started: "2026-10-01T10:00:00Z".to_owned(),
            finished: "2026-10-01T10:05:00Z".to_owned(),
        }
    }

    #[test]
    fn a_property_entry_keeps_its_reach() {
        let r = record(
            "difftest run l1 --property P13 --min-reach 20 --decision-reach 0 --served-reach 0 --cases 5",
        );
        let entry = entry_from("kind=property id=P13 cases=5 seed=9", &r, "x").unwrap();
        assert_eq!(entry.cases, Some(5));
        assert_eq!(entry.reach, BTreeMap::from([("minReach".to_owned(), 20)]));
        let entry = entry_from("kind=property id=P1 cases=5 seed=9", &r, "x").unwrap();
        assert!(entry.reach.is_empty());
    }

    #[test]
    fn a_target_entry_needs_seconds_and_forks() {
        let r = record("cargo +nightly fuzz run t");
        let entry = entry_from("kind=target id=t seconds=600 forks=8", &r, "x").unwrap();
        assert!((entry.cpu_hours() - 600.0 * 8.0 / 3600.0).abs() < 1e-9);
        assert!(entry_from("kind=target id=t seconds=600", &r, "x").is_err());
        assert!(entry_from("kind=property id=P1 cases=1", &r, "x").is_err());
        assert!(entry_from("kind=thing id=P1", &r, "x").is_err());
        assert!(entry_from("kind=target id=t seconds=1 forks=1 extra=1", &r, "x").is_err());
    }

    #[test]
    fn a_run_a_signal_ended_is_stopped_and_a_lenient_or_skipping_run_counts_for_nothing() {
        let mut killed = record("difftest run l1 --property P13 --cases 5 --strict");
        killed.exit_status = 137;
        killed.result = GateResult::Fail;
        let entry = entry_from("kind=property id=P13 cases=5 seed=9", &killed, "x").unwrap();
        assert!(entry.stopped);
        assert_eq!(not_counted(&entry, &killed), None);
        // A failed case is a failed run, whatever ended it.
        killed.counts = Some(Counts {
            compared: 3,
            failed: 1,
            ..Counts::default()
        });
        assert!(
            !entry_from("kind=property id=P13 cases=5 seed=9", &killed, "x")
                .unwrap()
                .stopped
        );
        let lenient = record("difftest run l1 --property P13 --cases 5");
        let entry = entry_from("kind=property id=P13 cases=5 seed=9", &lenient, "x").unwrap();
        assert!(not_counted(&entry, &lenient).is_some());
        let mut skipping = record("difftest run l1 --property P13 --cases 5 --strict");
        skipping.counts = Some(Counts {
            compared: 5,
            passed: 5,
            skipped: 2,
            ..Counts::default()
        });
        let entry = entry_from("kind=property id=P13 cases=5 seed=9", &skipping, "x").unwrap();
        assert!(
            not_counted(&entry, &skipping)
                .unwrap()
                .contains("skipped 2")
        );
        let strict = record("difftest run l1 --property P13 --cases 5 --strict");
        let entry = entry_from("kind=property id=P13 cases=5 seed=9", &strict, "x").unwrap();
        assert_eq!(not_counted(&entry, &strict), None);
    }

    #[test]
    fn tooling_is_read_from_a_run_or_the_runs_under_a_directory() {
        let dir = std::env::temp_dir().join(format!("difftest-tooling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("suite-a"), dir.join("suite-b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let line = |c: char, path: &str| format!("{}  {path}\n", c.to_string().repeat(64));
        std::fs::write(
            a.join(TOOLING_FILE),
            line('a', "scripts/run.sh") + &line('c', "/abs/target/release/heartwood"),
        )
        .unwrap();
        std::fs::write(b.join(TOOLING_FILE), line('b', "scripts/run.sh")).unwrap();
        let both = tooling_of(&[dir.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(
            both,
            BTreeMap::from([(
                "scripts/run.sh".to_owned(),
                vec!["a".repeat(64), "b".repeat(64)]
            )])
        );
        let one = tooling_of(&[a.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(one["scripts/run.sh"], vec!["a".repeat(64)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tested_paths_are_cleaned_and_kept_inside_the_tree() {
        assert_eq!(
            tested_paths(vec!["./b/".into(), "a".into(), "a".into()]).unwrap(),
            ["a", "b"]
        );
        assert!(tested_paths(vec!["../x".into()]).is_err());
        assert!(tested_paths(vec!["/x".into()]).is_err());
    }
}
