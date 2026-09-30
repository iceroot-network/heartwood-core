//! Reading the gate records and the ledger entries from their directories, and the path rule's
//! verdict on each.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use super::criteria::Criteria;
use super::git::{PathRule, Repo, Validity};
use super::ledger::{LEDGER_FORMAT, LedgerEntry, LedgerKind};
use super::record::{GateRecord, RECORD_FORMAT};
use crate::errors::{Error, Result};

/// A record or an entry, where it was read, and whether it counts at the final head.
#[derive(Debug, Clone)]
pub struct Item<T> {
    /// The file.
    pub path: PathBuf,
    /// Its content.
    pub value: T,
    /// The path rule's verdict.
    pub validity: Validity,
}

/// Every record and entry the directories hold, judged against one final head.
#[derive(Debug)]
pub struct Evidence {
    /// The final head and the rule.
    pub rule: PathRule,
    /// The gate records.
    pub records: Vec<Item<GateRecord>>,
    /// The ledger entries.
    pub entries: Vec<Item<LedgerEntry>>,
}

/// The places evidence is read from and the tree it is judged in.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// Directories of gate records (searched recursively).
    pub records: Vec<PathBuf>,
    /// Directories of ledger entries (searched recursively).
    pub ledger: Vec<PathBuf>,
    /// The tree's root.
    pub repo: PathBuf,
    /// The final head, as a revision.
    pub head: String,
    /// Whether records of a dirty tree count.
    pub allow_dirty: bool,
}

impl Evidence {
    /// Read the records and entries of `sources` and judge each.
    pub fn load(criteria: &Criteria, sources: &Sources) -> Result<Evidence> {
        let rule = PathRule::new(
            Repo::open(&sources.repo),
            &sources.head,
            sources.allow_dirty,
        )?;
        let mut records = Vec::new();
        for (path, value) in load_all::<GateRecord>(&sources.records, RECORD_FORMAT)? {
            let validity = rule.check(&value.head, value.dirty, &value.tested_paths)?;
            records.push(Item {
                path,
                value,
                validity,
            });
        }
        let mut entries = Vec::new();
        for (path, value) in load_all::<LedgerEntry>(&sources.ledger, LEDGER_FORMAT)? {
            let paths = entry_paths(criteria, &value);
            let validity = rule.check(&value.head, false, &paths)?;
            entries.push(Item {
                path,
                value,
                validity,
            });
        }
        Ok(Evidence {
            rule,
            records,
            entries,
        })
    }
}

/// The paths an entry's run tested: its property's or target's in the criteria (the whole tree
/// when the criteria do not list it).
fn entry_paths(criteria: &Criteria, entry: &LedgerEntry) -> Vec<String> {
    match entry.kind {
        LedgerKind::Property => criteria
            .properties
            .get(&entry.id)
            .map(|p| p.paths.clone())
            .unwrap_or_default(),
        LedgerKind::Target => criteria
            .targets
            .get(&entry.id)
            .map(|t| t.paths.clone())
            .unwrap_or_default(),
    }
}

/// Every JSON file under `dirs` (recursively, symbolic links not followed) whose `format` is
/// `format`, parsed strictly and sorted by path. Other JSON files are not this store's and are
/// skipped; a file of the format that does not parse is an error. A directory that is not there
/// holds nothing.
pub fn load_all<T: DeserializeOwned>(dirs: &[PathBuf], format: &str) -> Result<Vec<(PathBuf, T)>> {
    let mut files = Vec::new();
    for dir in dirs {
        // A directory nothing has been written to yet holds nothing.
        if dir.is_dir() {
            walk(dir, &mut files)?;
        }
    }
    files.sort();
    files.dedup();
    let mut found = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value.get("format").and_then(|f| f.as_str()) != Some(format) {
            continue;
        }
        let parsed = serde_json::from_value(value).map_err(|e| Error::Corpus {
            path: path.clone(),
            message: e.to_string(),
        })?;
        found.push((path, parsed));
    }
    Ok(found)
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let listing = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    for item in listing {
        let item = item.map_err(|e| Error::io(dir, e))?;
        let path = item.path();
        let kind = item.file_type().map_err(|e| Error::io(&path, e))?;
        if kind.is_dir() {
            walk(&path, files)?;
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "json") {
            files.push(path);
        }
    }
    Ok(())
}

/// The file name of a ledger entry: its start, kind, id, seed and machine.
pub fn entry_file_name(entry: &LedgerEntry) -> String {
    let safe = |text: &str| -> String {
        text.chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' => c,
                _ => '-',
            })
            .collect()
    };
    let kind = match entry.kind {
        LedgerKind::Property => "property",
        LedgerKind::Target => "target",
    };
    let seed = entry
        .seed
        .as_deref()
        .map(|s| format!("-{}", safe(s)))
        .unwrap_or_default();
    format!(
        "{}-{kind}-{}{seed}-{}.json",
        safe(&entry.started),
        safe(&entry.id),
        safe(&entry.machine)
    )
}

/// Write `entry` into `dir` under its file name; its path, and whether it is new. An identical
/// file already there is left alone; a different one is an error, since two runs never share a
/// name.
pub fn put_entry(entry: &LedgerEntry, dir: &Path) -> Result<(PathBuf, bool)> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let path = dir.join(entry_file_name(entry));
    let text = serde_json::to_string_pretty(entry).map_err(|e| Error::Corpus {
        path: path.clone(),
        message: e.to_string(),
    })? + "\n";
    match std::fs::read_to_string(&path) {
        Ok(existing) if existing == text => Ok((path, false)),
        Ok(_) => Err(Error::Corpus {
            path,
            message: "a different ledger entry with this name exists".to_owned(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
            Ok((path, true))
        }
        Err(e) => Err(Error::io(&path, e)),
    }
}

/// Write `entry` into `dir`; its path.
pub fn write_entry(entry: &LedgerEntry, dir: &Path) -> Result<PathBuf> {
    put_entry(entry, dir).map(|(path, _)| path)
}
