//! The chains of a live run as corpora: node 1's archives (and, after a partition, the minority's
//! archive from before the heal) imported under the corpus root and judged by the oracle, as
//! `make corpus-import corpus-judge` does. A corpus already imported from the same archives and
//! judged is used as it is; one of an earlier run of the same name is moved aside to
//! `<name>.replaced` and imported again. Its manifest and `corpus.json` are copied into the run's
//! `corpus/<name>/`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::corpus::{self, Corpus, TS_DIGESTS};
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
        imported = true;
    }
    let mut judged = false;
    if !dir.join(TS_DIGESTS).exists() {
        let oracle = oracle()?;
        let work = ScratchDir::new("live-judge")?;
        oracle.judge(&dir, work.path(), &[])?;
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
