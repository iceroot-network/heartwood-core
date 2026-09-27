//! The state digest files of L2 replays (`ts-digests.jsonl`, written by the oracle's replay) and
//! their comparison with Heartwood's digest lines.
//!
//! A line is `{h, step, id, rw?, b, s, r, m, w, sd, cd}` (see `heartwood_state::digest`). Lines
//! are compared as JSON values; at the first difference, [`section_differences`] turns both sides'
//! records of that step into a list of differences by section, wallet and field.

use std::collections::BTreeMap;
use std::path::Path;

use heartwood_state::digest::{DigestLine, Sections};
use serde_json::{Map, Value};

use crate::errors::{Error, Result};

/// The first line where two replays differ.
#[derive(Debug, Clone, PartialEq)]
pub struct LineDifference {
    /// The line's position in the files, from 1.
    pub line: usize,
    /// The oracle's line.
    pub oracle: Value,
    /// Heartwood's line.
    pub heartwood: Value,
}

/// The lines of the digest file at `path`.
pub fn read(path: &Path) -> Result<Vec<Value>> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|e| Error::Parse {
            path: path.to_path_buf(),
            line: index + 1,
            message: e.to_string(),
        })?;
        if !value.is_object() {
            return Err(Error::Parse {
                path: path.to_path_buf(),
                line: index + 1,
                message: "a digest line is a JSON object".into(),
            });
        }
        out.push(value);
    }
    Ok(out)
}

/// Heartwood's line as the JSON value the oracle's line is compared with.
pub fn line_value(line: &DigestLine) -> Value {
    serde_json::from_str(&line.to_json()).unwrap_or(Value::Null)
}

/// The first line of `heartwood` that differs from the same line of `oracle`, or where one side
/// has more lines.
pub fn first_difference(oracle: &[Value], heartwood: &[DigestLine]) -> Option<LineDifference> {
    let n = oracle.len().max(heartwood.len());
    (0..n).find_map(|i| {
        let ours = heartwood.get(i).map_or(Value::Null, line_value);
        let theirs = oracle.get(i).cloned().unwrap_or(Value::Null);
        (ours != theirs).then(|| LineDifference {
            line: i + 1,
            oracle: theirs,
            heartwood: ours,
        })
    })
}

/// The section hashes a revert line must share with the forward line of its height.
const SYMMETRIC: [&str; 4] = ["b", "s", "r", "w"];

/// Every revert line of `lines` that differs from the last forward line (apply or reapply) of
/// the same height in `b`, `s`, `r` or `w`, as a message.
pub fn revert_asymmetries(lines: &[Value]) -> Vec<String> {
    let mut forward: BTreeMap<u64, &Value> = BTreeMap::new();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let height = line.get("h").and_then(Value::as_u64).unwrap_or(0);
        match line.get("step").and_then(Value::as_str) {
            Some("genesis" | "apply" | "reapply") => {
                forward.insert(height, line);
            }
            Some("revert") => match forward.get(&height) {
                Some(before) => {
                    let differing: Vec<&str> = SYMMETRIC
                        .into_iter()
                        .filter(|k| before.get(*k) != line.get(*k))
                        .collect();
                    if !differing.is_empty() {
                        out.push(format!(
                            "line {}: the revert to height {height} differs from the forward line in {}",
                            i + 1,
                            differing.join(", ")
                        ));
                    }
                }
                None => out.push(format!(
                    "line {}: a revert to height {height}, which no forward line has",
                    i + 1
                )),
            },
            other => out.push(format!("line {}: unknown step {other:?}", i + 1)),
        }
    }
    out
}

/// The sections of `records` (an object with `B`, `S`, `R`, `M`, `W` texts), as Heartwood's type.
pub fn sections_from_json(records: &Value) -> Option<Sections> {
    let text = |k: &str| records.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(Sections {
        block: text("B")?,
        supply: text("S")?,
        round: text("R")?,
        missed: text("M")?,
        wallets: text("W")?,
    })
}

/// The sections of `sections`, as the oracle's dump writes them.
pub fn sections_to_json(sections: &Sections) -> Value {
    let mut out = Map::new();
    out.insert("B".into(), Value::String(sections.block.clone()));
    out.insert("S".into(), Value::String(sections.supply.clone()));
    out.insert("R".into(), Value::String(sections.round.clone()));
    out.insert("M".into(), Value::String(sections.missed.clone()));
    out.insert("W".into(), Value::String(sections.wallets.clone()));
    Value::Object(out)
}

fn records(section: &str) -> Vec<Value> {
    section
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap_or_else(|_| Value::String(l.to_owned())))
        .collect()
}

fn field_differences(prefix: &str, oracle: &Value, heartwood: &Value, out: &mut Vec<String>) {
    match (oracle, heartwood) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for k in keys {
                match (a.get(k), b.get(k)) {
                    (Some(x), Some(y)) if x == y => {}
                    (x, y) => out.push(format!(
                        "{prefix}.{k}: oracle {}, Heartwood {}",
                        x.map_or("absent".into(), Value::to_string),
                        y.map_or("absent".into(), Value::to_string)
                    )),
                }
            }
        }
        (a, b) if a != b => out.push(format!("{prefix}: oracle {a}, Heartwood {b}")),
        _ => {}
    }
}

/// The differences between the oracle's and Heartwood's records of one step: per section, and in
/// `#W` per wallet and field.
pub fn section_differences(oracle: &Sections, heartwood: &Sections) -> Vec<String> {
    let mut out = Vec::new();
    let pairs = [
        ("#B", &oracle.block, &heartwood.block),
        ("#S", &oracle.supply, &heartwood.supply),
        ("#R", &oracle.round, &heartwood.round),
        ("#M", &oracle.missed, &heartwood.missed),
    ];
    for (tag, a, b) in pairs {
        if a == b {
            continue;
        }
        let (ra, rb) = (records(a), records(b));
        if ra.len() != rb.len() {
            out.push(format!(
                "{tag}: the oracle has {} records, Heartwood {}",
                ra.len(),
                rb.len()
            ));
        }
        for (i, (x, y)) in ra.iter().zip(&rb).enumerate() {
            field_differences(&format!("{tag}[{i}]"), x, y, &mut out);
        }
    }
    if oracle.wallets != heartwood.wallets {
        let by_address = |section: &str| -> BTreeMap<String, Value> {
            records(section)
                .into_iter()
                .map(|r| {
                    let a = r.get("a").and_then(Value::as_str).unwrap_or("?").to_owned();
                    (a, r)
                })
                .collect()
        };
        let (wa, wb) = (by_address(&oracle.wallets), by_address(&heartwood.wallets));
        let addresses: std::collections::BTreeSet<&String> = wa.keys().chain(wb.keys()).collect();
        for a in addresses {
            match (wa.get(a), wb.get(a)) {
                (Some(x), Some(y)) => field_differences(&format!("#W {a}"), x, y, &mut out),
                (Some(_), None) => out.push(format!("#W {a}: only the oracle lists it")),
                (None, Some(_)) => out.push(format!("#W {a}: only Heartwood lists it")),
                (None, None) => {}
            }
        }
        if out.is_empty() {
            out.push("#W: the texts differ in order or formatting only".into());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::vectors::testutil::TestDir;

    fn line(h: u64, step: &str, w: &str) -> Value {
        json!({"h": h, "step": step, "id": "x", "b": "b", "s": "s", "r": "r", "m": "m", "w": w, "sd": "sd", "cd": "cd"})
    }

    #[test]
    fn reads_digest_files() {
        let dir = TestDir::new("digests-read");
        let path = dir.path().join("ts-digests.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n\n{}\n",
                line(1, "genesis", "w1"),
                line(2, "apply", "w2")
            ),
        )
        .unwrap();
        let lines = read(&path).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["h"], json!(2));
        std::fs::write(&path, "{\"h\":1}\n[1]\n").unwrap();
        assert!(matches!(read(&path), Err(Error::Parse { line: 2, .. })));
    }

    #[test]
    fn reverts_must_equal_the_forward_line() {
        let ok = vec![
            line(1, "genesis", "w1"),
            line(2, "apply", "w2"),
            line(3, "apply", "w3"),
            line(2, "revert", "w2"),
            line(3, "reapply", "w3"),
        ];
        assert!(revert_asymmetries(&ok).is_empty());
        let mut bad = ok.clone();
        bad[3] = line(2, "revert", "other");
        let messages = revert_asymmetries(&bad);
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("line 4") && messages[0].contains('w'),
            "{messages:?}"
        );
        assert_eq!(revert_asymmetries(&[line(5, "revert", "w")]).len(), 1);
    }

    #[test]
    fn section_differences_name_wallets_and_fields() {
        let oracle = Sections {
            block: "#B\n{\"h\":2}\n".into(),
            supply: "#S\n{\"issued\":\"1\",\"supply\":\"1\"}\n".into(),
            round: "#R\n".into(),
            missed: "#M\n".into(),
            wallets: "#W\n{\"a\":\"dA\",\"bal\":\"5\",\"n\":\"1\"}\n{\"a\":\"dB\",\"bal\":\"1\",\"n\":\"0\"}\n".into(),
        };
        let mut heartwood = oracle.clone();
        assert!(section_differences(&oracle, &heartwood).is_empty());
        heartwood.wallets = "#W\n{\"a\":\"dA\",\"bal\":\"6\",\"n\":\"1\"}\n{\"a\":\"dC\",\"bal\":\"1\",\"n\":\"0\"}\n".into();
        heartwood.block = "#B\n{\"h\":3}\n".into();
        let d = section_differences(&oracle, &heartwood);
        assert!(
            d.contains(&"#B[0].h: oracle 2, Heartwood 3".to_string()),
            "{d:?}"
        );
        assert!(
            d.contains(&"#W dA.bal: oracle \"5\", Heartwood \"6\"".to_string()),
            "{d:?}"
        );
        assert!(
            d.contains(&"#W dB: only the oracle lists it".to_string()),
            "{d:?}"
        );
        assert!(
            d.contains(&"#W dC: only Heartwood lists it".to_string()),
            "{d:?}"
        );
        let json = sections_to_json(&oracle);
        assert_eq!(sections_from_json(&json), Some(oracle));
    }
}
