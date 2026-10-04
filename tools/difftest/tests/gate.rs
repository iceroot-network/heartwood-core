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
        "format": "heartwood-baseline-criteria/1",
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
        json!(["src/p1.rs", "tools/gate-record.sh"]),
        "the property's paths and the recorder, not --tested"
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
fn a_recorded_fuzz_run_is_judged_by_its_status_line_and_tests_the_script_that_judges_it() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fuzz-run.sh");
    let rig = Rig::new();
    let log = rig.arg("fuzz.log");
    let line = "#9: cov: 12 ft: 30 corp: 4 exec/s: 100 oom/timeout/crash: 0/0/0 time: 60s job: 3";
    let run = |printed: &str, exit: i32| -> (Option<i32>, String) {
        let body = format!("printf '%s\\n' '{printed}'; exit {exit}");
        let out = Command::new(&script)
            .args([log.as_str(), "sh", "-c", &body])
            .output()
            .expect("fuzz-run.sh runs");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    let (status, said) = run(line, 0);
    assert_eq!(status, Some(0), "{said}");
    assert!(said.contains("fuzz-run: #9: cov: 12"), "{said}");
    // No status line, a status line that counts a stop, and a failed fuzzer each fail.
    assert_eq!(run("INFO: done", 0).0, Some(1));
    let crashed = line.replace("0/0/0", "0/0/1");
    assert_eq!(run(&crashed, 0).0, Some(1));
    assert_eq!(run(line, 77).0, Some(77));
    // The record of such a run tests its target's paths, the recorder and the script.
    rig.criteria(&base_criteria());
    let command =
        "tools/fuzz-run.sh target/fuzz-long/parse.log cargo +nightly fuzz run parse -- -fork=2";
    assert!(rig.record("fuzz-long", command, 0, line, T1).ok);
    assert_eq!(
        rig.records()[0]["testedPaths"],
        json!(["src/a.rs", "tools/fuzz-run.sh", "tools/gate-record.sh"])
    );
}

/// tools/gate-record.sh run on `command` with a stand-in harness that keeps the arguments it was
/// given; the child, and the file the stand-in writes them to.
fn recorder(rig: &Rig, command: &[&str]) -> (std::process::Child, PathBuf) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../gate-record.sh");
    let given = rig.path("given.txt");
    let stand_in = rig.path("stand-in.sh");
    std::fs::write(
        &stand_in,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", given.display()),
    )
    .expect("stand-in");
    let chmod = Command::new("chmod")
        .args(["+x", &stand_in.to_string_lossy()])
        .status()
        .expect("chmod");
    assert!(chmod.success());
    let child = Command::new(script)
        .args(command)
        .current_dir(rig.tree())
        .env("GATE_RECORDS", rig.arg("records"))
        .env("GATE_TARGET", "t")
        .env("GATE_DIFFTEST", &stand_in)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("gate-record.sh runs");
    (child, given)
}

#[test]
fn the_recorder_keeps_the_command_s_signals_and_marks_a_run_it_was_asked_to_stop() {
    let rig = Rig::new();
    // The command handles an interrupt and a quit as it would have alone.
    let (child, given) = recorder(&rig, &["sh", "-c", "grep SigIgn /proc/self/status"]);
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    let mask = printed
        .split_whitespace()
        .nth(1)
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .expect("a SigIgn mask");
    assert_eq!(mask & 0b110, 0, "SIGINT or SIGQUIT ignored: {printed}");
    let args = std::fs::read_to_string(&given).expect("given");
    assert!(!args.contains("--interrupted"), "{args}");
    // A terminate sent to the recorder reaches the command, and the record says so.
    let (mut child, given) = recorder(&rig, &["sleep", "30"]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let term = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(term.success());
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(143));
    let args = std::fs::read_to_string(&given).expect("given");
    assert!(
        args.contains("--interrupted") && args.contains("143"),
        "{args}"
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
fn a_failed_long_run_is_recorded_whatever_the_explanations_file_the_criteria_name() {
    // The criteria name an explanations file that is not there (a machine the file was not
    // copied to): the failed run still leaves its failed record and its failed ledger entry, so
    // the totals see the failure. Recording reads the criteria for the tested paths alone.
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["explanations"] = json!("missing-explanations.json");
    rig.criteria(&criteria);
    let printed = report(10, &[]);
    std::fs::write(rig.path("p.txt"), &printed).expect("p");
    let ran = rig.gate(&[
        "record",
        "--target",
        "l1",
        "--command",
        "difftest run l1 --property P1",
        "--exit",
        "2",
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
        &rig.arg("p.txt"),
    ]);
    assert!(ran.ok, "{}", ran.text);
    assert_eq!(rig.records().len(), 1, "{}", ran.text);
    assert_eq!(rig.records()[0]["result"], "fail");
    let entries: Vec<PathBuf> = std::fs::read_dir(rig.path("ledger"))
        .expect("the ledger entry is written")
        .map(|e| e.expect("entry").path())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(read(&entries[0])["result"], "fail");
    // The stage table still needs the file: it reads the explanations.
    assert!(!baseline(&rig, &[]).ok);
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

fn baseline(rig: &Rig, extra: &[&str]) -> Ran {
    let mut args = vec!["baseline", "--criteria"];
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
    let ran = baseline(&rig, &[]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("R1"), "{}", ran.text);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("R3       pending"), "{}", ran.text);
    assert!(ran.text.contains("waits on a live run"), "{}", ran.text);
}

/// The criteria with a row R4 that needs the scope audit `scope.md` of the rows `ids`, written
/// with `rows` (`id`, status).
fn scope_rig(rows: &[(&str, &str)], ids: &[&str], accepted: Option<Value>) -> Rig {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["scope"] = json!({"file": "scope.md", "rowIds": ids});
    if let Some(accepted) = accepted {
        criteria["scope"]["accepted"] = accepted;
    }
    criteria["rows"].as_array_mut().expect("rows").push(
        json!({"id": "R4", "criterion": "scope", "requires": "every row settled",
                     "shownBy": "the scope audit", "needs": [{"kind": "scope"}]}),
    );
    rig.criteria(&criteria);
    let mut table = String::from("| ID | Item | Status |\n|---|---|---|\n");
    for (id, status) in rows {
        table.push_str(&format!("| {id} | an item | {status} |\n"));
    }
    std::fs::write(rig.path("scope.md"), table).expect("scope");
    rig
}

#[test]
fn the_scope_audit_passes_only_with_every_row_settled_or_accepted() {
    let ids = ["KEEP1", "REMOVE1", "DEFER1"];
    let settled = [
        ("KEEP1", "mapped"),
        ("REMOVE1", "absent"),
        ("DEFER1", "unbuilt"),
    ];
    let text = baseline(&scope_rig(&settled, &ids, None), &["--verbose"]).text;
    assert!(text.contains("R4       pass"), "{text}");
    // A REMOVE row the audit records as a deviation waits on a person, named.
    let deviation = [
        ("KEEP1", "mapped"),
        ("REMOVE1", "deviation"),
        ("DEFER1", "unbuilt"),
    ];
    let text = baseline(&scope_rig(&deviation, &ids, None), &["--verbose"]).text;
    assert!(text.contains("R4       pending"), "{text}");
    assert!(text.contains("REMOVE1 (deviation)"), "{text}");
    // Accepted by a named decision, it passes; an unsettled row cannot be accepted.
    let accepted = Some(json!({"REMOVE1": "the decision that accepts it"}));
    let text = baseline(
        &scope_rig(&deviation, &ids, accepted.clone()),
        &["--verbose"],
    )
    .text;
    assert!(text.contains("R4       pass"), "{text}");
    assert!(text.contains("accepted"), "{text}");
    let unsure = [
        ("KEEP1", "unsure"),
        ("REMOVE1", "deviation"),
        ("DEFER1", "unbuilt"),
    ];
    let text = baseline(&scope_rig(&unsure, &ids, accepted), &["--verbose"]).text;
    assert!(text.contains("R4       pending"), "{text}");
    assert!(text.contains("KEEP1 (unsure)"), "{text}");
    // A row missing from the audit, or named twice, fails it.
    let missing = [("KEEP1", "mapped"), ("DEFER1", "unbuilt")];
    let text = baseline(&scope_rig(&missing, &ids, None), &[]).text;
    assert!(text.contains("R4       FAIL"), "{text}");
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
    let row1 = |rig: &Rig| baseline(rig, &["--verbose"]).text;
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
    let ran = baseline(&rig, &[]);
    assert!(!ran.ok);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("no longer count"), "{}", ran.text);
    assert!(ran.text.contains("src/a.rs"), "{}", ran.text);

    // A new record at the new head counts again.
    assert!(
        rig.record("l0", "difftest run l0", 0, &report(50, &[]), T2)
            .ok
    );
    assert!(baseline(&rig, &[]).text.contains("R1       pass"));
}

#[test]
fn a_record_of_a_dirty_tree_does_not_count_unless_dirty_records_are_allowed() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    rig.write("src/a.rs", "fn a() { let _ = 4; }\n");
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    let ran = baseline(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("dirty tree"), "{}", ran.text);
    let allowed = baseline(&rig, &["--allow-dirty"]);
    assert!(allowed.text.contains("R1       pass"), "{}", allowed.text);
}

#[test]
fn an_unexplained_failed_record_blocks_its_row_even_after_a_passing_repeat() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(rig.record("l0", "difftest run l0", 1, "", T1).ok);
    let ran = baseline(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("failed record"), "{}", ran.text);
    assert!(rig.record("l0", "difftest run l0", 0, "", T2).ok);
    let ran = baseline(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("no explanation"), "{}", ran.text);
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
    let fast = baseline(&rig, &["--part", "fast", "--strict"]);
    assert!(fast.ok, "{}", fast.text);
    assert!(!fast.text.contains("R3"), "{}", fast.text);
    let all = baseline(&rig, &[]);
    assert!(all.ok, "pending is not a failure: {}", all.text);
    let strict = baseline(&rig, &["--strict"]);
    assert!(!strict.ok, "{}", strict.text);
    let unknown = baseline(&rig, &["--part", "nothing"]);
    assert!(!unknown.ok);
}

#[test]
fn a_stale_record_is_missing_evidence_so_a_waiting_row_stays_pending() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    // The live record tests src (it has no narrowing); the waiting row R3 reads it.
    assert!(rig.record("live", "make live", 0, "", T1).ok);
    assert!(baseline(&rig, &[]).text.contains("R3       pass"));
    rig.write("src/a.rs", "fn a() { let _ = 7; }\n");
    rig.commit("change a");
    let ran = baseline(&rig, &["--verbose"]);
    assert!(
        ran.text.contains("R3       pending"),
        "a stale run is no failed run: {}",
        ran.text
    );
    assert!(ran.text.contains("missing live"), "{}", ran.text);
    assert!(ran.text.contains("no longer count"), "{}", ran.text);
    assert!(ran.text.contains("src/a.rs"), "{}", ran.text);
    // A dirty record is still a fault, however the row waits.
    rig.write("src/a.rs", "fn a() { let _ = 8; }\n");
    assert!(rig.record("live", "make live", 0, "", T2).ok);
    rig.git(&["checkout", "--", "src/a.rs"]);
    let ran = baseline(&rig, &[]);
    assert!(ran.text.contains("R3       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("dirty tree"), "{}", ran.text);
}

// ----- the ledger

fn ledger_run(rig: &Rig, dir: &str, machine: &str, spec: &str, started: &str) -> Ran {
    ledger_run_as(
        rig,
        dir,
        machine,
        spec,
        started,
        ("difftest run l1 --property P1 --strict", 0),
    )
}

/// A long run's record and ledger entry, its command and exit status as given.
fn ledger_run_as(
    rig: &Rig,
    dir: &str,
    machine: &str,
    spec: &str,
    started: &str,
    (command, exit): (&str, i32),
) -> Ran {
    ledger_run_with(rig, dir, machine, spec, started, (command, exit), false)
}

/// A long run's record and ledger entry, with its recorder's interrupt mark. A run that passed
/// printed its report: every case of its spec compared and passed.
fn ledger_run_with(
    rig: &Rig,
    dir: &str,
    machine: &str,
    spec: &str,
    started: &str,
    (command, exit): (&str, i32),
    interrupted: bool,
) -> Ran {
    let cases = spec
        .split_whitespace()
        .find_map(|w| w.strip_prefix("cases="))
        .unwrap_or("0");
    let printed = if exit == 0 {
        format!("  {cases} compared: {cases} passed, 0 failed\n")
    } else {
        String::new()
    };
    let output = rig.arg("printed-ledger.txt");
    std::fs::write(&output, printed).expect("printed");
    let exit = exit.to_string();
    let mut args = vec![
        "record",
        "--target",
        "l1-long",
        "--command",
        command,
        "--exit",
        &exit,
        "--started",
        started,
        "--finished",
        started,
        "--output",
        &output,
        "--ledger",
        spec,
        "--machine",
        machine,
    ];
    let (records, criteria, ledger) = (rig.arg("records"), rig.arg("criteria.json"), rig.arg(dir));
    args.extend([
        "--out",
        &records,
        "--criteria",
        &criteria,
        "--ledger-dir",
        &ledger,
    ]);
    if interrupted {
        args.push("--interrupted");
    }
    rig.gate(&args)
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
fn a_short_total_of_stale_runs_is_missing_so_a_waiting_row_stays_pending() {
    let mut criteria = base_criteria();
    criteria["rows"][1]["waits"] = json!("the long compute");
    let rig = Rig::new();
    rig.criteria(&criteria);
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
            "kind=target id=parse seconds=600 forks=1",
            T1
        )
        .ok
    );
    // Both runs go stale: each tested file changes.
    rig.write("src/p1.rs", "fn p1() { let _ = 9; }\n");
    rig.write("src/a.rs", "fn a() { let _ = 9; }\n");
    rig.commit("both changed");
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    let ran = baseline(&rig, &[]);
    assert!(ran.ok, "pending is not a failure: {}", ran.text);
    assert!(ran.text.contains("R2       pending"), "{}", ran.text);
    assert!(
        ran.text.contains("waits on the long compute"),
        "{}",
        ran.text
    );
    assert!(ran.text.contains("1 stale run not counted"), "{}", ran.text);
    let strict = baseline(&rig, &["--strict"]);
    assert!(!strict.ok, "the final gate still fails a short total");
    // A failed run is a failure, waits or not.
    assert!(
        rig.gate(&[
            "record",
            "--target",
            "l1-long",
            "--command",
            "difftest run l1 --property P1 --cases 10",
            "--exit",
            "1",
            "--started",
            T2,
            "--finished",
            T2,
            "--out",
            &rig.arg("records"),
            "--criteria",
            &rig.arg("criteria.json"),
            "--ledger",
            "kind=property id=P1 cases=10 seed=2",
            "--ledger-dir",
            &rig.arg("ledger"),
        ])
        .ok
    );
    let ran = baseline(&rig, &[]);
    assert!(ran.text.contains("R2       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("failed run"), "{}", ran.text);
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
    // Each run printed its report: its cases and its reach check compared.
    let printed = rig.arg("printed-reach.txt");
    std::fs::write(&printed, "  1001 compared: 1001 passed, 0 failed\n").expect("printed");
    let low = rig.gate(&[
        "record",
        "--target",
        "l1-long",
        "--command",
        "difftest run l1 --property P13 --min-reach 5 --cases 1000 --strict",
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
        "--output",
        &printed,
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
        "difftest run l1 --property P13 --min-reach 20 --cases 1000 --strict",
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
        "--output",
        &printed,
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
    // Levels without evidence are not a failure: a dry run writes no record that would block.
    let dry = coverage(&rig, &["--out", &rig.arg("cov-dry")]);
    assert!(dry.ok, "{}", dry.text);
    assert!(dry.text.contains("no record written"), "{}", dry.text);
    assert!(!rig.path("cov-dry").exists());
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
    assert!(ran.text.contains("1 ok, 0 pending, 4 fail"), "{}", ran.text);
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

#[test]
fn a_record_test_that_waits_leaves_its_entry_pending_until_its_record_runs() {
    let rig = Rig::new();
    let waiting = json!([{"kind": "record", "target": "l2", "waits": "the chains of a later run"}]);
    rig.criteria(&register_criteria(
        json!([entry("E-1", "when a", waiting, &[])]),
        json!([{"target": "l2", "class": "honest"}]),
    ));
    let out = rig.arg("own");
    let ran = register(&rig, &["--out", &out]);
    assert!(ran.ok, "pending is not a failure: {}", ran.text);
    assert!(ran.text.contains("E-1  active   pending"), "{}", ran.text);
    assert!(
        ran.text
            .contains("waits on the chains of a later run: no valid passing record of l2"),
        "{}",
        ran.text
    );
    assert!(ran.text.contains("0 ok, 1 pending, 0 fail"), "{}", ran.text);
    let strict = register(&rig, &["--strict"]);
    assert!(!strict.ok, "{}", strict.text);
    // A pending entry is neither a pass nor a failure: no record of the register is written.
    assert!(ran.text.contains("no record written"), "{}", ran.text);
    assert!(!rig.path("own").exists(), "no record while entries wait");

    // A valid record that failed is a failure, not a wait.
    assert!(rig.record("l2", "difftest run l2 corpus", 1, "", T1).ok);
    let ran = register(&rig, &[]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("E-1  active   FAIL"), "{}", ran.text);
    assert!(rig.record("l2", "difftest run l2 corpus", 0, "", T2).ok);
    let ran = register(&rig, &[]);
    assert!(ran.text.contains("E-1  active   ok"), "{}", ran.text);
}

#[test]
fn the_register_reads_no_hits_from_the_records_of_the_commands_that_judge_records() {
    let rig = Rig::new();
    rig.write("src/shows.rs", "#[test]\nfn shown() {}\n");
    rig.commit("a test");
    let tests = json!([{"kind": "cargo", "package": "p", "file": "src/shows.rs", "name": "shown"}]);
    rig.criteria(&register_criteria(
        json!([entry(
            "E-1",
            "when the index is replayed",
            tests,
            &["Documented::ReplayIndexOrder"]
        )]),
        json!([{"target": "l1", "class": "shows"}]),
    ));
    let replay = reason_of("Documented::ReplayIndexOrder");
    assert!(
        rig.record("l1", "difftest run l1", 0, &report(10, &[(3, &replay)]), T1)
            .ok
    );
    // The register's own record (and a coverage record) in the records' directory.
    let records = rig.arg("records");
    let first = register(&rig, &["--out", &records]);
    assert!(first.ok, "{}", first.text);
    assert!(rig.records().iter().any(|r| r["target"] == "register"));
    let second = register(&rig, &[]);
    assert!(
        second.ok,
        "a second reading is not a hit problem: {}",
        second.text
    );
    assert!(
        second.text.contains("E-1  active   ok  3 hits"),
        "each hit counted once: {}",
        second.text
    );
    assert!(!second.text.contains("no rule for it"), "{}", second.text);
}

// ----- the shape of the files

#[test]
fn a_criteria_file_with_an_unknown_field_or_row_is_refused() {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["surprise"] = json!(1);
    rig.criteria(&criteria);
    let ran = baseline(&rig, &[]);
    assert!(!ran.ok);
    assert!(ran.text.contains("surprise"), "{}", ran.text);
    let mut criteria = base_criteria();
    criteria["rows"][1]["needs"] = json!([{"kind": "rows", "ids": ["R9"]}]);
    rig.criteria(&criteria);
    let ran = baseline(&rig, &[]);
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
    let ran = baseline(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("failed record"), "{}", ran.text);
    assert!(rig.record("l0", "difftest run l0", 0, "", T3).ok);
    assert!(baseline(&rig, &[]).text.contains("R1       FAIL"));
}

/// A record of `target` whose make command line set `variables`.
fn record_of(
    rig: &Rig,
    target: &str,
    variables: &str,
    exit: i32,
    printed: &str,
    started: &str,
) -> Ran {
    let output = rig.arg("printed.txt");
    std::fs::write(&output, printed).expect("printed");
    let exit = exit.to_string();
    rig.gate(&[
        "record",
        "--target",
        target,
        "--command",
        &format!("make {target} {variables}"),
        "--variables",
        variables,
        "--exit",
        &exit,
        "--started",
        started,
        "--finished",
        started,
        "--out",
        &rig.arg("records"),
        "--criteria",
        &rig.arg("criteria.json"),
        "--tested",
        "src",
        "--output",
        &output,
        "--machine",
        "box-1",
    ])
}

/// The base criteria with one more row, `T`, of `needs`.
fn with_row(needs: Value) -> Value {
    let mut criteria = base_criteria();
    criteria["rows"].as_array_mut().expect("rows").push(
        json!({"id": "T", "criterion": "t", "requires": "t", "shownBy": "t", "needs": needs}),
    );
    criteria
}

/// The line of row `T` and the lines of its needs in `text`.
fn row_t(text: &str) -> String {
    let start = text.find("\nT ").map_or(0, |i| i + 1);
    text[start..].lines().take(3).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_chain_need_follows_the_runs_its_live_check_counts() {
    // The chains of the runs the live check counts (seeds 11 and 13), each judged once; a chain
    // of another run (2, an earlier run's under the same pattern) neither stands in for one of
    // them nor blocks them.
    let rig = Rig::new();
    let runs = json!({"target": "live-check", "contains": ["RUN=mixed-short"]});
    rig.criteria(&with_row(json!([
        {"kind": "record", "target": "live-check", "contains": ["RUN=mixed-short"], "min": 2,
         "distinct": true, "distinctBy": ["SEED"]},
        {"kind": "record", "target": "corpus-judge",
         "contains": ["NAME=live-mixed-short-{n}", "PROCESSOR=1"], "min": 2, "distinct": true,
         "distinctBy": ["NAME"], "seedsOf": runs}
    ])));
    for (variables, started) in [
        ("RUN=mixed-short SEED=11", T1),
        ("RUN=mixed-short SEED=13", T1),
    ] {
        assert!(record_of(&rig, "live-check", variables, 0, "", started).ok);
    }
    for (variables, exit) in [
        ("NAME=live-mixed-short-11 PROCESSOR=1", 0),
        ("NAME=live-mixed-short-2 PROCESSOR=1", 0),
    ] {
        assert!(record_of(&rig, "corpus-judge", variables, exit, "", T2).ok);
    }
    let text = baseline(&rig, &["--verbose"]).text;
    assert!(!text.contains("T        pass"), "{text}");
    assert!(text.contains("live-mixed-short-13"), "{text}");
    // A failed judgement of the earlier run's chain blocks nothing; the counted run's chain
    // judged, the row passes.
    assert!(
        record_of(
            &rig,
            "corpus-judge",
            "NAME=live-mixed-short-3 PROCESSOR=1",
            1,
            "",
            T2
        )
        .ok
    );
    assert!(
        record_of(
            &rig,
            "corpus-judge",
            "NAME=live-mixed-short-13 PROCESSOR=1",
            0,
            "",
            T3
        )
        .ok
    );
    let text = baseline(&rig, &["--verbose"]).text;
    assert!(row_t(&text).contains("T        pass"), "{text}");
    // A failed judgement of a counted run's chain blocks it.
    assert!(
        record_of(
            &rig,
            "corpus-judge",
            "NAME=live-mixed-short-11 PROCESSOR=1",
            1,
            "",
            "2026-10-01T13:00:00Z"
        )
        .ok
    );
    let text = baseline(&rig, &["--verbose"]).text;
    assert!(row_t(&text).contains("T        FAIL"), "{text}");
}

#[test]
fn one_run_counts_once_whatever_the_order_or_the_machine_settings_of_its_command_line() {
    let rig = Rig::new();
    rig.criteria(&with_row(json!([{"kind": "record", "target": "live-check",
        "contains": ["RUN=mixed-short"], "min": 2, "distinct": true, "distinctBy": ["SEED"]}])));
    for (variables, started) in [
        ("SEED=1 RUN=mixed-short", T1),
        ("RUN=mixed-short SEED=1", T2),
        ("IMAGE=img DEVNET_PREFIX=127.1.0 SEED=1 RUN=mixed-short", T3),
        ("RUN=mixed-short", "2026-10-01T13:00:00Z"),
    ] {
        assert!(record_of(&rig, "live-check", variables, 0, "", started).ok);
    }
    let ran = baseline(&rig, &["--verbose"]);
    let t = row_t(&ran.text);
    assert!(t.contains("1 of 2 passing runs"), "{t}");
    assert!(t.contains("1 record without SEED not counted"), "{t}");
    assert!(
        record_of(
            &rig,
            "live-check",
            "SEED=2 RUN=mixed-short",
            0,
            "",
            "2026-10-01T14:00:00Z"
        )
        .ok
    );
    let t = row_t(&baseline(&rig, &["--verbose"]).text);
    assert!(t.contains("T        pass"), "{t}");
    assert!(t.contains("2 passing runs"), "{t}");
}

#[test]
fn a_failure_is_superseded_only_when_explained_and_repeated_once() {
    let rig = Rig::new();
    let mut criteria = with_row(json!([{"kind": "record", "target": "live-check",
        "contains": ["RUN=mixed"], "min": 1, "distinct": true, "distinctBy": ["SEED"]}]));
    criteria["explanations"] = json!("explained.json");
    rig.criteria(&criteria);
    let explain = |records: &[&str]| {
        let failures: Vec<Value> = records
            .iter()
            .map(|r| json!({"record": r, "cause": "the host stalled", "evidence": "load.jsonl"}))
            .collect();
        let file = json!({"format": "heartwood-baseline-explanations/1", "failures": failures});
        std::fs::write(rig.path("explained.json"), file.to_string()).expect("explanations");
    };
    explain(&[]);
    // Another machine's loopback block and another word order name the same run.
    assert!(
        record_of(
            &rig,
            "live-check",
            "DEVNET_PREFIX=127.33.0 SEED=1 RUN=mixed",
            1,
            "",
            T1
        )
        .ok
    );
    assert!(record_of(&rig, "live-check", "RUN=mixed SEED=1", 0, "", T2).ok);
    let t = row_t(&baseline(&rig, &["--verbose"]).text);
    assert!(
        t.contains("T        FAIL") && t.contains("no explanation"),
        "{t}"
    );
    let failed = format!("{}-live-check.json", T1.replace(':', "-"));
    explain(&[&failed]);
    let t = row_t(&baseline(&rig, &["--verbose"]).text);
    assert!(t.contains("T        pass"), "{t}");
    assert!(
        t.contains("1 explained failure repeated") && t.contains("the host stalled"),
        "{t}"
    );
    // The repeat failed too: the run is not repeated again.
    assert!(record_of(&rig, "live-check", "SEED=1 RUN=mixed", 1, "", T3).ok);
    let t = row_t(&baseline(&rig, &["--verbose"]).text);
    assert!(
        t.contains("T        FAIL") && t.contains("failed more than once"),
        "{t}"
    );
}

/// Row `T`, one need matched by the whole run, after the live-check records `runs` (variables,
/// exit status, start), every failure explained.
fn row_t_after_explained(runs: &[(&str, i32, &str)]) -> String {
    let rig = Rig::new();
    let mut criteria = with_row(json!([{"kind": "record", "target": "live-check",
        "contains": ["RUN=mixed"]}]));
    criteria["explanations"] = json!("explained.json");
    rig.criteria(&criteria);
    let failures: Vec<Value> = (runs.iter().filter(|(_, exit, _)| *exit != 0))
        .map(|(_, _, started)| {
            let record = format!("{}-live-check.json", started.replace(':', "-"));
            json!({"record": record, "cause": "the host stalled", "evidence": "load.jsonl"})
        })
        .collect();
    let file = json!({"format": "heartwood-baseline-explanations/1", "failures": failures});
    std::fs::write(rig.path("explained.json"), file.to_string()).expect("explanations");
    for (variables, exit, started) in runs {
        assert!(record_of(&rig, "live-check", variables, *exit, "", started).ok);
    }
    row_t(&baseline(&rig, &["--verbose"]).text)
}

/// A run's variables with a criteria variable among them.
const WITH_CRITERIA: &str = "RUN=mixed SEED=1 STAGE_CRITERIA=docs/stage.json";

#[test]
fn an_explained_failure_is_repeated_by_a_pass_that_set_another_criteria_variable_or_none() {
    for repeat in [
        "RUN=mixed SEED=1",
        "RUN=mixed SEED=1 BASELINE_CRITERIA=docs/baseline.json",
    ] {
        let t = row_t_after_explained(&[(WITH_CRITERIA, 1, T1), (repeat, 0, T2)]);
        assert!(
            t.contains("T        pass") && t.contains("1 explained failure repeated"),
            "{t}"
        );
    }
}

#[test]
fn failures_with_and_without_a_criteria_variable_are_one_run_that_failed_twice() {
    // Each failure is followed by a pass of its own form; the run still failed twice.
    let t = row_t_after_explained(&[
        (WITH_CRITERIA, 1, T1),
        ("RUN=mixed SEED=1", 1, T2),
        (WITH_CRITERIA, 0, T3),
        ("RUN=mixed SEED=1", 0, "2026-10-01T13:00:00Z"),
    ]);
    assert!(
        t.contains("T        FAIL") && t.contains("failed more than once"),
        "{t}"
    );
}

#[test]
fn an_explanations_file_without_a_cause_is_refused() {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["explanations"] = json!("explained.json");
    rig.criteria(&criteria);
    let file = json!({"format": "heartwood-baseline-explanations/1",
        "failures": [{"record": "x.json", "cause": " ", "evidence": "e"}]});
    std::fs::write(rig.path("explained.json"), file.to_string()).expect("explanations");
    let ran = baseline(&rig, &[]);
    assert!(!ran.ok);
    assert!(
        ran.text.contains("a cause and its evidence are needed"),
        "{}",
        ran.text
    );
}

#[test]
fn a_scenario_need_reads_the_scenario_s_own_verdict_in_its_record() {
    let rig = Rig::new();
    rig.criteria(&with_row(json!([
        {"kind": "record", "target": "live-check", "contains": ["RUN=suites"],
         "without": ["SUITE"], "scenario": "chained-transfers"},
        {"kind": "record", "target": "live-check", "contains": ["RUN=suites"],
         "scenario": "double-spend"},
        {"kind": "record", "target": "live-check", "contains": ["RUN=suites"],
         "scenario": "propagation"}
    ])));
    let printed = "scenario compare: 9 records in 2 files\n  9 compared: 8 passed, 1 failed\n\
                   scenario transactions/chain: pass\n\
                   scenario transactions/chained-transfers: pass\n\
                   scenario transactions/double-spend: fail\n\
                   scenario transactions/steps: pass\n";
    assert!(record_of(&rig, "live-check", "SEED=1 RUN=suites", 1, printed, T1).ok);
    // A comparison of one suite alone names a SUITE: it counts for no need without one.
    let one = "scenario network/chain: pass\nscenario network/propagation: pass\n\
               scenario network/steps: pass\n";
    assert!(
        record_of(
            &rig,
            "live-check",
            "SUITE=network SEED=1 RUN=suites",
            0,
            one,
            T2
        )
        .ok
    );
    let text = baseline(&rig, &["--verbose"]).text;
    let t = row_t(&text);
    assert!(text.contains("ok      live-check [RUN=suites] without SUITE scenario chained-transfers: 1 passing record"), "{text}");
    assert!(
        text.contains("FAIL    live-check [RUN=suites] scenario double-spend: 1 failed record"),
        "{text}"
    );
    assert!(
        text.contains("ok      live-check [RUN=suites] scenario propagation: 1 passing record"),
        "{text}"
    );
    assert!(t.contains("T        FAIL"), "{t}");
}

#[test]
fn a_scenario_of_a_comparison_shows_nothing_unless_its_suite_s_chains_and_steps_held() {
    let rig = Rig::new();
    rig.criteria(&with_row(json!([
        {"kind": "record", "target": "live-check", "contains": ["RUN=suites"],
         "scenario": "peer-discovery"},
        {"kind": "record", "target": "live-check", "contains": ["RUN=suites"],
         "scenario": "burns"},
        {"kind": "record", "target": "live-check", "contains": ["RUN=suites"],
         "scenario": "restarts"}
    ])));
    // The network suite's chains failed: its scenarios fail with them. The transactions suite's
    // steps were not judged (no line): its scenario shows nothing. The lifecycle suite held.
    let printed = "scenario network/chain: fail\nscenario network/peer-discovery: pass\n\
                   scenario network/steps: pass\nscenario transactions/burns: pass\n\
                   scenario transactions/chain: pass\nscenario lifecycle/chain: pass\n\
                   scenario lifecycle/restarts: pass\nscenario lifecycle/steps: pass\n";
    assert!(record_of(&rig, "live-check", "SEED=1 RUN=suites", 1, printed, T1).ok);
    let text = baseline(&rig, &["--verbose"]).text;
    assert!(
        text.contains("FAIL    live-check [RUN=suites] scenario peer-discovery: 1 failed record"),
        "{text}"
    );
    assert!(
        text.contains("missing live-check [RUN=suites] scenario burns: 0 of 1 passing records"),
        "{text}"
    );
    assert!(
        text.contains("ok      live-check [RUN=suites] scenario restarts: 1 passing record"),
        "{text}"
    );
}

#[test]
fn a_record_counts_only_with_every_run_s_whole_tooling_in_versions_the_criteria_accept() {
    let rig = Rig::new();
    let sum = |c: char| c.to_string().repeat(64);
    let mut criteria = with_row(json!([{"kind": "record", "target": "suites"}]));
    criteria["tooling"] = json!({
        "accepted": {"scripts/suites.sh": [sum('b')], "scripts/lib.sh": [sum('d')]},
        "required": [{"target": "suites"}],
        "recipes": {"scripts/suites.sh": ["scripts/suites.sh", "scripts/lib.sh"]}
    });
    rig.criteria(&criteria);
    // Two runs of one seed, as a comparison of both implementations reads them.
    let seed = rig.path("seed");
    let (left, right) = (seed.join("x-reference"), seed.join("x-heartwood"));
    for run in [&left, &right] {
        std::fs::create_dir_all(run).expect("run");
        std::fs::write(run.join("run.json"), "{}").expect("run.json");
    }
    let list = |run: &Path, lines: &[(char, &str)]| {
        let text: String = lines
            .iter()
            .map(|(c, path)| format!("{}  {path}\n", sum(*c)))
            .collect();
        std::fs::write(run.join("tooling.sha256"), text).expect("tooling");
    };
    let n = std::cell::Cell::new(0);
    let record = |with_tooling: bool| -> String {
        n.set(n.get() + 1);
        let started = format!("2026-10-01T1{}:00:00Z", n.get());
        let _ = std::fs::remove_dir_all(rig.path("records"));
        let (out, dir) = (rig.arg("records"), rig.arg("seed"));
        let mut args = vec![
            "record",
            "--target",
            "suites",
            "--command",
            "suites.sh",
            "--exit",
            "0",
            "--started",
            &started,
            "--finished",
            &started,
            "--out",
            &out,
            "--machine",
            "box-1",
        ];
        if with_tooling {
            args.extend(["--tooling", &dir]);
        }
        assert!(rig.gate(&args).ok);
        row_t(&baseline(&rig, &["--verbose"]).text)
    };
    let whole = [('b', "scripts/suites.sh"), ('d', "scripts/lib.sh")];
    list(&left, &whole);
    list(&right, &whole);
    assert!(record(true).contains("T        pass"));
    // One run's list is not there: the other's cannot stand for it.
    std::fs::remove_file(right.join("tooling.sha256")).expect("remove");
    let t = record(true);
    assert!(
        t.contains("run seed/x-heartwood names none of its devnet tooling"),
        "{t}"
    );
    // A version the criteria do not accept.
    list(
        &right,
        &[('a', "scripts/suites.sh"), ('d', "scripts/lib.sh")],
    );
    let t = record(true);
    assert!(
        t.contains("run seed/x-heartwood was made with scripts/suites.sh aaaaaaaaaaaa"),
        "{t}"
    );
    // A file the recipe runs left out, and a file the criteria do not list.
    list(&right, &[('b', "scripts/suites.sh")]);
    let t = record(true);
    assert!(
        t.contains("does not name scripts/lib.sh, which that recipe runs"),
        "{t}"
    );
    list(
        &right,
        &[
            ('b', "scripts/suites.sh"),
            ('d', "scripts/lib.sh"),
            ('e', "scripts/new.sh"),
        ],
    );
    let t = record(true);
    assert!(
        t.contains("scripts/new.sh, which the criteria do not list"),
        "{t}"
    );
    // No recipe the criteria know, and no tooling at all.
    list(&right, &[('d', "scripts/lib.sh")]);
    assert!(record(true).contains("names no recipe the criteria know"));
    assert!(record(false).contains("names none of the devnet tooling"));
}

#[test]
fn a_change_to_the_build_of_a_tested_crate_makes_its_record_stale_and_another_crate_s_does_not() {
    let rig = Rig::new();
    let lock = |b_version: &str, c_version: &str| {
        format!(
            "version = 4\n\n[[package]]\nname = \"crate-a\"\nversion = \"0.1.0\"\ndependencies = [\n \"dep-b\",\n]\n\n\
             [[package]]\nname = \"dep-b\"\nversion = \"{b_version}\"\n\n\
             [[package]]\nname = \"crate-c\"\nversion = \"0.1.0\"\ndependencies = [\n \"dep-d\",\n]\n\n\
             [[package]]\nname = \"dep-d\"\nversion = \"{c_version}\"\n"
        )
    };
    rig.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"packages/*\"]\n\n[profile.release]\npanic = \"abort\"\n",
    );
    rig.write("Cargo.lock", &lock("1.0.0", "1.0.0"));
    rig.write(
        "packages/a/Cargo.toml",
        "[package]\nname = \"crate-a\"\n\n[dependencies]\ndep-b = \"1\"\n",
    );
    rig.write("packages/a/src/lib.rs", "pub fn a() {}\n");
    rig.write(
        "packages/c/Cargo.toml",
        "[package]\nname = \"crate-c\"\n\n[dependencies]\ndep-d = \"1\"\n",
    );
    rig.write("packages/c/src/lib.rs", "pub fn c() {}\n");
    rig.commit("crates");
    rig.criteria(&with_row(json!([{"kind": "record", "target": "lines"}])));
    let started = T1;
    assert!(
        rig.gate(&[
            "record",
            "--target",
            "lines",
            "--command",
            "make lines",
            "--exit",
            "0",
            "--started",
            started,
            "--finished",
            started,
            "--out",
            &rig.arg("records"),
            "--tested",
            "packages/a/src",
            "--machine",
            "box-1",
        ])
        .ok
    );
    let t = |rig: &Rig| row_t(&baseline(rig, &["--verbose"]).text);
    assert!(t(&rig).contains("T        pass"), "{}", t(&rig));
    // Another crate's dependency, a test target and a comment change nothing of crate-a's build.
    rig.write("Cargo.lock", &lock("1.0.0", "1.1.0"));
    rig.write("packages/a/Cargo.toml", "[package]\nname = \"crate-a\" # the crate\n\n[dependencies]\ndep-b = \"1\"\n\n[[test]]\nname = \"t\"\n");
    rig.commit("elsewhere");
    assert!(t(&rig).contains("T        pass"), "{}", t(&rig));
    // A version of crate-a's dependency does.
    rig.write("Cargo.lock", &lock("1.2.0", "1.1.0"));
    rig.commit("dependency");
    let now = t(&rig);
    assert!(now.contains("Cargo.lock: dep-b 1.0.0 -> 1.2.0"), "{now}");
    // So does the release profile.
    assert!(
        rig.gate(&[
            "record",
            "--target",
            "lines",
            "--command",
            "make lines",
            "--exit",
            "0",
            "--started",
            T2,
            "--finished",
            T2,
            "--out",
            &rig.arg("records"),
            "--tested",
            "packages/a/src",
            "--machine",
            "box-1",
        ])
        .ok
    );
    rig.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"packages/*\"]\n\n[profile.release]\npanic = \"unwind\"\n",
    );
    rig.commit("profile");
    let now = t(&rig);
    assert!(now.contains("[profile.release] changed"), "{now}");
}

#[test]
fn a_feature_of_a_crate_of_the_tree_that_a_tested_crate_uses_makes_its_record_stale() {
    let rig = Rig::new();
    rig.write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\n    \"packages/*\",\n]\n",
    );
    rig.write(
        "Cargo.lock",
        "version = 4\n\n[[package]]\nname = \"crate-a\"\nversion = \"0.1.0\"\ndependencies = [\n \"crate-b\",\n]\n\n\
         [[package]]\nname = \"crate-b\"\nversion = \"0.1.0\"\n\n\
         [[package]]\nname = \"crate-c\"\nversion = \"0.1.0\"\n",
    );
    rig.write(
        "packages/a/Cargo.toml",
        "[package]\nname = \"crate-a\"\n\n[dependencies]\ncrate-b = { path = \"../b\" }\n",
    );
    rig.write("packages/a/src/lib.rs", "pub fn a() {}\n");
    rig.write("packages/b/Cargo.toml", "[package]\nname = \"crate-b\"\n");
    rig.write("packages/b/src/lib.rs", "pub fn b() {}\n");
    rig.write("packages/c/Cargo.toml", "[package]\nname = \"crate-c\"\n");
    rig.write("packages/c/src/lib.rs", "pub fn c() {}\n");
    rig.commit("crates");
    rig.criteria(&with_row(json!([{"kind": "record", "target": "lines"}])));
    assert!(
        rig.gate(&[
            "record",
            "--target",
            "lines",
            "--command",
            "make lines",
            "--exit",
            "0",
            "--started",
            T1,
            "--finished",
            T1,
            "--out",
            &rig.arg("records"),
            "--tested",
            "packages/a/src",
            "--machine",
            "box-1",
        ])
        .ok
    );
    let t = |rig: &Rig| row_t(&baseline(rig, &["--verbose"]).text);
    // A crate the tested one does not use changes nothing.
    rig.write(
        "packages/c/Cargo.toml",
        "[package]\nname = \"crate-c\"\n\n[features]\nloud = []\n",
    );
    rig.commit("another crate's feature");
    assert!(t(&rig).contains("T        pass"), "{}", t(&rig));
    // A default feature of the crate it uses reaches its build, with the lockfile unchanged.
    rig.write(
        "packages/b/Cargo.toml",
        "[package]\nname = \"crate-b\"\n\n[features]\ndefault = [\"loud\"]\nloud = []\n",
    );
    rig.commit("a used crate's feature");
    let now = t(&rig);
    assert!(
        now.contains("packages/b/Cargo.toml (the build of a crate it uses)"),
        "{now}"
    );
}

#[test]
fn a_record_narrowed_to_a_module_goes_stale_on_what_its_code_reaches_only() {
    let rig = Rig::new();
    rig.write("tool/Cargo.toml", "[package]\nname = \"tool\"\n");
    rig.write(
        "tool/src/lib.rs",
        "pub mod cli;\npub mod run;\npub mod used;\npub mod other;\n",
    );
    rig.write(
        "tool/src/cli.rs",
        "pub struct Args;\nuse crate::other::all;\n",
    );
    rig.write(
        "tool/src/run/mod.rs",
        "mod step;\npub fn go() { step::one(); crate::used::help(); crate::cli::Args; }\n",
    );
    rig.write("tool/src/run/step.rs", "pub fn one() {}\n");
    rig.write("tool/src/used.rs", "pub fn help() {}\n");
    rig.write("tool/src/other.rs", "pub fn all() {}\n");
    rig.commit("a tool");
    let mut criteria = with_row(json!([{"kind": "record", "target": "suites"}]));
    criteria["narrowing"]["suites"] =
        json!(["rust:tool/src/run", "rust-leaf:tool/src/cli.rs", "data"]);
    rig.criteria(&criteria);
    assert!(
        rig.gate(&[
            "record",
            "--target",
            "suites",
            "--command",
            "suites.sh",
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
            "--machine",
            "box-1",
        ])
        .ok
    );
    assert_eq!(
        rig.records()[0]["testedPaths"],
        json!([
            "data",
            "tool/src/cli.rs",
            "tool/src/lib.rs",
            "tool/src/run/mod.rs",
            "tool/src/run/step.rs",
            "tool/src/used.rs",
            "tools/gate-record.sh"
        ])
    );
    let t = |rig: &Rig| row_t(&baseline(rig, &["--verbose"]).text);
    // A module the code does not reach (one the leaf alone names) changes nothing.
    rig.write("tool/src/other.rs", "pub fn all() { let _ = 1; }\n");
    rig.commit("another module");
    assert!(t(&rig).contains("T        pass"), "{}", t(&rig));
    // A module it reaches does.
    rig.write("tool/src/used.rs", "pub fn help() { let _ = 2; }\n");
    rig.commit("a used module");
    let now = t(&rig);
    assert!(now.contains("stale: tool/src/used.rs changed"), "{now}");
}

#[test]
fn a_record_narrowed_to_a_module_goes_stale_on_the_declaration_that_chooses_its_file() {
    let rig = Rig::new();
    rig.write("tool/Cargo.toml", "[package]\nname = \"tool\"\n");
    rig.write("tool/src/lib.rs", "pub mod a;\npub mod b;\n");
    rig.write("tool/src/a.rs", "pub fn f() { crate::b::c::answer(); }\n");
    rig.write("tool/src/b/mod.rs", "pub mod c;\n");
    rig.write("tool/src/b/c.rs", "pub fn answer() {}\n");
    rig.commit("a tool");
    let mut criteria = with_row(json!([{"kind": "record", "target": "suites"}]));
    criteria["narrowing"]["suites"] = json!(["rust:tool/src/a.rs"]);
    rig.criteria(&criteria);
    assert!(
        rig.gate(&[
            "record",
            "--target",
            "suites",
            "--command",
            "suites.sh",
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
            "--machine",
            "box-1",
        ])
        .ok
    );
    assert_eq!(
        rig.records()[0]["testedPaths"],
        json!([
            "tool/src/a.rs",
            "tool/src/b/c.rs",
            "tool/src/b/mod.rs",
            "tool/src/lib.rs",
            "tools/gate-record.sh"
        ])
    );
    let t = |rig: &Rig| row_t(&baseline(rig, &["--verbose"]).text);
    assert!(t(&rig).contains("T        pass"), "{}", t(&rig));
    // Only the parent's declaration changes: the path now compiles another file.
    rig.write("tool/src/b/other.rs", "pub fn answer() { let _ = 1; }\n");
    rig.write("tool/src/b/mod.rs", "#[path = \"other.rs\"]\npub mod c;\n");
    rig.commit("another file for c");
    let now = t(&rig);
    assert!(now.contains("stale: tool/src/b/mod.rs changed"), "{now}");
}

#[test]
fn a_run_stopped_on_purpose_counts_for_nothing_a_killed_one_fails_and_a_lenient_one_has_no_entry() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    let p1 = |rig: &Rig| -> String {
        let show = rig.gate(&[
            "ledger",
            "show",
            "--criteria",
            &rig.arg("criteria.json"),
            "--ledger",
            &rig.arg("ledger"),
        ]);
        show.text
    };
    // Its recorder was asked to stop it: no case counts, and nothing failed.
    let stopped = ledger_run_with(
        &rig,
        "ledger",
        "box-1",
        "kind=property id=P1 cases=600 seed=1",
        T1,
        ("difftest run l1 --property P1 --strict", 130),
        true,
    );
    assert!(stopped.ok, "{}", stopped.text);
    let lenient = ledger_run_as(
        &rig,
        "ledger",
        "box-1",
        "kind=property id=P1 cases=600 seed=2",
        T2,
        ("difftest run l1 --property P1", 0),
    );
    assert!(
        lenient.ok
            && lenient
                .text
                .contains("no ledger entry: the run was not strict"),
        "{}",
        lenient.text
    );
    let text = p1(&rig);
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("P1 "))
        .expect("P1 line");
    assert!(
        line.contains("0 cases") && line.contains("0 failed  1 stopped"),
        "{line}"
    );
    assert!(
        text.contains("stopped on purpose, no case counted"),
        "{text}"
    );
    // Killed by anything else (the out-of-memory killer's 137, an abort's 134): a failed run.
    let killed = ledger_run_as(
        &rig,
        "ledger",
        "box-1",
        "kind=property id=P1 cases=600 seed=3",
        T3,
        ("difftest run l1 --property P1 --strict", 137),
    );
    assert!(killed.ok, "{}", killed.text);
    let text = p1(&rig);
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("P1 "))
        .expect("P1 line");
    assert!(line.contains("1 failed  1 stopped"), "{line}");
}

#[test]
fn a_failed_long_run_is_superseded_only_when_explained_and_its_seed_passed_once_since() {
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["explanations"] = json!("explained.json");
    rig.criteria(&criteria);
    let explain = |records: &[&str]| {
        let failures: Vec<Value> = records
            .iter()
            .map(|r| json!({"record": r, "cause": "the runner ran out of memory", "evidence": "dmesg"}))
            .collect();
        let file = json!({"format": "heartwood-baseline-explanations/1", "failures": failures});
        std::fs::write(rig.path("explained.json"), file.to_string()).expect("explanations");
    };
    explain(&[]);
    let run = |seed: u32, exit: i32, started: &str| {
        let ran = ledger_run_as(
            &rig,
            "ledger",
            "box-1",
            &format!("kind=property id=P1 cases=600 seed={seed}"),
            started,
            ("difftest run l1 --property P1 --strict", exit),
        );
        assert!(ran.ok, "{}", ran.text);
    };
    let show = |rig: &Rig| -> String {
        rig.gate(&[
            "ledger",
            "show",
            "--criteria",
            &rig.arg("criteria.json"),
            "--ledger",
            &rig.arg("ledger"),
        ])
        .text
    };
    let line = |text: &str| -> String {
        text.lines()
            .find(|l| l.trim_start().starts_with("P1 "))
            .expect("P1 line")
            .to_owned()
    };
    run(7, 137, T1);
    run(7, 0, T2);
    // Unexplained, the failure stands beside the passing repeat.
    assert!(line(&show(&rig)).contains("1 failed"), "{}", show(&rig));
    let failed = format!("{}-l1-long.json", T1.replace(':', "-"));
    explain(&[&failed]);
    let text = show(&rig);
    assert!(
        line(&text).contains("600 cases") && line(&text).contains("0 failed"),
        "{text}"
    );
    assert!(
        text.contains("failed, explained and repeated: superseded"),
        "{text}"
    );
    // A second failure of the same seed is not superseded again.
    run(7, 1, T3);
    assert!(line(&show(&rig)).contains("2 failed"), "{}", show(&rig));
}

#[test]
fn a_strict_gate_passes_rows_that_wait_only_on_a_person_and_the_final_gate_fails_missing_evidence()
{
    let rig = Rig::new();
    let mut criteria = base_criteria();
    criteria["rows"] = json!([
        {"id": "R1", "criterion": "vectors", "requires": "pass", "shownBy": "l0",
         "needs": [{"kind": "record", "target": "l0"}]},
        {"id": "R2", "criterion": "sign-off", "requires": "a tick", "shownBy": "a person",
         "needs": [{"kind": "rows", "ids": ["R1"]}, {"kind": "manual", "what": "the tick"}]}
    ]);
    rig.criteria(&criteria);
    assert!(rig.record("l0", "difftest run l0", 0, "", T1).ok);
    let strict = baseline(&rig, &["--strict"]);
    assert!(strict.ok, "{}", strict.text);
    assert!(
        strict
            .text
            .contains("1 pending (1 waiting only on a person)"),
        "{}",
        strict.text
    );
    criteria["rows"].as_array_mut().expect("rows").push(json!(
        {"id": "R3", "criterion": "later", "requires": "live", "shownBy": "live",
         "waits": "a live run", "needs": [{"kind": "record", "target": "live"}]}));
    rig.criteria(&criteria);
    assert!(!baseline(&rig, &["--strict"]).ok);
    let last = baseline(&rig, &["--final"]);
    assert!(!last.ok);
    assert!(last.text.contains("R3       FAIL"), "{}", last.text);
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
    let ran = baseline(&rig, &[]);
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
    assert!(baseline(&rig, &[]).text.contains("R1       pass"));
    rig.write("docs/notes.md", "the guide changed\n");
    rig.commit("guide");
    let ran = baseline(&rig, &[]);
    assert!(ran.text.contains("R1       FAIL"), "{}", ran.text);
    assert!(ran.text.contains("docs/notes.md"), "{}", ran.text);
}

#[test]
fn a_register_run_fails_an_entry_whose_test_fails() {
    let rig = Rig::new();
    rig.write(
        "Cargo.toml",
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    rig.write("src/lib.rs", "");
    rig.write(".gitignore", "target/\n");
    rig.write(
        "tests/shows.rs",
        "#[test]\nfn it_passes() {}\n\n#[test]\nfn it_fails() {\n    panic!(\"on purpose\");\n}\n\n\
         #[test]\n#[ignore = \"integration\"]\nfn an_ignored_test_that_passes() {}\n",
    );
    let lock = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(rig.tree())
        .output()
        .expect("cargo runs");
    assert!(
        lock.status.success(),
        "{}",
        String::from_utf8_lossy(&lock.stderr)
    );
    rig.commit("a package");
    let test = |name: &str| json!([{"kind": "cargo", "package": "probe", "test": "shows", "file": "tests/shows.rs", "name": name}]);
    rig.criteria(&register_criteria(
        json!([
            entry("E-1", "when a", test("it_passes"), &[]),
            entry("E-2", "when b", test("it_fails"), &[]),
            entry("E-3", "when c", test("an_ignored_test_that_passes"), &[]),
        ]),
        json!([]),
    ));
    let ran = register(&rig, &["--run", "--strict"]);
    assert!(!ran.ok, "{}", ran.text);
    assert!(ran.text.contains("E-1  active   ok"), "{}", ran.text);
    assert!(ran.text.contains("E-2  active   FAIL"), "{}", ran.text);
    assert!(
        ran.text.contains("probe test it_fails did not pass"),
        "{}",
        ran.text
    );
    assert!(ran.text.contains("E-3  active   ok"), "{}", ran.text);
}

#[test]
fn a_merge_adds_no_run_twice_and_refuses_a_directory_without_entries() {
    let rig = Rig::new();
    rig.criteria(&base_criteria());
    assert!(
        ledger_run(
            &rig,
            "ledger",
            "box-1",
            "kind=target id=parse seconds=1800 forks=1",
            T1
        )
        .ok
    );
    // A server's directory that holds a copy of the ledger beside its own run.
    let copy = rig.path("server");
    std::fs::create_dir_all(&copy).expect("server");
    for file in std::fs::read_dir(rig.path("ledger")).expect("ledger") {
        let file = file.expect("entry").path();
        std::fs::copy(&file, copy.join(file.file_name().expect("name"))).expect("copy");
    }
    assert!(
        ledger_run(
            &rig,
            "server",
            "box-2",
            "kind=target id=parse seconds=1800 forks=1",
            T2
        )
        .ok
    );
    let merge = |from: &str| {
        rig.gate(&[
            "ledger",
            "merge",
            "--from",
            &rig.arg(from),
            "--machine",
            "box-2",
            "--ledger",
            &rig.arg("ledger"),
        ])
    };
    let merged = merge("server");
    assert!(merged.ok, "{}", merged.text);
    assert!(merged.text.contains("1 entries added"), "{}", merged.text);
    assert!(merged.text.contains("1 already there"), "{}", merged.text);
    let show = rig.gate(&[
        "ledger",
        "show",
        "--criteria",
        &rig.arg("criteria.json"),
        "--ledger",
        &rig.arg("ledger"),
    ]);
    let parse = show
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("parse"))
        .expect("parse line");
    assert!(parse.contains("1.00 hours"), "{parse}");
    std::fs::create_dir_all(rig.path("records-only")).expect("dir");
    let empty = merge("records-only");
    assert!(!empty.ok, "{}", empty.text);
    assert!(empty.text.contains("no ledger entries"), "{}", empty.text);
}
