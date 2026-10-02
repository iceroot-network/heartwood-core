//! The chains of a live run as corpora: node 1's archives (and, after a partition, the minority's
//! archive from before the heal) imported under the corpus root and judged by the oracle with its
//! processor results, as `make corpus-import corpus-judge PROCESSOR=1` does, so that
//! `make l2-processor` compares each block's result through the reference's job too. A corpus
//! already imported from the same archives and judged so, in the image of the oracle asked for
//! (by the image's id), is used as it is; one judged without the processor results, or by another
//! oracle, is judged again (a local oracle always judges again, since a judgement does not record
//! its scripts); one of an earlier run of the same name is moved aside to `<name>.replaced` and
//! imported again. Its manifest and `corpus.json` are copied into the run's
//! `corpus/<name>/`. A chain a Heartwood node recorded (`nodes/<node>`, the node's directory, as
//! the chain's archives) is imported from the node's block files through
//! [`crate::corpus_from_blocks`] instead.
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

/// Where the run's corpora go, relative to the current directory: the Makefile's `CORPUS`.
pub const CORPUS_ROOT: &str = "target/corpus";

/// A chain of the run, imported and judged.
#[derive(Debug, Clone)]
pub struct Judged {
    /// The corpus's directory.
    pub dir: PathBuf,
    /// Whether the judge's anchors hold.
    pub anchors_ok: bool,
    /// The anchors' results, for the report.
    pub anchors: Value,
    /// The anchors that do not apply to this corpus ([`corpus::absent_anchors`]).
    pub absent: Vec<&'static str>,
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

/// Whether the corpus in `dir` was judged by `oracle`: in the same image, by its id (the judge
/// records the image it ran in). A local oracle's judgement records no identity of the scripts it
/// ran, so it is never taken for this one's.
fn judged_by(dir: &Path, oracle: &Oracle) -> Result<bool> {
    let Some(id) = oracle.image_id()? else {
        return Ok(false);
    };
    let meta = read_json(&dir.join("corpus.json"))?;
    Ok(
        meta.pointer("/judge/oracle/mode").and_then(Value::as_str) == Some("container")
            && meta.pointer("/judge/oracle/image").and_then(Value::as_str) == Some(id.as_str()),
    )
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
    let source = run_dir.join(archives);
    // A Heartwood node's directory (`nodes/<node>`) holds its block files under `difftest/`; any
    // other names a directory of the devnet tooling's archives.
    let native = source.join("difftest").is_dir();
    let scratch = ScratchDir::new("live-block-import")?;
    let staged = scratch.path().join("corpus");
    let dirs = if native {
        let records = source.join("difftest");
        let mut files: Vec<(u32, PathBuf)> = std::fs::read_dir(&records)
            .map_err(|e| Error::io(&records, e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter_map(|p| {
                let n = p
                    .file_name()?
                    .to_str()?
                    .strip_prefix("blocks-")?
                    .strip_suffix(".bin")?
                    .parse()
                    .ok()?;
                Some((n, p))
            })
            .collect();
        files.sort();
        let files: Vec<PathBuf> = files.into_iter().map(|(_, p)| p).collect();
        if files.is_empty() {
            return Err(Error::Usage(format!(
                "{}: no block file of the chain {name}",
                records.display()
            )));
        }
        let crypto = run::crypto_dir(run_dir, &source);
        crate::corpus_from_blocks::from_blocks(&files, &crypto, seed, &staged)?;
        Vec::new()
    } else {
        let dirs = run::archives(&source)?;
        if dirs.is_empty() {
            return Err(Error::Usage(format!(
                "{}: no archive of the chain {name}",
                source.display()
            )));
        }
        dirs
    };
    let end = if native {
        let c = Corpus::load(&staged)?;
        let id = c
            .block_id(c.tip())
            .map(|id| id.to_hex())
            .ok_or_else(|| Error::Usage(format!("{name}: the imported chain has no tip")))?;
        let epoch = c.meta.pointer("/chain/epoch").and_then(Value::as_str);
        (u64::from(c.tip()), id, epoch.unwrap_or("").to_owned())
    } else {
        archive_end(&dirs)?
    };
    let dir = root.join(name);
    let mut imported = false;
    if dir.exists() && !same_chain(&dir, &end) {
        let aside = root.join(format!("{name}.replaced"));
        let _ = std::fs::remove_dir_all(&aside);
        std::fs::rename(&dir, &aside).map_err(|e| Error::io(&dir, e))?;
    }
    if !dir.exists() {
        std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
        if native {
            crate::corpus_from_blocks::copy_dir(&staged, &dir)?;
        } else {
            corpus::import(&dirs, &dir, seed)?;
        }
        adjust_plan(&dir)?;
        imported = true;
    }
    let mut judged = false;
    let judge_with = if !dir.join(TS_DIGESTS).exists() || !dir.join(TS_PROCESSOR).exists() {
        Some(oracle()?)
    } else {
        // A judgement is used again only by the oracle that made it; without an oracle to ask,
        // the recorded one is what there is.
        match oracle() {
            Ok(asked) if !judged_by(&dir, &asked)? => Some(asked),
            _ => None,
        }
    };
    if let Some(oracle) = judge_with {
        let work = ScratchDir::new("live-judge")?;
        oracle.judge(&dir, work.path(), &["--processor".to_owned()])?;
        corpus::record_judge(&dir, work.path())?;
        judged = true;
    }
    let (anchors_ok, anchors) = l2::judge_report(&dir)?;
    let loaded = Corpus::load(&dir)?;
    let absent = corpus::absent_anchors(&anchors, &loaded.meta);
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
        absent,
        chain: JudgedChain {
            name: name.to_owned(),
            forward,
        },
        tip: loaded.tip(),
        imported,
        judged,
    })
}
