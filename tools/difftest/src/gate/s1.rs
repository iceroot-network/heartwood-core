//! `gate s1`: the baseline record's table, a verdict per row from the gate records, the ledger and the
//! criteria file.
//!
//! ```text
//! difftest gate s1 --criteria FILE --records DIR... [--ledger DIR...] [--part TAG] [--strict]
//!     [--final] [--repo DIR] [--head REV] [--allow-dirty] [--verbose]
//! ```
//!
//! A row passes when every need is met; it fails when a need is missing, failed or stale; it is
//! pending when nothing failed and what is missing is something the row waits on, or only a
//! person can show. With `--final` (the last gate, when nothing is left to wait on) missing
//! evidence fails its row whatever the row waits on. Without `--strict` the command exits 0 unless
//! a row fails; with it, only when every row passes or waits only on a person (a sign-off or a
//! reading the table cannot give), which the table counts apart.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::criteria::Criteria;
use super::flags::Flags;
use super::needs::{Judge, RowStatus, Status, Verdict};
use super::store::{Evidence, Sources};
use crate::errors::{Error, Result};

/// The rows' verdicts and the text of the table.
#[derive(Debug)]
pub struct Table {
    /// The text.
    pub text: String,
    /// Rows that pass.
    pub pass: usize,
    /// Rows that are pending.
    pub pending: usize,
    /// Of the pending rows, those that wait only on a person.
    pub for_a_person: usize,
    /// Rows that fail.
    pub fail: usize,
}

/// The name of a status in the table.
pub fn status_word(status: Status) -> &'static str {
    match status {
        Status::Pass => "ok",
        Status::Pending => "pending",
        Status::Missing => "missing",
        Status::Fail => "FAIL",
    }
}

/// Add the lines of `verdicts` under a row; a passing need only with `verbose`.
pub fn push_verdicts(out: &mut String, verdicts: &[Verdict], verbose: bool) {
    let mut passed = 0usize;
    for verdict in verdicts {
        if verdict.status == Status::Pass && !verbose {
            passed += 1;
            continue;
        }
        let _ = writeln!(
            out,
            "      {:<8}{}",
            status_word(verdict.status),
            verdict.detail
        );
    }
    if passed > 0 {
        let _ = writeln!(
            out,
            "      ok      {passed} more need{} met",
            if passed == 1 { "" } else { "s" }
        );
    }
}

/// Judge the rows of `criteria` selected by `part` (all when none) against `evidence`; at the
/// final gate when `last`.
pub fn table(
    criteria: &Criteria,
    evidence: &Evidence,
    part: Option<&str>,
    verbose: bool,
    last: bool,
) -> Table {
    let judge = Judge::new(criteria, evidence).at_the_end(last);
    let mut text = String::new();
    let (mut pass, mut pending, mut for_a_person, mut fail) = (0, 0, 0, 0);
    for row in &criteria.rows {
        if part.is_some_and(|p| !row.parts.iter().any(|t| t == p)) {
            continue;
        }
        let verdict = judge.row(&row.id);
        let word = match verdict.status {
            RowStatus::Pass => {
                pass += 1;
                "pass"
            }
            RowStatus::Pending => {
                pending += 1;
                if judge.waits_only_on_a_person(&row.id) {
                    for_a_person += 1;
                }
                "pending"
            }
            RowStatus::Fail => {
                fail += 1;
                "FAIL"
            }
        };
        let _ = writeln!(
            text,
            "{:<9}{:<8} {}: {}",
            row.id, word, row.criterion, row.requires
        );
        if let Some(waits) = &row.waits
            && verdict.status != RowStatus::Pass
        {
            let _ = writeln!(text, "      waits on {waits}");
        }
        push_verdicts(&mut text, &verdict.needs, verbose);
    }
    Table {
        text,
        pass,
        pending,
        for_a_person,
        fail,
    }
}

/// Where the evidence commands read: the flags they share.
pub struct Setup {
    /// The criteria file.
    pub criteria: Criteria,
    /// Where the evidence is.
    pub sources: Sources,
}

/// Read the flags `s1`, `coverage` and `register` share.
pub fn setup(flags: &mut Flags) -> Result<Setup> {
    let criteria_path = flags
        .one("criteria")?
        .or_else(|| std::env::var("S1_CRITERIA").ok())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| Error::Usage("--criteria is required (or S1_CRITERIA)".to_owned()))?;
    let sources = Sources {
        records: flags
            .many("records")
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        ledger: flags
            .many("ledger")
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        repo: PathBuf::from(flags.one("repo")?.unwrap_or_else(|| ".".to_owned())),
        head: flags.one("head")?.unwrap_or_else(|| "HEAD".to_owned()),
        allow_dirty: flags.switch("allow-dirty"),
    };
    Ok(Setup {
        criteria: Criteria::read(Path::new(&criteria_path))?,
        sources,
    })
}

/// Run `gate s1`.
pub fn run(args: &[String]) -> Result<bool> {
    let mut flags = Flags::parse(args, &["strict", "allow-dirty", "verbose", "final"])?;
    let strict = flags.switch("strict");
    let verbose = flags.switch("verbose");
    let last = flags.switch("final");
    let part = flags.one("part")?;
    let setup = setup(&mut flags)?;
    flags.finish()?;
    let evidence = Evidence::load(&setup.criteria, &setup.sources)?;
    if let Some(part) = &part
        && !setup.criteria.rows.iter().any(|r| r.parts.contains(part))
    {
        return Err(Error::Usage(format!("no row is tagged {part:?}")));
    }
    let table = table(&setup.criteria, &evidence, part.as_deref(), verbose, last);
    println!(
        "S1 gate at {} ({} records, {} ledger entries)",
        evidence.rule.head(),
        evidence.records.len(),
        evidence.entries.len()
    );
    print!("{}", table.text);
    println!(
        "{} pass, {} pending ({} waiting only on a person), {} fail",
        table.pass, table.pending, table.for_a_person, table.fail
    );
    Ok(table.fail == 0 && (!strict || table.pending == table.for_a_person))
}
