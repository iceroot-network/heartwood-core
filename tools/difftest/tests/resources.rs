//! Resource budgets remain a separate command and can use explicitly accepted figures.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use heartwood_difftest::live::resources;
use heartwood_difftest::store::ScratchDir;
use serde_json::{Value, json};

fn recording() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live/churn")
}

fn report(baseline: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_difftest"));
    command
        .args(["live", "resources"])
        .arg(recording())
        .arg("--strict");
    if let Some(path) = baseline {
        command.arg("--baseline").arg(path);
    }
    command.output().unwrap()
}

fn accepted() -> Value {
    let mut figures = serde_json::Map::new();
    for n in 1..=6 {
        figures.insert(format!("relay.rss.p95.node{n}"), json!(200));
        figures.insert(format!("relay.rss.max.full-pool.node{n}"), json!(200));
        if n <= 5 {
            figures.insert(format!("forger.rss.p95.node{n}"), json!(40));
        }
    }
    figures.insert("nodes.rss.total".into(), json!(1000));
    figures.insert("disk.per-day".into(), json!(8000));
    json!({"format": "heartwood-resource-baseline/1", "decision": "accepted in the resource report", "figures": figures})
}

#[test]
fn missed_budgets_need_accepted_figures_with_no_more_than_a_quarter_growth() {
    let scratch = ScratchDir::new("resource-baseline").unwrap();
    let path = scratch.path().join("baseline.json");
    let output = report(None);
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    for key in [
        "relay.rss.p95.node1",
        "relay.rss.max.full-pool.node1",
        "forger.rss.p95.node1",
        "nodes.rss.total",
        "disk.per-day",
    ] {
        assert!(text.contains(&format!("[{key}]")), "{text}");
    }
    // The shared command renderer lists failures only; successful cases retain their keys.
    let cases = resources::report(&recording()).unwrap();
    assert!(
        cases
            .results
            .iter()
            .any(|r| r.label.contains("[cpu.quiet-windows]"))
    );
    let mut baseline = accepted();
    std::fs::write(&path, baseline.to_string()).unwrap();
    let output = report(Some(&path));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let cases = resources::report_with_baseline(&recording(), Some(&path)).unwrap();
    assert!(
        cases
            .results
            .iter()
            .any(|r| r.label.contains("within 25 % of accepted baseline"))
    );

    baseline["figures"]["disk.per-day"] = json!(5000);
    std::fs::write(&path, baseline.to_string()).unwrap();
    let output = report(Some(&path));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("more than 25 % above accepted baseline 5000")
    );

    baseline["figures"]
        .as_object_mut()
        .unwrap()
        .remove("disk.per-day");
    std::fs::write(&path, baseline.to_string()).unwrap();
    assert_eq!(report(Some(&path)).status.code(), Some(1));
}

#[test]
fn malformed_or_unreadable_baselines_are_usage_errors() {
    let scratch = ScratchDir::new("invalid-resource-baseline").unwrap();
    let path = scratch.path().join("baseline.json");
    assert_eq!(report(Some(&path)).status.code(), Some(2));
    for text in [
        "{".to_owned(),
        "null".to_owned(),
        json!({"format": "unknown", "decision": "accepted", "figures": {}}).to_string(),
    ] {
        std::fs::write(&path, text).unwrap();
        assert_eq!(report(Some(&path)).status.code(), Some(2));
    }
    for value in [json!(-1), json!("200"), Value::Null] {
        let mut baseline = accepted();
        baseline["figures"]["relay.rss.p95.node1"] = value;
        std::fs::write(&path, baseline.to_string()).unwrap();
        assert_eq!(report(Some(&path)).status.code(), Some(2));
    }
    let mut baseline = accepted();
    baseline["decision"] = json!("  ");
    std::fs::write(&path, baseline.to_string()).unwrap();
    assert_eq!(report(Some(&path)).status.code(), Some(2));
}
