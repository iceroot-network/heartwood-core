//! Golden-vector files: JSONL per vector class, a meta record first, and a SHA-256 manifest.
//!
//! The oracle's `gen-vectors.js` writes them. A file looks like this:
//!
//! ```text
//! {"op":"meta","format":"heartwood-vectors/1","class":"V02-transactions","records":16,"solar":{...},...}
//! {"op":"tx.build","network":"devnet","height":2,"name":"transfer-1","input":{...},"output":{...}}
//! {"op":"tx.fromBytes","network":"devnet","height":2,"input":{...},"error":{"class":"...","message":"..."}}
//! ```
//!
//! Records hold only values the reference computed. What legitimately differs between runs (the
//! bcrypto backend, the image id, the Node version) is in the meta record, so two runs of the same
//! oracle must agree on every record line.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::errors::{Error, Result};

/// The format string every meta record carries.
pub const FORMAT: &str = "heartwood-vectors/1";
/// The manifest file name, in `sha256sum` format.
pub const MANIFEST: &str = "MANIFEST.sha256";
/// The extension of vector files.
pub const EXTENSION: &str = "jsonl";

/// The first record of every vector file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub format: String,
    pub class: String,
    pub records: usize,
    pub solar: SolarInfo,
    pub oracle: OracleInfo,
    pub node: String,
    pub icu: Option<String>,
    pub bcrypto: String,
    pub network: Value,
    /// Anything else the generator recorded (generator name, aux, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Where the reference came from.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolarInfo {
    pub version: String,
    pub commit: Option<String>,
    pub tree: Option<String>,
    pub dirty: Option<bool>,
    pub source: String,
    pub series_sha256: Option<String>,
}

/// How the oracle ran: `container` (with the image id) or `local`.
#[derive(Debug, Clone, Deserialize)]
pub struct OracleInfo {
    pub mode: String,
    pub image: Option<String>,
}

/// An error the reference raised for a record's input. Messages are never compared.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorRecord {
    pub class: String,
    pub message: String,
}

/// What the reference produced for a record: an output value or an error.
#[derive(Debug, Clone)]
pub enum Expected {
    Output(Value),
    Error(ErrorRecord),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    op: String,
    network: String,
    height: u64,
    #[serde(default)]
    name: Option<String>,
    input: Value,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    error: Option<ErrorRecord>,
}

/// One golden-vector record.
#[derive(Debug, Clone)]
pub struct Record {
    /// The oracle operation, for example `tx.build` (tools/oracle/lib/ops.js).
    pub op: String,
    /// The network configuration the record was produced under (the meta record describes it).
    pub network: String,
    /// The rule height the reference was set to.
    pub height: u64,
    /// An optional label, for example `transfer-256`.
    pub name: Option<String>,
    pub input: Value,
    /// The reference's output or error.
    pub expected: Expected,
    /// 1-based line number in its file.
    pub line: usize,
    /// The whole record as parsed, for regeneration diffs.
    pub raw: Value,
}

impl Record {
    /// `op "name"` or just `op`, for reports.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{} \"{}\"", self.op, name),
            None => self.op.clone(),
        }
    }

    fn parse(value: Value, path: &Path, line: usize) -> Result<Record> {
        let parse_error = |message: String| Error::Parse {
            path: path.to_path_buf(),
            line,
            message,
        };
        let raw: RawRecord =
            serde_json::from_value(value.clone()).map_err(|e| parse_error(e.to_string()))?;
        if raw.op == "meta" {
            return Err(parse_error("a second meta record".into()));
        }
        let expected = match (raw.output, raw.error) {
            (Some(output), None) => Expected::Output(output),
            (None, Some(error)) => Expected::Error(error),
            _ => {
                return Err(parse_error(
                    "a record needs exactly one of output and error".into(),
                ));
            }
        };
        Ok(Record {
            op: raw.op,
            network: raw.network,
            height: raw.height,
            name: raw.name,
            input: raw.input,
            expected,
            line,
            raw: value,
        })
    }
}

/// One vector file: its meta record and its records.
#[derive(Debug, Clone)]
pub struct VectorFile {
    pub path: PathBuf,
    pub meta: Meta,
    pub records: Vec<Record>,
}

impl VectorFile {
    /// The file name without its directory.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn load(path: &Path) -> Result<VectorFile> {
        let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty());
        let parse_error = |line: usize, message: String| Error::Parse {
            path: path.to_path_buf(),
            line,
            message,
        };

        let (index, first) = lines.next().ok_or_else(|| Error::Vectors {
            path: path.to_path_buf(),
            message: "empty file".into(),
        })?;
        let first: Value =
            serde_json::from_str(first).map_err(|e| parse_error(index + 1, e.to_string()))?;
        if first.get("op").and_then(Value::as_str) != Some("meta") {
            return Err(parse_error(
                index + 1,
                "the first record must be the meta record".into(),
            ));
        }
        let meta: Meta = serde_json::from_value(first)
            .map_err(|e| parse_error(index + 1, format!("meta record: {e}")))?;
        if meta.format != FORMAT {
            return Err(parse_error(
                index + 1,
                format!("format {} is not {FORMAT}", meta.format),
            ));
        }

        let mut records = Vec::new();
        for (index, line) in lines {
            let value: Value =
                serde_json::from_str(line).map_err(|e| parse_error(index + 1, e.to_string()))?;
            records.push(Record::parse(value, path, index + 1)?);
        }
        if records.len() != meta.records {
            return Err(Error::Vectors {
                path: path.to_path_buf(),
                message: format!(
                    "the meta record announces {} records, the file has {}",
                    meta.records,
                    records.len()
                ),
            });
        }
        Ok(VectorFile {
            path: path.to_path_buf(),
            meta,
            records,
        })
    }
}

/// A directory of vector files, checked against its manifest.
#[derive(Debug, Clone)]
pub struct VectorSet {
    pub dir: PathBuf,
    pub files: Vec<VectorFile>,
}

impl VectorSet {
    /// Verify the manifest, then load every vector file it lists, in name order.
    pub fn load(dir: &Path) -> Result<VectorSet> {
        let names = verify_manifest(dir)?;
        let files = names
            .iter()
            .map(|name| VectorFile::load(&dir.join(name)))
            .collect::<Result<Vec<_>>>()?;
        Ok(VectorSet {
            dir: dir.to_path_buf(),
            files,
        })
    }

    pub fn record_count(&self) -> usize {
        self.files.iter().map(|f| f.records.len()).sum()
    }

    /// The meta record of the first file: every file of one run shares the provenance fields.
    pub fn meta(&self) -> Option<&Meta> {
        self.files.first().map(|f| &f.meta)
    }
}

/// SHA-256 of a byte string, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn vector_files_in(dir: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(EXTENSION) {
            names.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(names)
}

/// Check `MANIFEST.sha256`: every listed file exists and has its hash, and every vector file in
/// the directory is listed. Returns the listed vector-file names in order.
pub fn verify_manifest(dir: &Path) -> Result<Vec<String>> {
    let manifest_path = dir.join(MANIFEST);
    let text = fs::read_to_string(&manifest_path).map_err(|e| Error::io(&manifest_path, e))?;
    let mut problems = Vec::new();
    let mut listed = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((hash, name)) = line.split_once("  ") else {
            problems.push(format!("line {}: not `<sha256>  <file>`", index + 1));
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || name.contains('/') {
            problems.push(format!("line {}: not `<sha256>  <file>`", index + 1));
            continue;
        }
        match fs::read(dir.join(name)) {
            Ok(bytes) => {
                let actual = sha256_hex(&bytes);
                if actual != hash.to_ascii_lowercase() {
                    problems.push(format!(
                        "{name}: SHA-256 is {actual}, the manifest says {hash}"
                    ));
                }
            }
            Err(e) => problems.push(format!("{name}: {e}")),
        }
        listed.push(name.to_string());
    }
    let listed_set: BTreeSet<String> = listed.iter().cloned().collect();
    for name in vector_files_in(dir)?.difference(&listed_set) {
        problems.push(format!("{name}: not listed in {MANIFEST}"));
    }
    if listed.is_empty() {
        problems.push(format!("{MANIFEST} lists no files"));
    }
    if !problems.is_empty() {
        return Err(Error::Manifest {
            dir: dir.to_path_buf(),
            problems,
        });
    }
    listed.sort();
    Ok(listed
        .into_iter()
        .filter(|n| n.ends_with(&format!(".{EXTENSION}")))
        .collect())
}

/// Write `MANIFEST.sha256` for every vector file in `dir`, in name order.
pub fn write_manifest(dir: &Path) -> Result<()> {
    let mut out = String::new();
    for name in vector_files_in(dir)? {
        let path = dir.join(&name);
        let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        out.push_str(&format!("{}  {name}\n", sha256_hex(&bytes)));
    }
    let manifest = dir.join(MANIFEST);
    fs::write(&manifest, out).map_err(|e| Error::io(&manifest, e))
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};

    /// A temporary directory, removed when dropped.
    pub struct TestDir(PathBuf);

    impl TestDir {
        pub fn new(name: &str) -> TestDir {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "heartwood-difftest-{}-{n}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TestDir(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn meta(class: &str, records: usize, backend: &str) -> Value {
        json!({
            "op": "meta", "format": super::FORMAT, "class": class, "records": records,
            "generator": "test",
            "solar": {"version": "4.3.1", "commit": "b45369d7da143ab246f2d19b28ed9fb77fc99f36",
                      "tree": null, "dirty": false, "source": "git", "seriesSha256": null},
            "oracle": {"mode": "local", "image": null},
            "node": "v18.20.8", "icu": "74.2", "bcrypto": backend,
            "network": {"name": "devnet", "pubKeyHash": 90}
        })
    }

    /// Write one vector file (meta record first) and refresh the manifest.
    pub fn write_file(dir: &Path, class: &str, records: &[Value], backend: &str) {
        let mut body = meta(class, records.len(), backend).to_string();
        body.push('\n');
        for r in records {
            body.push_str(&r.to_string());
            body.push('\n');
        }
        std::fs::write(dir.join(format!("{class}.jsonl")), body).unwrap();
        super::write_manifest(dir).unwrap();
    }

    pub fn keys_record(passphrase: &str, public_key: &str) -> Value {
        json!({"op": "keys.fromPassphrase", "network": "devnet", "height": 2,
               "input": {"passphrase": passphrase},
               "output": {"privateKey": "00", "publicKey": public_key}})
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::testutil::{TestDir, keys_record, write_file};
    use super::*;

    #[test]
    fn loads_a_valid_set() {
        let dir = TestDir::new("valid");
        write_file(
            dir.path(),
            "V01-keys-addresses",
            &[keys_record("a", "02aa")],
            "native",
        );
        write_file(
            dir.path(),
            "V03-removed-types",
            &[
                json!({"op": "tx.fromBytes", "network": "devnet", "height": 2, "input": {"hex": "ff"},
                     "error": {"class": "InvalidTransactionBytesError", "message": "short"}}),
            ],
            "native",
        );
        let set = VectorSet::load(dir.path()).unwrap();
        assert_eq!(set.files.len(), 2);
        assert_eq!(set.record_count(), 2);
        assert_eq!(set.files[0].file_name(), "V01-keys-addresses.jsonl");
        assert_eq!(set.files[0].records[0].line, 2);
        assert!(
            matches!(&set.files[1].records[0].expected, Expected::Error(e) if e.class == "InvalidTransactionBytesError")
        );
        assert_eq!(set.meta().unwrap().solar.version, "4.3.1");
    }

    #[test]
    fn manifest_detects_a_changed_file() {
        let dir = TestDir::new("tampered");
        write_file(
            dir.path(),
            "V01-keys-addresses",
            &[keys_record("a", "02aa")],
            "native",
        );
        let path = dir.path().join("V01-keys-addresses.jsonl");
        let text = fs::read_to_string(&path).unwrap().replace("02aa", "02ab");
        fs::write(&path, text).unwrap();
        let err = VectorSet::load(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains("V01-keys-addresses.jsonl: SHA-256 is"),
            "{err}"
        );
    }

    #[test]
    fn manifest_detects_an_unlisted_file() {
        let dir = TestDir::new("unlisted");
        write_file(
            dir.path(),
            "V01-keys-addresses",
            &[keys_record("a", "02aa")],
            "native",
        );
        fs::write(dir.path().join("V99-extra.jsonl"), "{}\n").unwrap();
        let err = verify_manifest(dir.path()).unwrap_err().to_string();
        assert!(err.contains("V99-extra.jsonl: not listed"), "{err}");
    }

    #[test]
    fn manifest_detects_a_missing_file() {
        let dir = TestDir::new("missing");
        write_file(
            dir.path(),
            "V01-keys-addresses",
            &[keys_record("a", "02aa")],
            "native",
        );
        fs::remove_file(dir.path().join("V01-keys-addresses.jsonl")).unwrap();
        let err = verify_manifest(dir.path()).unwrap_err().to_string();
        assert!(err.contains("V01-keys-addresses.jsonl:"), "{err}");
    }

    #[test]
    fn rejects_a_record_with_output_and_error() {
        let dir = TestDir::new("both");
        let bad = json!({"op": "x", "network": "devnet", "height": 1, "input": {},
                         "output": {}, "error": {"class": "E", "message": "m"}});
        write_file(dir.path(), "V01-keys-addresses", &[bad], "native");
        let err = VectorSet::load(dir.path()).unwrap_err().to_string();
        assert!(
            err.contains(":2: a record needs exactly one of output and error"),
            "{err}"
        );
    }

    #[test]
    fn rejects_a_wrong_record_count_and_format() {
        let dir = TestDir::new("count");
        write_file(
            dir.path(),
            "V01-keys-addresses",
            &[keys_record("a", "02aa")],
            "native",
        );
        let path = dir.path().join("V01-keys-addresses.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("\"records\":1", "\"records\":2")).unwrap();
        write_manifest(dir.path()).unwrap();
        let err = VectorSet::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("announces 2 records, the file has 1"), "{err}");

        fs::write(&path, text.replace(FORMAT, "heartwood-vectors/0")).unwrap();
        write_manifest(dir.path()).unwrap();
        let err = VectorSet::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("format heartwood-vectors/0"), "{err}");
    }
}
