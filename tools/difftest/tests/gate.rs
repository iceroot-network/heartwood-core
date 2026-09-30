//! The gate commands against real git trees: a record's tested paths and counts, the path rule
//! (a documentation-only commit keeps a record valid, a change to a tested file does not, a
//! dirty tree does not count), the stage table's verdicts, the ledger's sums across machines,
//! the coverage rows and the register's checks. Every test builds its own tree, records, ledger
//! and criteria file in a scratch directory and runs the `difftest` binary.

use std::path::{Path, PathBuf};
use std::process::Command;

use heartwood_difftest::store::ScratchDir;
use serde_json::{Value, json};

/// A scratch directory holding a git tree (`tree`), records, a ledger and a criteria file.
struct Rig {
    dir: ScratchDir,
}

/// What a command printed and whether it succeeded.
struct Ran {
    ok: bool,
    text: String,
}

impl Rig {
    fn new() -> Rig {
        let rig = Rig {
            dir: ScratchDir::new("gate").expect("scratch"),
        };
        std::fs::create_dir_all(rig.tree()).expect("tree");
        rig.git(&["init", "-q", "-b", "main"]);
        rig.write("src/a.rs", "fn a() {}\n");
        rig.write("src/p1.rs", "fn p1() {}\n");
        rig.write("docs/notes.md", "notes\n");
        rig.commit("first");
        rig
    }

    fn tree(&self) -> PathBuf {
        self.dir.path().join("tree")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn arg(&self, name: &str) -> String {
        self.path(name).to_string_lossy().into_owned()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(self.tree())
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .env("TZ", "UTC")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.tree().join(file);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, text).expect("write");
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// `difftest gate ARGS...` in the tree.
    fn gate(&self, args: &[&str]) -> Ran {
        let out = Command::new(env!("CARGO_BIN_EXE_difftest"))
            .arg("gate")
            .args(args)
            .args(if args.starts_with(&["ledger", "merge"]) {
                Vec::new()
            } else {
                vec![
                    "--repo".to_owned(),
                    self.tree().to_string_lossy().into_owned(),
                ]
            })
            .output()
            .expect("difftest runs");
        Ran {
            ok: out.status.success(),
            text: format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        }
    }

    /// A gate record of `target`, made by `gate record` after a command that printed `printed`.
    fn record(&self, target: &str, command: &str, exit: i32, printed: &str, started: &str) -> Ran {
        let output = self.arg("printed.txt");
        std::fs::write(&output, printed).expect("printed");
        self.gate(&[
            "record",
            "--target",
            target,
            "--command",
            command,
            "--exit",
            &exit.to_string(),
            "--started",
            started,
            "--finished",
            started,
            "--out",
            &self.arg("records"),
            "--criteria",
            &self.arg("criteria.json"),
            "--tested",
            "src",
            "--output",
            &output,
            "--machine",
            "box-1",
        ])
    }

    fn criteria(&self, value: &Value) {
        std::fs::write(self.path("criteria.json"), value.to_string()).expect("criteria");
    }

    fn records(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir(self.path("records")) else {
            return out;
        };
        let mut files: Vec<PathBuf> = dir.map(|e| e.expect("entry").path()).collect();
        files.sort();
        for file in files {
            out.push(read(&file));
        }
        out
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read")).expect("json")
}

/// A report as the comparison commands print it: `passed` cases, `n` hits of `reason`.
fn report(compared: u64, hits: &[(u64, &str)]) -> String {
    let mut text = format!("  {compared} compared: {compared} passed, 0 failed\n");
    if !hits.is_empty() {
        let total: u64 = hits.iter().map(|(n, _)| n).sum();
        text += &format!("  {total} documented differences (Heartwood differs on purpose):\n");
        for (n, reason) in hits {
            text += &format!("    {n:>5}  {reason}\n");
        }
    }
    text
}

/// The reason text of a documented difference the harness prints, for the hits below.
fn reason_of(name: &str) -> String {
    use heartwood_difftest::gate::names::{ALL, name as name_of};
    ALL.iter()
        .find(|d| name_of(**d) == name)
        .unwrap_or_else(|| panic!("no code name {name}"))
        .reason()
        .to_owned()
}

/// A small criteria file: one property, one target, two make targets and a register.
fn base_criteria() -> Value {
    json!({
        "format": "heartwood-s1-criteria/1",
        "totals": {"propertyCases": 1000, "propertyRuns": 2, "targetCpuHours": 1.0},
        "properties": {"P1": {"name": "fee-floor", "paths": ["src/p1.rs"]}},
        "targets": {"parse": {"paths": ["src/a.rs"]}},
        "narrowing": {"l0": ["src/a.rs"], "docs-free": ["src"]},
        "evidence": {},
        "rows": [
            {"id": "R1", "criterion": "vectors", "requires": "the vectors pass", "shownBy": "l0",
             "needs": [{"kind": "record", "target": "l0"}]},
            {"id": "R2", "criterion": "totals", "requires": "the totals are met", "shownBy": "ledger",
             "needs": [{"kind": "properties"}, {"kind": "fuzz"}]},
            {"id": "R3", "criterion": "later", "requires": "a live run", "shownBy": "live",
             "waits": "a live run that has not happened",
             "needs": [{"kind": "record", "target": "live"}]}
        ],
        "coverage": [
            {"row": 1, "item": "the first item",
             "levels": {"L0": [{"kind": "record", "target": "l0"}],
                        "L1": [{"kind": "properties", "ids": ["P1"], "cases": 500, "runs": 1}]}}
        ],
        "scenarios": [],
        "register": {"entries": [], "records": []}
    })
}

const T1: &str = "2026-10-01T10:00:00Z";
const T2: &str = "2026-10-01T11:00:00Z";
const T3: &str = "2026-10-01T12:00:00Z";

// ----- gate record

#[test]
fn a_record_keeps_the_narrow_paths_of_its_command_and_the_counts_and_hits_of_its_output() {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["register"]["entries"] = json!([]);
    rig.criteria(&criteria);
    let printed = report(
        300,
        &[
            (7, &reason_of("Documented::ReplayIndexOrder")),
            (3, &reason_of("ApiDifference::TieOrder")),
        ],
    );
    let ran = rig.record(
        "l1",
        "target/debug/difftest run l1 --property P1 --cases 300",
        0,
        &printed,
        T1,
    );
    assert!(ran.ok, "{}", ran.text);
    let records = rig.records();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["format"], "heartwood-gate-record/1");
    assert_eq!(
        record["testedPaths"],
        json!(["src/p1.rs"]),
        "the property's paths, not --tested"
    );
    assert_eq!(record["result"], "pass");
    assert_eq!(record["dirty"], false);
    assert_eq!(record["machine"], "box-1");
    assert_eq!(record["counts"]["compared"], 300);
    assert_eq!(record["counts"]["documented"], 10);
    assert_eq!(record["hits"]["Documented::ReplayIndexOrder"], 7);
    assert_eq!(record["hits"]["ApiDifference::TieOrder"], 3);
    assert_eq!(record["head"], rig.git(&["rev-parse", "HEAD"]));
    let name = rig
        .path("records")
        .join(format!("{T1}-l1.json").replace(':', "-"));
    assert!(
        name.exists(),
        "the record is named by its start and target: {name:?}"
    );
}

#[test]
fn a_record_of_a_command_without_narrowing_keeps_the_paths_it_was_given() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    let ran = rig.record("l2", "difftest run l2 corpus", 0, "", T1);
    assert!(ran.ok, "{}", ran.text);
    assert_eq!(rig.records()[0]["testedPaths"], json!(["src"]));
    assert!(rig.records()[0]["counts"].is_null());
}

#[test]
fn a_failed_command_makes_a_failed_record_and_a_bad_stamp_is_refused() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    let ran = rig.record("l0", "difftest run l0", 1, "", T1);
    assert!(ran.ok, "the record is written: {}", ran.text);
    assert_eq!(rig.records()[0]["result"], "fail");
    assert_eq!(rig.records()[0]["exitStatus"], 1);
    let bad = rig.record("l0", "difftest run l0", 0, "", "yesterday");
    assert!(!bad.ok);
    assert!(bad.text.contains("time stamp"), "{}", bad.text);
}

#[test]
fn a_dirty_tree_is_recorded_as_dirty_and_refuses_the_ledger_entry() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    rig.write("src/a.rs", "fn a() { let _ = 1; }\n");
    let printed = report(10, &[]);
    let ran = rig.gate(&[
        "record",
        "--target",
        "l1",
        "--command",
        "difftest run l1 --property P1",
        "--exit",
        "0",
        "--started",
        T1,
        "--finished",
        T1,
        "--out",
        &rig.arg("records"),
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        "kind=property id=P1 cases=10 seed=1",
        "--ledger-dir",
        &rig.arg("ledger"),
        "--output",
        &{
            std::fs::write(rig.path("p.txt"), &printed).expect("p");
            rig.arg("p.txt")
        },
    ]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("changes beside its head"), "{}", ran.text);
    assert_eq!(rig.records()[0]["dirty"], true);
    assert!(
        !rig.path("ledger").exists()
            || std::fs::read_dir(rig.path("ledger")).expect("dir").count() == 0
    );
}

// ----- the path rule and the stage table

fn s1(rig: &Rig, extra: &[&str]) -> Ran {
    let mut args = vec!["s1", "--criteria"];
    let criteria = rig.arg("criteria.json");
    let records = rig.arg("records");
    let ledger = rig.arg("ledger");
    args.push(&criteria);
    args.extend(["--records", &records, "--ledger", &ledger]);
    args.extend(extra);
    rig.gate(&args)
}

#[test]
fn a_missing_record_fails_its_row_and_a_waiting_row_stays_pending() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    let ran = s1(&rig, &[]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("R1"), "{}", ran.text);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("R3       pending"), "{}", ran.text);
    assert!(ran.text.contains("waits on a live run"), "{}", ran.text);
}

#[test]
fn a_documentation_only_commit_keeps_a_record_valid_and_a_change_to_a_tested_file_does_not() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    // The l0 record tests src/a.rs only.
    assert!(
        rig.record("l0", "difftest run l0", 0, &report(50, &[]), T1)
            .ok
    );
    let part = ["--part", "none"];
    let _ = part;
    let row1 = |rig: &Rig| s1(rig, &["--verbose"]).text;
    assert!(row1(&rig).contains("R1       pass"), "{}", row1(&rig));

    // A commit that touches documentation and a file the record does not test.
    rig.write("docs/notes.md", "more notes\n");
    rig.write("src/p1.rs", "fn p1() { let _ = 2; }\n");
    rig.commit("notes");
    let text = row1(&rig);
    assert!(text.contains("R1       pass"), "still valid: {text}");

    // A documentation file inside a tested directory changes no record either.
    rig.write("src/README.md", "about the module\n");
    rig.commit("readme");
    let text = row1(&rig);
    assert!(text.contains("R1       pass"), "still valid: {text}");

    // A commit that changes a tested file.
    rig.write("src/a.rs", "fn a() { let _ = 3; }\n");
    rig.commit("change a");
    let ran = s1(&rig, &[]);
    assert!(!ran.ok);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("no longer count"), "{}", ran.text);
    assert!(ran.text.contains("src/a.rs"), "{}", ran.text);

    // A new record at the new head counts again.
    assert!(
        rig.record("l0", "difftest run l0", 0, &report(50, &[]), T2)
            .ok
    );
    assert!(s1(&rig, &[]).text.contains("R1       pass"));
}

#[test]
fn a_record_of_a_dirty_tree_does_not_count_unless_dirty_records_are_allowed() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    rig.write("src/a.rs", "fn a() { let _ = 4; }\n");
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    let ran = s1(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("dirty tree"), "{}", ran.text);
    let allowed = s1(&rig, &["--allow-dirty"]);
    assert!(allowed.text.contains("R1       pass"), "{}", allowed.text);
}

#[test]
fn a_failed_record_blocks_its_row_until_a_later_pass_with_the_same_variables() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(rig.record("l0", "difftest run l0", 1, "", T1).ok);
    let ran = s1(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("failed record"), "{}", ran.text);
    assert!(rig.record("l0", "difftest run l0", 0, "", T2).ok);
    assert!(s1(&rig, &[]).text.contains("R1       pass"));
}

#[test]
fn a_strict_run_fails_while_a_row_is_pending_and_a_part_selects_rows() {
    let mut criteria = base_criteria();
    criteria["rows"] = json!([
        {"id": "R1", "criterion": "vectors", "requires": "pass", "shownBy": "l0", "parts": ["fast"],
         "needs": [{"kind": "record", "target": "l0"}]},
        {"id": "R3", "criterion": "later", "requires": "live", "shownBy": "live", "parts": ["slow"],
         "waits": "a live run", "needs": [{"kind": "record", "target": "live"}]}
    ]);
    let rig = Rig::new();
    rig.criteria(&criteria);
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    let fast = s1(&rig, &["--part", "fast", "--strict"]);
    assert!(fast.ok, "{}", fast.text);
    assert!(!fast.text.contains("R3"), "{}", fast.text);
    let all = s1(&rig, &[]);
    assert!(all.ok, "pending is not a failure: {}", all.text);
    let strict = s1(&rig, &["--strict"]);
    assert!(!strict.ok, "{}", strict.text);
    let unknown = s1(&rig, &["--part", "nothing"]);
    assert!(!unknown.ok);
}

// ----- the ledger

fn ledger_run(rig: &Rig, dir: &str, machine: &str, spec: &str, started: &str) -> Ran {
    rig.gate(&[
        "record",
        "--target",
        "l1-long",
        "--command",
        "difftest run l1 --property P1",
        "--exit",
        "0",
        "--started",
        started,
        "--finished",
        started,
        "--out",
        &rig.arg("records"),
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        spec,
        "--ledger-dir",
        &rig.arg(dir),
        "--machine",
        machine,
    ])
}

#[test]
fn the_ledger_sums_runs_across_machines_and_counts_independent_seeds() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(
        ledger_run(
            &rig,
            "ledger",
            "box-1",
            "kind=property id=P1 cases=600 seed=1",
            T1
        )
        .ok
    );
    // Another machine's ledger, merged in.
    assert!(
        ledger_run(
            &rig,
            "other",
            "box-2",
            "kind=property id=P1 cases=600 seed=2",
            T2
        )
        .ok
    );
    let merged = rig.gate(&[
        "ledger",
        "merge",
        "--from",
        &rig.arg("other"),
        "--machine",
        "box-2",
        "--ledger",
        &rig.arg("ledger"),
    ]);
    assert!(merged.ok, "{}", merged.text);
    assert!(merged.text.contains("1 entries added"), "{}", merged.text);
    // Merging again adds nothing.
    let again = rig.gate(&[
        "ledger",
        "merge",
        "--from",
        &rig.arg("other"),
        "--machine",
        "box-2",
        "--ledger",
        &rig.arg("ledger"),
    ]);
    assert!(again.text.contains("0 entries added"), "{}", again.text);

    let show = rig.gate(&[
        "ledger",
        "show",
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        &rig.arg("ledger"),
    ]);
    assert!(show.ok, "{}", show.text);
    let p1 = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("P1 "))
        .expect("P1 line");
    assert!(p1.contains("1,200 cases"), "{p1}");
    assert!(p1.contains("  2 runs"), "{p1}");
    assert!(p1.contains("met"), "{p1}");
}

#[test]
fn a_property_needs_its_runs_seeded_independently_and_a_target_its_cpu_hours() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    // Two runs with the same seed are one run.
    assert!(
        ledger_run(
            &rig,
            "ledger",
            "box-1",
            "kind=property id=P1 cases=1000 seed=1",
            T1
        )
        .ok
    );
    assert!(
        ledger_run(
            &rig,
            "ledger",
            "box-1",
            "kind=property id=P1 cases=1000 seed=1",
            T2
        )
        .ok
    );
    // 1800 s at 4 forks is 2 CPU hours.
    assert!(
        ledger_run(
            &rig,
            "ledger",
            "box-1",
            "kind=target id=parse seconds=1800 forks=4",
            T3
        )
        .ok
    );
    let args = [
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        &rig.arg("ledger"),
    ];
    let show = rig.gate(&[&["ledger", "show"], &args[..]].concat());
    let p1 = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("P1 "))
        .expect("P1 line");
    assert!(p1.contains("  1 runs"), "one seed is one run: {p1}");
    assert!(p1.contains("short"), "{p1}");
    let parse = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("parse"))
        .expect("parse line");
    assert!(
        parse.contains("2.00 hours") && parse.contains("met"),
        "{parse}"
    );
    let plan = rig.gate(&[&["ledger", "plan"], &args[..]].concat());
    assert!(plan.text.contains("P1"), "{}", plan.text);
    assert!(
        plan.text.contains("1 seeded runs still needed"),
        "{}",
        plan.text
    );
    assert!(plan.text.contains("new seeds only, not 1"), "{}", plan.text);
    assert!(
        !plan.text.contains("parse"),
        "a met target is not planned: {}",
        plan.text
    );
    let strict = rig.gate(&[&["ledger", "show", "--strict"], &args[..]].concat());
    assert!(!strict.ok);
}

#[test]
fn a_ledger_entry_goes_stale_when_its_property_changed_and_only_then() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(
        ledger_run(
            &rig,
            "ledger",
            "box-1",
            "kind=property id=P1 cases=1000 seed=1",
            T1
        )
        .ok
    );
    let args = [
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        &rig.arg("ledger"),
    ];
    rig.write("src/a.rs", "fn a() { let _ = 5; }\n");
    rig.commit("unrelated code");
    let show = rig.gate(&[&["ledger", "show"], &args[..]].concat());
    let p1 = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("P1 "))
        .expect("P1 line");
    assert!(p1.contains("1,000 cases"), "src/a.rs is not P1's: {p1}");
    rig.write("src/p1.rs", "fn p1() { let _ = 6; }\n");
    rig.commit("property code");
    let show = rig.gate(&[&["ledger", "show"], &args[..]].concat());
    let p1 = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("P1 "))
        .expect("P1 line");
    assert!(p1.contains("0 cases") || p1.contains(" 0 cases"), "{p1}");
    assert!(show.text.contains("not counted"), "{}", show.text);
}

#[test]
fn a_ledger_entry_that_misses_its_reach_is_not_counted() {
    let mut criteria = base_criteria();
    criteria["properties"]["P1"]["reach"] = json!({"minReach": 20});
    let rig = Rig::new();
    rig.criteria(&criteria);
    // A P13-style reach is read from the command's flag; P1 has none, so name the property P13.
    criteria["properties"]["P13"] =
        json!({"name": "processor", "paths": ["src/p1.rs"], "reach": {"minReach": 20}});
    rig.criteria(&criteria);
    let low = rig.gate(&[
        "record",
        "--target",
        "l1-long",
        "--command",
        "difftest run l1 --property P13 --min-reach 5 --cases 1000",
        "--exit",
        "0",
        "--started",
        T1,
        "--finished",
        T1,
        "--out",
        &rig.arg("records"),
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        "kind=property id=P13 cases=1000 seed=1",
        "--ledger-dir",
        &rig.arg("ledger"),
    ]);
    assert!(low.ok, "{}", low.text);
    let show = rig.gate(&[
        "ledger",
        "show",
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        &rig.arg("ledger"),
    ]);
    assert!(
        show.text.contains("reach below the criteria's"),
        "{}",
        show.text
    );
    let p13 = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("P13 "))
        .expect("P13 line");
    assert!(p13.contains(" 0 cases"), "{p13}");
    let enough = rig.gate(&[
        "record",
        "--target",
        "l1-long",
        "--command",
        "difftest run l1 --property P13 --min-reach 20 --cases 1000",
        "--exit",
        "0",
        "--started",
        T2,
        "--finished",
        T2,
        "--out",
        &rig.arg("records"),
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        "kind=property id=P13 cases=1000 seed=2",
        "--ledger-dir",
        &rig.arg("ledger"),
    ]);
    assert!(enough.ok, "{}", enough.text);
    let show = rig.gate(&[
        "ledger",
        "show",
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        &rig.arg("ledger"),
    ]);
    let p13 = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("P13 "))
        .expect("P13 line");
    assert!(p13.contains("1,000 cases"), "{p13}");
}

// ----- coverage

#[test]
fn a_coverage_row_passes_when_its_records_at_every_level_pass() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    let coverage = |rig: &Rig, extra: &[&str]| {
        let criteria = rig.arg("criteria.json");
        let records = rig.arg("records");
        let ledger = rig.arg("ledger");
        let mut args = vec![
            "coverage",
            "--criteria",
            &criteria,
            "--records",
            &records,
            "--ledger",
            &ledger,
        ];
        args.extend(extra);
        rig.gate(&args)
    };
    let ran = coverage(&rig, &["--strict"]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(
        ran.text.contains("2 levels: 0 pass, 2 missing"),
        "{}",
        ran.text
    );
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    assert!(
        ledger_run(
            &rig,
            "ledger",
            "box-1",
            "kind=property id=P1 cases=600 seed=1",
            T2
        )
        .ok
    );
    let ran = coverage(&rig, &["--strict", "--out", &rig.arg("cov")]);
    assert!(ran.ok, "{}", ran.text);
    assert!(
        ran.text.contains("2 pass, 0 missing, 0 fail"),
        "{}",
        ran.text
    );
    let written: Vec<_> = std::fs::read_dir(rig.path("cov")).expect("cov").collect();
    assert_eq!(written.len(), 1);
}

// ----- register

fn register_criteria(entries: Value, records: Value) -> Value {
    let mut criteria = base_criteria();
    criteria["register"] = json!({"entries": entries, "retired": ["OLD-1"], "records": records});
    criteria
}

fn register(rig: &Rig, extra: &[&str]) -> Ran {
    let criteria = rig.arg("criteria.json");
    let records = rig.arg("records");
    let mut args = vec!["register", "--criteria", &criteria, "--records", &records];
    args.extend(extra);
    rig.gate(&args)
}

fn entry(id: &str, trigger: &str, tests: Value, names: &[&str]) -> Value {
    json!({"id": id, "status": "active", "trigger": trigger, "codeNames": names, "tests": tests})
}

#[test]
fn an_entry_needs_a_trigger_and_a_test_that_exists() {
    let rig = Rig::new();
    rig.write("src/shows.rs", "#[test]\nfn it_shows_the_difference() {}\n");
    rig.commit("a test");
    let good = json!([{"kind": "cargo", "package": "p", "test": "t", "file": "src/shows.rs", "name": "it_shows_the_difference"}]);
    let missing_fn = json!([{"kind": "cargo", "package": "p", "test": "t", "file": "src/shows.rs", "name": "it_shows"}]);
    let missing_file =
        json!([{"kind": "cargo", "package": "p", "file": "src/nope.rs", "name": "x"}]);
    rig.criteria(&register_criteria(
        json!([
            entry("E-1", "when the block is old", good, &[]),
            entry("E-2", "", json!([{"kind": "cargo", "package": "p", "file": "src/shows.rs", "name": "it_shows_the_difference"}]), &[]),
            entry("E-3", "when x", missing_fn, &[]),
            entry("E-4", "when y", missing_file, &[]),
            entry("E-5", "when z", json!([]), &[]),
        ]),
        json!([]),
    ));
    let ran = register(&rig, &[]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("E-1  active   ok"), "{}", ran.text);
    assert!(ran.text.contains("E-2  active   FAIL"), "{}", ran.text);
    assert!(ran.text.contains("has no trigger"), "{}", ran.text);
    assert!(ran.text.contains("has no test it_shows"), "{}", ran.text);
    assert!(
        ran.text.contains("src/nope.rs is not there"),
        "{}",
        ran.text
    );
    assert!(ran.text.contains("names no test"), "{}", ran.text);
    assert!(ran.text.contains("1 ok, 4 fail"), "{}", ran.text);
}

#[test]
fn a_retired_entry_may_not_be_listed_and_a_claimed_name_must_be_real() {
    let rig = Rig::new();
    rig.write("src/shows.rs", "#[test]\nfn shown() {}\n");
    rig.commit("a test");
    let tests = json!([{"kind": "cargo", "package": "p", "file": "src/shows.rs", "name": "shown"}]);
    rig.criteria(&register_criteria(
        json!([
            entry("OLD-1", "when a", tests.clone(), &[]),
            entry("E-1", "when b", tests, &["Documented::NoSuchThing"]),
        ]),
        json!([]),
    ));
    let ran = register(&rig, &[]);
    assert!(!ran.ok);
    assert!(
        ran.text.contains("is retired and also listed"),
        "{}",
        ran.text
    );
    assert!(
        ran.text
            .contains("Documented::NoSuchThing, which is not a code name"),
        "{}",
        ran.text
    );
}

#[test]
fn an_honest_run_that_hits_an_entry_fails_and_an_intended_hit_is_counted() {
    let rig = Rig::new();
    rig.write("src/shows.rs", "#[test]\nfn shown() {}\n");
    rig.commit("a test");
    let tests = json!([{"kind": "cargo", "package": "p", "file": "src/shows.rs", "name": "shown"}]);
    let entries = json!([
        entry(
            "E-1",
            "when the index is replayed",
            tests.clone(),
            &["Documented::ReplayIndexOrder"]
        ),
        entry(
            "E-2",
            "when ties are sorted",
            tests,
            &["ApiDifference::TieOrder"]
        ),
    ]);
    let rules = json!([
        {"target": "live", "class": "honest", "allow": ["Documented::ReplayIndexOrder"]},
        {"target": "l1", "class": "shows"},
        {"target": "attack", "class": "intended"}
    ]);
    rig.criteria(&register_criteria(entries, rules));
    let replay = reason_of("Documented::ReplayIndexOrder");
    let ties = reason_of("ApiDifference::TieOrder");

    // The properties and an attack suite hit entries on purpose; an honest run may hit what it allows.
    assert!(
        rig.record(
            "l1",
            "difftest run l1",
            0,
            &report(100, &[(9, &replay), (4, &ties)]),
            T1
        )
        .ok
    );
    assert!(
        rig.record("attack", "make attack", 0, &report(10, &[(2, &ties)]), T2)
            .ok
    );
    assert!(
        rig.record("live", "make live", 0, &report(10, &[(1, &replay)]), T3)
            .ok
    );
    let ran = register(&rig, &[]);
    assert!(ran.ok, "{}", ran.text);
    assert!(ran.text.contains("counted hits"), "{}", ran.text);
    assert!(
        ran.text.contains("E-1  active   ok  10 hits"),
        "{}",
        ran.text
    );
    assert!(
        ran.text.contains("E-2  active   ok  6 hits"),
        "{}",
        ran.text
    );
    assert!(
        ran.text.contains("10  Documented::ReplayIndexOrder"),
        "{}",
        ran.text
    );
    assert!(
        ran.text.contains("6  ApiDifference::TieOrder"),
        "{}",
        ran.text
    );

    // An honest run that meets the other entry fails the register.
    assert!(
        rig.record(
            "live",
            "make live again",
            0,
            &report(10, &[(1, &ties)]),
            "2026-10-01T13:00:00Z"
        )
        .ok
    );
    let ran = register(&rig, &[]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("an honest run hits none"), "{}", ran.text);
    assert!(ran.text.contains("ApiDifference::TieOrder"), "{}", ran.text);
}

#[test]
fn a_hit_no_entry_claims_and_a_hit_of_a_record_without_a_rule_fail() {
    let rig = Rig::new();
    rig.write("src/shows.rs", "#[test]\nfn shown() {}\n");
    rig.commit("a test");
    let tests = json!([{"kind": "cargo", "package": "p", "file": "src/shows.rs", "name": "shown"}]);
    rig.criteria(&register_criteria(
        json!([entry(
            "E-1",
            "when a",
            tests,
            &["Documented::ReplayIndexOrder"]
        )]),
        json!([{"target": "l1", "class": "shows"}]),
    ));
    let other = reason_of("ApiDifference::TieOrder");
    let replay = reason_of("Documented::ReplayIndexOrder");
    assert!(
        rig.record("l1", "difftest run l1", 0, &report(10, &[(1, &other)]), T1)
            .ok
    );
    let ran = register(&rig, &[]);
    assert!(!ran.ok);
    assert!(ran.text.contains("which no entry claims"), "{}", ran.text);
    assert!(
        rig.record("l2", "difftest run l2", 0, &report(10, &[(1, &replay)]), T2)
            .ok
    );
    let ran = register(&rig, &[]);
    assert!(
        ran.text.contains("the register has no rule for it"),
        "{}",
        ran.text
    );
}

#[test]
fn a_record_test_needs_a_valid_passing_record_and_the_register_writes_its_own_record() {
    let rig = Rig::new();
    let record_test = json!([{"kind": "record", "target": "l0", "contains": ["difftest run l0"]}]);
    rig.criteria(&register_criteria(
        json!([entry("E-1", "when a", record_test, &[])]),
        json!([]),
    ));
    let ran = register(&rig, &[]);
    assert!(!ran.ok);
    assert!(
        ran.text.contains("no valid passing record of l0"),
        "{}",
        ran.text
    );
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    let out = rig.arg("own");
    let ran = register(&rig, &["--out", &out]);
    assert!(ran.ok, "{}", ran.text);
    // Without --run the tests were not run, so the record it writes is not a pass.
    assert!(ran.text.contains("tests were not run"), "{}", ran.text);
    let strict = register(&rig, &["--strict"]);
    assert!(!strict.ok, "{}", strict.text);
    let files: Vec<_> = std::fs::read_dir(rig.path("own"))
        .expect("own")
        .map(|e| e.expect("e").path())
        .collect();
    assert_eq!(files.len(), 1);
    let written = read(&files[0]);
    assert_eq!(written["target"], "register");
    assert_eq!(written["result"], "fail");
    assert_eq!(written["counts"]["compared"], 1);
}

// ----- the shape of the files

#[test]
fn a_criteria_file_with_an_unknown_field_or_row_is_refused() {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["surprise"] = json!(1);
    rig.criteria(&criteria);
    let ran = s1(&rig, &[]);
    assert!(!ran.ok);
    assert!(ran.text.contains("surprise"), "{}", ran.text);
    let mut criteria = base_criteria();
    criteria["rows"][1]["needs"] = json!([{"kind": "rows", "ids": ["R9"]}]);
    rig.criteria(&criteria);
    let ran = s1(&rig, &[]);
    assert!(!ran.ok);
    assert!(ran.text.contains("R9"), "{}", ran.text);
}

#[test]
fn a_command_that_is_not_a_gate_verb_is_a_usage_error() {
    let rig = Rig::new();
    let out = Command::new(env!("CARGO_BIN_EXE_difftest"))
        .args(["gate", "nothing"])
        .output()
        .expect("runs");
    assert_eq!(out.status.code(), Some(2));
    let _ = rig;
}

// ----- more of the ledger, the records, the path rule and the register

#[test]
fn runs_of_one_seed_count_their_cases_once() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    // One seed draws the same cases: its longest run holds every shorter one.
    for (spec, started) in [
        ("kind=property id=P1 cases=1000 seed=1", T1),
        ("kind=property id=P1 cases=400 seed=1", T2),
        ("kind=property id=P1 cases=300 seed=2", T3),
    ] {
        assert!(ledger_run(&rig, "ledger", "box-1", spec, started).ok);
    }
    let args = [
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        &rig.arg("ledger"),
    ];
    let show = rig.gate(&[&["ledger", "show"], &args[..]].concat());
    let p1 = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("P1 "))
        .expect("P1 line");
    assert!(p1.contains("1,300 cases"), "{p1}");
    assert!(p1.contains("  2 runs"), "{p1}");
    assert!(p1.contains("met"), "{p1}");
    let plan = rig.gate(&[&["ledger", "plan"], &args[..]].concat());
    assert!(
        !plan.text.contains("P1 "),
        "a met property is not planned: {}",
        plan.text
    );
}

#[test]
fn a_failed_record_blocks_its_row_even_beside_enough_passes() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    assert!(rig.record("l0", "difftest run l0", 1, "", T2).ok);
    let ran = s1(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("failed record"), "{}", ran.text);
    assert!(rig.record("l0", "difftest run l0", 0, "", T3).ok);
    assert!(s1(&rig, &[]).text.contains("R1       pass"));
}

#[test]
fn two_records_of_one_target_started_in_one_second_are_both_kept() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    assert!(rig.record("l0", "difftest run l0", 1, "", T1).ok);
    let records = rig.records();
    assert_eq!(records.len(), 2, "{records:?}");
    let results: Vec<&Value> = records.iter().map(|r| &r["result"]).collect();
    assert!(results.contains(&&json!("pass")) && results.contains(&&json!("fail")));
}

#[test]
fn a_record_file_that_is_not_json_is_an_error_not_a_skip() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    std::fs::write(rig.path("records").join("cut-short.json"), "{\"format\": ").expect("write");
    let ran = s1(&rig, &[]);
    assert!(!ran.ok);
    assert!(ran.text.contains("cut-short.json"), "{}", ran.text);
}

#[test]
fn every_documented_reason_a_report_lists_is_a_hit() {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["register"] = json!({
        "entries": [entry(
            "E-1",
            "when a refused sender had no key",
            json!([{"kind": "record", "target": "l0"}]),
            &["levels::BOUND_KEY"]
        )],
        "records": [{"target": "l2", "class": "honest"}]
    });
    rig.criteria(&criteria);
    let printed = report(
        10,
        &[
            (2, heartwood_difftest::levels::BOUND_KEY),
            (1, "a difference no name is given"),
        ],
    );
    assert!(
        rig.record("l2", "difftest run l2 corpus", 0, &printed, T1)
            .ok
    );
    let hits = &rig.records()[0]["hits"];
    assert_eq!(hits["levels::BOUND_KEY"], 2, "{hits}");
    assert_eq!(hits["unnamed: a difference no name is given"], 1, "{hits}");
    let ran = register(&rig, &[]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("an honest run hits none"), "{}", ran.text);
    assert!(ran.text.contains("which no entry claims"), "{}", ran.text);
}

#[test]
fn a_documentation_file_a_record_tests_makes_it_stale() {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["narrowing"]["guide-check"] = json!(["docs/notes.md"]);
    criteria["rows"][0]["needs"] = json!([{"kind": "record", "target": "guide-check"}]);
    rig.criteria(&criteria);
    assert!(rig.record("guide-check", "check the guide", 0, "", T1).ok);
    assert!(s1(&rig, &[]).text.contains("R1       pass"));
    rig.write("docs/notes.md", "the guide changed\n");
    rig.commit("guide");
    let ran = s1(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("docs/notes.md"), "{}", ran.text);
}
