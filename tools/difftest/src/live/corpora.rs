//! The chains of a live run as corpora: node 1's archives (and, after a partition, the minority's
//! archive from before the heal) imported under the corpus root and judged by the oracle with its
//! processor results, as `make corpus-import corpus-judge PROCESSOR=1` does, so that
//! `make l2-processor` compares each block's result through the reference's job too. A corpus
//! already imported from the same archives and judged so is used as it is; one judged without the
//! processor results is judged again; one of an earlier run of the same name is moved aside to
//! `<name>.replaced` and imported again. Its manifest and `corpus.json` are copied into the run's
//! `corpus/<name>/`.
//!
//! One change to an imported corpus: a random revert of its plan that would return to the last
//! block of a round reverts one block more. The judge's headless node then deletes the next round's
//! rows, as a reference node that reverts to a round's last block does, and does not save them
//! again when it applies the next blocks, while the live node, which never reverted, holds them: the
//! round rows' anchor would fail on the plan, not on the chain. The steps changed are listed in
//! `corpus.json` (`revertPlanAdjusted`).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::corpus::{self, Corpus, RevertPlan, TS_DIGESTS, TS_PROCESSOR};
use crate::errors::{Error, Result};
use crate::levels::l2;
use crate::live::digest::JudgedChain;
use crate::live::run::{self, read_json};
use crate::oracle::Oracle;
use crate::store::{self, ScratchDir};

/// A chain of the run, imported and judged.
#[derive(Debug, Clone)]
pub struct Judged {
    /// The corpus's directory.
    pub dir: PathBuf,
    /// Whether the judge's anchors hold.
    pub anchors_ok: bool,
    /// The anchors' results, for the report.
    pub anchors: Value,
    /// The judge's forward lines.
    pub chain: JudgedChain,
    /// The corpus's tip.
    pub tip: u32,
    /// Whether this check imported it, and whether it judged it.
    pub imported: bool,
    /// Whether this check ran the judge.
    pub judged: bool,
}

/// The last block (height and id) the archives in `dirs` end with, and their chain's epoch.
fn archive_end(dirs: &[PathBuf]) -> Result<(u64, String, String)> {
    let last = dirs
        .last()
        .ok_or_else(|| Error::Usage("no archive to import".into()))?;
    let m = read_json(&last.join("manifest.json"))?;
    let to = m.pointer("/range/to").and_then(Value::as_u64).unwrap_or(0);
    let id = m
        .pointer("/range/lastId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let epoch = m
        .pointer("/chain/epoch")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Ok((to, id, epoch))
}

/// Whether the corpus in `dir` was imported from archives that end at `end`.
fn same_chain(dir: &Path, end: &(u64, String, String)) -> bool {
    let Ok(meta) = read_json(&dir.join("corpus.json")) else {
        return false;
    };
    let Ok(loaded) = Corpus::load(dir) else {
        return false;
    };
    meta.get("tip").and_then(Value::as_u64) == Some(end.0)
        && loaded
            .block_id(loaded.tip())
            .is_some_and(|id| id.to_hex() == end.1)
        && meta.pointer("/chain/epoch").and_then(Value::as_str) == Some(end.2.as_str())
}

/// Move each revert of the plan of the corpus in `dir` that returns to a round's last block one
/// block further back, record the changed steps in `corpus.json`, and write the manifest again.
fn adjust_plan(dir: &Path) -> Result<()> {
    let path = dir.join("revert-plan.json");
    let mut plan = RevertPlan::from_json(&read_json(&path)?, &path)?;
    let length = plan.round_length.max(1);
    let mut changed = Vec::new();
    for step in &mut plan.steps {
        let to = step.after_height - step.blocks;
        if to >= length && to % length == 0 {
            let moved = if step.blocks + 1 < step.after_height {
                step.blocks + 1
            } else {
                step.blocks - 1
            };
            changed.push(serde_json::json!({"after": step.after_height, "blocks": step.blocks, "now": moved}));
            step.blocks = moved;
        }
    }
    if changed.is_empty() {
        return Ok(());
    }
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&plan.to_json()).unwrap_or_default()
        ),
    )
    .map_err(|e| Error::io(&path, e))?;
    let meta_path = dir.join("corpus.json");
    let mut meta = read_json(&meta_path)?;
    meta["revertPlanAdjusted"] = Value::Array(changed);
    std::fs::write(
        &meta_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&meta).unwrap_or_default()
        ),
    )
    .map_err(|e| Error::io(&meta_path, e))?;
    corpus::write_manifest(dir)
}

/// Import (when needed) and judge (when needed) the corpus `name` of the run in `run_dir` from its
/// archive directory `archives` (relative to the run), under `root`. `oracle` gives the oracle
/// when a judge must run.
pub fn prepare(
    run_dir: &Path,
    name: &str,
    archives: &str,
    seed: &str,
    root: &Path,
    oracle: &dyn Fn() -> Result<Oracle>,
) -> Result<Judged> {
    let dirs = run::archives(&run_dir.join(archives))?;
    if dirs.is_empty() {
        return Err(Error::Usage(format!(
            "{}: no archive of the chain {name}",
            run_dir.join(archives).display()
        )));
    }
    let end = archive_end(&dirs)?;
    let dir = root.join(name);
    let mut imported = false;
    if dir.exists() && !same_chain(&dir, &end) {
        let aside = root.join(format!("{name}.replaced"));
        let _ = std::fs::remove_dir_all(&aside);
        std::fs::rename(&dir, &aside).map_err(|e| Error::io(&dir, e))?;
    }
    if !dir.exists() {
        std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
        corpus::import(&dirs, &dir, seed)?;
        adjust_plan(&dir)?;
        imported = true;
    }
    let mut judged = false;
    if !dir.join(TS_DIGESTS).exists() || !dir.join(TS_PROCESSOR).exists() {
        let oracle = oracle()?;
        let work = ScratchDir::new("live-judge")?;
        oracle.judge(&dir, work.path(), &["--processor".to_owned()])?;
        corpus::record_judge(&dir, work.path())?;
        judged = true;
    }
    let (anchors_ok, anchors) = l2::judge_report(&dir)?;
    let loaded = Corpus::load(&dir)?;
    let forward = store::forward_lines(&loaded)?;
    let copy = run_dir.join("corpus").join(name);
    std::fs::create_dir_all(&copy).map_err(|e| Error::io(&copy, e))?;
    for file in [corpus::MANIFEST, "corpus.json"] {
        std::fs::copy(dir.join(file), copy.join(file)).map_err(|e| Error::io(dir.join(file), e))?;
    }
    Ok(Judged {
        dir,
        anchors_ok,
        anchors,
        chain: JudgedChain {
            name: name.to_owned(),
            forward,
        },
        tip: loaded.tip(),
        imported,
        judged,
    })
}
