//! `gate coverage`: each of the coverage plan's 24 rows names its records at each level it lists,
//! all passing.
//!
//! ```text
//! difftest gate coverage --criteria FILE --records DIR... [--ledger DIR...] [--out DIR] [--strict]
//!     [--repo DIR] [--head REV] [--allow-dirty]
//! ```
//!
//! With `--out` the command writes its own record (target `coverage`): a pass when every level
//! passes, a failure when one fails, and none while levels only lack their evidence.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::flags::Flags;
use super::make_record::{OwnRecord, write_own};
use super::needs::{Judge, Status};
use super::record::Counts;
use super::s1::{setup, status_word};
use super::store::Evidence;
use super::time;
use crate::errors::Result;

/// Run `gate coverage`.
pub fn run(args: &[String]) -> Result<bool> {
    let started = time::now_utc();
    let mut flags = Flags::parse(args, &["strict", "allow-dirty"])?;
    let strict = flags.switch("strict");
    let out = flags.one("out")?;
    let setup = setup(&mut flags)?;
    flags.finish()?;
    let evidence = Evidence::load(&setup.criteria, &setup.sources)?;
    let judge = Judge::new(&setup.criteria, &evidence);

    let mut text = String::new();
    let (mut levels, mut passed, mut missing, mut failed) = (0u64, 0u64, 0u64, 0u64);
    for row in &setup.criteria.coverage {
        let _ = writeln!(text, "row {:>2}  {}", row.row, row.item);
        for (level, needs) in &row.levels {
            let verdicts = judge.needs(needs);
            let worst = verdicts
                .iter()
                .map(|v| v.status)
                .min()
                .unwrap_or(Status::Missing);
            levels += 1;
            match worst {
                Status::Pass => passed += 1,
                Status::Fail => failed += 1,
                Status::Missing | Status::Pending => missing += 1,
            }
            let detail = verdicts
                .iter()
                .find(|v| v.status == worst)
                .map_or("no evidence named", |v| v.detail.as_str());
            let _ = writeln!(text, "    {level:<5} {:<8}{detail}", status_word(worst));
        }
        if row.levels.is_empty() {
            levels += 1;
            missing += 1;
            let _ = writeln!(text, "    (no level is named for this row)");
        }
    }
    print!("{text}");
    println!(
        "{} rows, {levels} levels: {passed} pass, {missing} missing, {failed} fail",
        setup.criteria.coverage.len()
    );
    let all = passed == levels && levels > 0;
    // A record is a pass or a failure: while levels only lack their evidence and none fails, no
    // record is written, since a failed one would block every row that reads the coverage.
    if out.is_some() && failed == 0 && missing > 0 {
        println!("gate coverage: no record written: {missing} levels have no evidence yet");
    } else if let Some(out) = out {
        let path = write_own(
            Path::new(&out),
            &setup.sources.repo,
            &setup.criteria,
            OwnRecord {
                target: "coverage",
                command: format!("difftest gate coverage {}", args.join(" ")),
                pass: all,
                started,
                counts: Counts {
                    compared: levels,
                    passed,
                    failed: failed + missing,
                    documented: 0,
                    skipped: 0,
                },
                hits: BTreeMap::new(),
            },
        )?;
        println!("gate coverage: {}", path.display());
    }
    Ok(if strict { all } else { failed == 0 })
}
