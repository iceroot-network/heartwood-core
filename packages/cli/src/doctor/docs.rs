//! The check documentation, as data: one JSON file per area under `doctor/docs/`, compiled in,
//! and `common.json` with the shared advice templates. `heartwood doctor checks` and `explain`,
//! the MCP server's `list_checks` and `explain_check`, the renderer's advice and the documentation
//! site all read the same entries.
//!
//! An area file is `{"area": <name>, "checks": [<entry>...]}`; an entry holds `id`, `title`,
//! `area`, `subjects`, `sources`, `rules` (a text per status), `advice` (a template per key; a
//! template names its values as `{name}`), optionally `words` (the phrase an advice text shows for
//! a fixed word of a fact, which the JSON facts keep as the word) and `docs`, the link to its
//! page. `common.json` is `{"advice": {<key>: <template>}}`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde_json::Value;

use crate::doctor::registry::{Area, CheckId};

/// The documentation site's page of the doctor; a check's section is `#<id with - for .>`.
pub const DOCS_PAGE: &str = "https://docs.iceroot.com/validators/doctor/";

/// The area files, in [`Area::ALL`]'s order, and the shared templates.
const FILES: [(&str, &str); 14] = [
    ("process", include_str!("docs/process.json")),
    ("peers", include_str!("docs/peers.json")),
    ("sync", include_str!("docs/sync.json")),
    ("slots", include_str!("docs/slots.json")),
    ("forger", include_str!("docs/forger.json")),
    ("quorum", include_str!("docs/quorum.json")),
    ("net", include_str!("docs/net.json")),
    ("clock", include_str!("docs/clock.json")),
    ("disk", include_str!("docs/disk.json")),
    ("config", include_str!("docs/config.json")),
    ("version", include_str!("docs/version.json")),
    ("host", include_str!("docs/host.json")),
    ("onboarding", include_str!("docs/onboarding.json")),
    ("common", include_str!("docs/common.json")),
];

/// One check's documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDoc {
    /// The check's id.
    pub id: String,
    /// A short title.
    pub title: String,
    /// The area.
    pub area: String,
    /// What its findings are about.
    pub subjects: Vec<String>,
    /// What it reads.
    pub sources: Vec<String>,
    /// Its rule for each status it gives, by status name.
    pub rules: BTreeMap<String, String>,
    /// Its advice templates, by key.
    pub advice: BTreeMap<String, String>,
    /// The phrases its advice shows for the fixed words of its facts, by word.
    pub words: BTreeMap<String, String>,
    /// The link to its page.
    pub docs: String,
}

/// Every check's documentation and the shared templates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Docs {
    /// The entries, in the files' order.
    pub checks: Vec<CheckDoc>,
    /// The shared advice templates, by key.
    pub common: BTreeMap<String, String>,
}

/// Why the compiled-in documentation does not read (a defect of this build).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the check documentation {file}.json is not valid: {reason}")]
pub struct DocsError {
    /// The file.
    pub file: &'static str,
    /// What is wrong.
    pub reason: String,
}

static DOCS: LazyLock<Result<Docs, DocsError>> = LazyLock::new(parse);

/// The documentation.
pub fn docs() -> Result<&'static Docs, DocsError> {
    DOCS.as_ref().map_err(Clone::clone)
}

impl Docs {
    /// The entry of `id`.
    pub fn check(&self, id: CheckId) -> Option<&CheckDoc> {
        self.checks.iter().find(|doc| doc.id == id.as_str())
    }

    /// The advice template `key` of `id`: the check's own, else the shared one.
    pub fn template(&self, id: CheckId, key: &str) -> Option<&str> {
        let own = self.check(id).and_then(|doc| doc.advice.get(key));
        own.or_else(|| self.common.get(key)).map(String::as_str)
    }

    /// The phrase `id`'s advice shows for the fixed word `word`, when its entry gives one.
    pub fn word(&self, id: CheckId, word: &str) -> Option<&str> {
        self.check(id)?.words.get(word).map(String::as_str)
    }
}

fn parse() -> Result<Docs, DocsError> {
    let mut docs = Docs {
        checks: Vec::new(),
        common: BTreeMap::new(),
    };
    for (file, text) in FILES {
        let invalid = |reason: &str| DocsError {
            file,
            reason: reason.to_owned(),
        };
        let value: Value = serde_json::from_str(text).map_err(|e| invalid(&e.to_string()))?;
        if file == "common" {
            docs.common = texts(value.get("advice")).ok_or_else(|| invalid("advice"))?;
            continue;
        }
        let known = Area::ALL.iter().any(|area| area.name() == file);
        if !known || value.get("area").and_then(Value::as_str) != Some(file) {
            return Err(invalid("area"));
        }
        let entries = value.get("checks").and_then(Value::as_array);
        for entry in entries.ok_or_else(|| invalid("checks"))? {
            docs.checks
                .push(check(entry).ok_or_else(|| invalid("an entry"))?);
        }
    }
    Ok(docs)
}

fn check(entry: &Value) -> Option<CheckDoc> {
    let text = |key: &str| entry.get(key)?.as_str().map(str::to_owned);
    let list = |key: &str| -> Option<Vec<String>> {
        let items = entry.get(key)?.as_array()?;
        items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect()
    };
    Some(CheckDoc {
        id: text("id")?,
        title: text("title")?,
        area: text("area")?,
        subjects: list("subjects")?,
        sources: list("sources")?,
        rules: texts(entry.get("rules"))?,
        advice: texts(entry.get("advice"))?,
        words: entry
            .get("words")
            .map_or(Some(BTreeMap::new()), |w| texts(Some(w)))?,
        docs: text("docs")?,
    })
}

fn texts(value: Option<&Value>) -> Option<BTreeMap<String, String>> {
    let object = value?.as_object()?;
    let pairs = object
        .iter()
        .map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())));
    pairs.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::registry::ENTRIES;

    #[test]
    fn every_registry_entry_has_its_documentation_with_the_same_area_subjects_and_sources() {
        let docs = docs().unwrap();
        assert_eq!(docs.checks.len(), ENTRIES.len());
        for entry in &ENTRIES {
            let doc = docs
                .check(entry.check_id())
                .unwrap_or_else(|| panic!("{}", entry.id));
            assert_eq!(doc.area, entry.area.name(), "{}", entry.id);
            let subjects: Vec<&str> = entry.subjects.iter().map(|s| s.name()).collect();
            assert_eq!(doc.subjects, subjects, "{}", entry.id);
            let sources: Vec<&str> = entry.sources.iter().map(|s| s.name()).collect();
            assert_eq!(doc.sources, sources, "{}", entry.id);
            let anchor = entry.check_id().anchor();
            assert_eq!(doc.docs, format!("{DOCS_PAGE}#{anchor}"), "{}", entry.id);
        }
        for key in ["not-built", "unknown", "skipped-profile", "skipped-role"] {
            assert!(docs.common.contains_key(key), "{key}");
        }
    }
}
