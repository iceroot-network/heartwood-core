//! What a gate command printed, read back for its record: the counts of a comparison report (or
//! of `cargo test`'s result lines) and the documented differences the report lists.
//!
//! A comparison prints, per report, `N compared: X passed, Y failed`, `D documented differences
//! ...` with one `    n  <reason>` line per difference, and `S skipped ...`. Each reason text is
//! mapped back to its code name ([`super::names`]); a line that is not a known reason counts
//! nothing.

use std::collections::BTreeMap;

use super::names;
use super::record::Counts;

/// What one command's output gave: its counts, when it printed any, and its hits by code name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// The counts summed over every report or result line, if there was one.
    pub counts: Option<Counts>,
    /// The documented differences met, by code name.
    pub hits: BTreeMap<String, u64>,
}

/// Read the output of a gate command.
pub fn parse(text: &str) -> Parsed {
    let mut counts = Counts::default();
    let mut seen = false;
    let mut hits: BTreeMap<String, u64> = BTreeMap::new();
    for line in text.lines() {
        if let Some((compared, passed, failed)) = compared_line(line) {
            seen = true;
            counts.compared += compared;
            counts.passed += passed;
            counts.failed += failed;
        } else if let Some(n) = leading_count(line, " documented differences") {
            seen = true;
            counts.documented += n;
        } else if let Some(n) = leading_count(line, " skipped") {
            seen = true;
            counts.skipped += n;
        } else if let Some((passed, failed, ignored)) = cargo_result(line) {
            seen = true;
            counts.compared += passed + failed;
            counts.passed += passed;
            counts.failed += failed;
            counts.skipped += ignored;
        } else if let Some((n, reason)) = reason_line(line)
            && let Some(name) = names::name_of_reason(reason)
        {
            *hits.entry(name.to_owned()).or_insert(0) += n;
        }
    }
    Parsed {
        counts: seen.then_some(counts),
        hits,
    }
}

/// The number that starts `line` (after spaces) when what follows it starts with `rest`.
fn leading_count(line: &str, rest: &str) -> Option<u64> {
    let trimmed = line.trim_start();
    let end = trimmed.find(|c: char| !c.is_ascii_digit())?;
    if end == 0 {
        return None;
    }
    trimmed[end..]
        .starts_with(rest)
        .then(|| trimmed[..end].parse().ok())
        .flatten()
}

/// `N compared: X passed, Y failed`.
fn compared_line(line: &str) -> Option<(u64, u64, u64)> {
    let compared = leading_count(line, " compared: ")?;
    let (_, tail) = line.split_once(" compared: ")?;
    let (passed, tail) = tail.split_once(" passed, ")?;
    let failed = tail.strip_suffix(" failed")?;
    Some((compared, passed.parse().ok()?, failed.trim().parse().ok()?))
}

/// `test result: ok. 12 passed; 0 failed; 1 ignored; ...`, as `cargo test` prints it.
fn cargo_result(line: &str) -> Option<(u64, u64, u64)> {
    let tail = line.trim_start().strip_prefix("test result: ")?;
    let (_, tail) = tail.split_once(". ")?;
    let field = |name: &str| -> Option<u64> {
        let (before, _) = tail.split_once(&format!(" {name}"))?;
        before.rsplit(' ').next()?.parse().ok()
    };
    Some((field("passed")?, field("failed")?, field("ignored")?))
}

/// A documented-difference line: a count right-aligned in five columns, two spaces and the
/// reason.
fn reason_line(line: &str) -> Option<(u64, &str)> {
    let trimmed = line.trim_start();
    let end = trimmed.find(|c: char| !c.is_ascii_digit())?;
    if end == 0 {
        return None;
    }
    let reason = trimmed[end..].strip_prefix("  ")?;
    Some((trimmed[..end].parse().ok()?, reason.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subject::{ApiDifference, Documented};

    fn report(reasons: &[(u64, Documented)]) -> String {
        let mut text = String::from("L1: 300 records in 3 files\n  oracle: local\n");
        text += "  290 compared: 288 passed, 2 failed\n";
        text += "  10 documented differences (Heartwood differs on purpose):\n";
        for (n, difference) in reasons {
            text += &format!("    {n:>5}  {}\n", difference.reason());
        }
        text += "  4 skipped (not modelled by the subject):\n";
        text += "      4  op  a reason\n";
        text
    }

    #[test]
    fn a_report_gives_its_counts_and_hits() {
        let text = report(&[
            (7, Documented::ReplayIndexOrder),
            (3, Documented::Api(ApiDifference::TieOrder)),
        ]);
        let parsed = parse(&text);
        assert_eq!(
            parsed.counts,
            Some(Counts {
                compared: 290,
                passed: 288,
                failed: 2,
                documented: 10,
                skipped: 4
            })
        );
        assert_eq!(parsed.hits["Documented::ReplayIndexOrder"], 7);
        assert_eq!(parsed.hits["ApiDifference::TieOrder"], 3);
        assert_eq!(parsed.hits.len(), 2);
    }

    #[test]
    fn several_reports_add_up() {
        let text = report(&[(1, Documented::Locale)]) + &report(&[(2, Documented::Locale)]);
        let parsed = parse(&text);
        assert_eq!(parsed.counts.map(|c| c.compared), Some(580));
        assert_eq!(parsed.hits["Documented::Locale"], 3);
    }

    #[test]
    fn cargo_test_lines_count_and_unknown_lines_do_not() {
        let text = "running 3 tests\ntest result: ok. 12 passed; 1 failed; 2 ignored; 0 measured; 0 filtered out\n   9  something else\n";
        let parsed = parse(text);
        assert_eq!(
            parsed.counts,
            Some(Counts {
                compared: 13,
                passed: 12,
                failed: 1,
                documented: 0,
                skipped: 2
            })
        );
        assert!(parsed.hits.is_empty());
        assert_eq!(parse("nothing to count\n"), Parsed::default());
    }
}
