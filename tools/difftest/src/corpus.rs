//! The chains that L2 replays.
//!
//! A corpus directory holds a chain produced by a reference devnet, in Heartwood's format and with
//! no machine path in it:
//!
//! - `crypto/`: `network.json`, `milestones.json`, `genesisBlock.json`, `exceptions.json`;
//! - `blocks.jsonl`: one `{h, id, hex}` per block after the genesis block, `hex` the full block
//!   bytes (header, signature, transaction lengths and transactions);
//! - `reference/`: the devnet node's round rows, missed slots, block metadata and API checkpoints,
//!   which anchor the oracle's replay to the node;
//! - `revert-plan.json`: the reverts both sides run;
//! - `restart-plan.json` and `ts-restarts.jsonl`, when the corpus is judged with restarts: the
//!   restarts both sides run and the reference's restart lines ([`crate::restart`]);
//! - `reference/restarts.jsonl`, when the node restarted while it made the chain: each live
//!   restart's height and path and the file of the state right after it;
//! - `ts-digests.jsonl` and `anchors.json`: the oracle's digest lines and anchor results;
//! - `ts-processor.jsonl`, when the corpus is judged with `--processor`: each block's result
//!   through the reference's job, one block per job;
//! - `corpus.json`: the reference commit and tree, the oracle image, heights and counts;
//! - `MANIFEST.sha256`: the SHA-256 of every other file.
//!
//! [`import`] builds a corpus from the devnet tooling's archives (format [`ARCHIVE_FORMAT`]):
//! one or more increments of the same chain, each continuing the one before it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use heartwood_crypto::blocks::factory;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockId, Milestones, Network};
use serde_json::{Map, Value, json};

use crate::errors::{Error, Result};
use crate::restart::{self, LiveRestart, RestartPlan};
use crate::vectors::sha256_hex;

/// The format string of `corpus.json`.
pub const FORMAT: &str = "heartwood-corpus/1";
/// The format string of `revert-plan.json`.
pub const PLAN_FORMAT: &str = "heartwood-revert-plan/1";
/// The archive format of the devnet tooling that [`import`] reads.
pub const ARCHIVE_FORMAT: &str = "solar-devnet-archive/1";
/// The manifest file of a corpus, in `sha256sum` format with paths relative to the corpus.
pub const MANIFEST: &str = "MANIFEST.sha256";
/// The oracle's digest lines, written by the judge.
pub const TS_DIGESTS: &str = "ts-digests.jsonl";
/// The judge's processor results, written with `--processor`: each block's result through the
/// reference's job with one block per job, beside its digest line.
pub const TS_PROCESSOR: &str = "ts-processor.jsonl";
/// The judge's anchor results.
pub const ANCHORS: &str = "anchors.json";
/// The chain definition files, in `crypto/`.
pub const CRYPTO_FILES: [&str; 4] = ["network", "milestones", "genesisBlock", "exceptions"];

/// A corpus, loaded.
#[derive(Debug, Clone)]
pub struct Corpus {
    /// The corpus directory.
    pub dir: PathBuf,
    /// The network description.
    pub network: Network,
    /// The milestones.
    pub milestones: Milestones,
    /// The genesis block.
    pub genesis: Block,
    /// The blocks after the genesis block, in height order.
    pub blocks: Vec<CorpusBlock>,
    /// The reverts to run.
    pub revert_plan: RevertPlan,
    /// The restarts to run, when the corpus has a restart plan.
    pub restart_plan: Option<RestartPlan>,
    /// The node's live restarts.
    pub live_restarts: Vec<LiveRestart>,
    /// `corpus.json`.
    pub meta: Value,
    /// The height the corpus is cut at, when it is ([`Corpus::cut`]).
    pub cut: Option<u32>,
}

/// One block of a corpus, as the devnet node stored it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusBlock {
    /// The height.
    pub height: u32,
    /// The id.
    pub id: BlockId,
    /// The full block bytes.
    pub bytes: Vec<u8>,
}

/// The reverts of a replay: after the block at `after_height` is applied, revert `blocks` blocks,
/// then apply them again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevertPlan {
    /// The seed the random reverts were drawn with.
    pub seed: u64,
    /// The round length the round ends were taken from.
    pub round_length: u32,
    /// The last height of the chain the plan was made for.
    pub tip: u32,
    /// The reverts, in height order.
    pub steps: Vec<RevertStep>,
}

/// One revert of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevertStep {
    /// The height after which the revert runs.
    pub after_height: u32,
    /// The number of blocks reverted, then applied again.
    pub blocks: u32,
}

/// Random reverts per plan.
const RANDOM_REVERTS: u32 = 20;
/// The largest random revert.
const MAX_RANDOM_REVERT: u32 = 102;
/// The long revert at every tenth round end: one round and one block.
const LONG_REVERT: u32 = 54;

/// SplitMix64: a small, fixed generator, so a plan depends on its seed only.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` (`bound` > 0), without modulo bias.
    fn below(&mut self, bound: u32) -> u32 {
        let bound = u64::from(bound);
        let limit = u64::MAX - u64::MAX % bound;
        loop {
            let r = self.next();
            if r < limit {
                return u32::try_from(r % bound).unwrap_or(0);
            }
        }
    }
}

impl RevertPlan {
    /// The plan for a chain of `tip` blocks with rounds of `round_length` blocks, from `seed`:
    /// one block at every round end, 2 and a round and one block at every tenth round end, and 1
    /// to 102 blocks at 20 random heights. No step reverts the genesis block.
    pub fn generate(seed: u64, tip: u32, round_length: u32) -> RevertPlan {
        let mut steps = Vec::new();
        if round_length > 0 && tip >= 2 {
            let mut end = round_length;
            let mut round = 1u32;
            while end <= tip {
                if end >= 2 {
                    steps.push(RevertStep {
                        after_height: end,
                        blocks: 1,
                    });
                }
                if round.is_multiple_of(10) {
                    for blocks in [2, LONG_REVERT] {
                        if blocks < end {
                            steps.push(RevertStep {
                                after_height: end,
                                blocks,
                            });
                        }
                    }
                }
                end = end.saturating_add(round_length);
                round += 1;
                if end == u32::MAX {
                    break;
                }
            }
            let mut rng = SplitMix64(seed);
            for _ in 0..RANDOM_REVERTS {
                let after_height = 2 + rng.below(tip - 1);
                let most = MAX_RANDOM_REVERT.min(after_height - 1);
                let blocks = 1 + rng.below(most);
                steps.push(RevertStep {
                    after_height,
                    blocks,
                });
            }
        }
        steps.sort_by_key(|s| s.after_height);
        RevertPlan {
            seed,
            round_length,
            tip,
            steps,
        }
    }

    /// The plan as `revert-plan.json` holds it.
    pub fn to_json(&self) -> Value {
        json!({
            "format": PLAN_FORMAT,
            "seed": self.seed.to_string(),
            "roundLength": self.round_length,
            "tip": self.tip,
            "steps": self.steps.iter()
                .map(|s| json!({"after": s.after_height, "blocks": s.blocks}))
                .collect::<Vec<_>>(),
        })
    }

    /// The plan in `value`, the JSON of the file `path` (a corpus's `revert-plan.json`);
    /// [`Error::Corpus`] when it breaks the plan's format.
    pub fn from_json(value: &Value, path: &Path) -> Result<RevertPlan> {
        RevertPlan::parse(value).map_err(|message| corpus_error(path, message))
    }

    fn parse(value: &Value) -> std::result::Result<RevertPlan, String> {
        if value.get("format").and_then(Value::as_str) != Some(PLAN_FORMAT) {
            return Err(format!("not a {PLAN_FORMAT} file"));
        }
        let number = |v: Option<&Value>, what: &str| {
            v.and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("{what} is not a height"))
        };
        let seed = value
            .get("seed")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("seed is not a decimal u64")?;
        let mut steps = Vec::new();
        for step in value
            .get("steps")
            .and_then(Value::as_array)
            .ok_or("no steps")?
        {
            let step = RevertStep {
                after_height: number(step.get("after"), "after")?,
                blocks: number(step.get("blocks"), "blocks")?,
            };
            if step.blocks == 0 || step.blocks >= step.after_height {
                return Err(format!(
                    "the step after {} reverts {} blocks",
                    step.after_height, step.blocks
                ));
            }
            steps.push(step);
        }
        if steps
            .windows(2)
            .any(|w| w[0].after_height > w[1].after_height)
        {
            return Err("the steps are not in height order".into());
        }
        Ok(RevertPlan {
            seed,
            round_length: number(value.get("roundLength"), "roundLength")?,
            tip: number(value.get("tip"), "tip")?,
            steps,
        })
    }

    /// The steps that run after the block at `height`, in plan order.
    pub fn after(&self, height: u32) -> impl Iterator<Item = &RevertStep> {
        self.steps.iter().filter(move |s| s.after_height == height)
    }
}

fn corpus_error(path: impl Into<PathBuf>, message: impl Into<String>) -> Error {
    Error::Corpus {
        path: path.into(),
        message: message.into(),
    }
}

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_str(&read_text(path)?)
        .map_err(|e| corpus_error(path, format!("not JSON: {e}")))
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (index, line) in read_text(path)?.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line).map_err(|e| Error::Parse {
            path: path.to_path_buf(),
            line: index + 1,
            message: e.to_string(),
        })?);
    }
    Ok(out)
}

fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    fs::write(path, text).map_err(|e| Error::io(path, e))
}

fn jsonl(values: &[Value]) -> String {
    values.iter().map(|v| format!("{v}\n")).collect()
}

/// Every file under `dir` except the manifest, as `/`-separated relative paths in byte order.
fn files_under(dir: &Path) -> Result<Vec<String>> {
    fn walk(root: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
        let here = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        for entry in fs::read_dir(&here).map_err(|e| Error::io(&here, e))? {
            let entry = entry.map_err(|e| Error::io(&here, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let kind = entry.file_type().map_err(|e| Error::io(entry.path(), e))?;
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if path != MANIFEST {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    out.sort();
    Ok(out)
}

/// Write `MANIFEST.sha256` for every file of the corpus in `dir`.
pub fn write_manifest(dir: &Path) -> Result<()> {
    let mut text = String::new();
    for rel in files_under(dir)? {
        let path = dir.join(&rel);
        let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        text.push_str(&format!("{}  {rel}\n", sha256_hex(&bytes)));
    }
    write_file(&dir.join(MANIFEST), &text)
}

/// Check the manifest of the corpus in `dir`: every listed file has its hash, and every file is
/// listed.
pub fn verify(dir: &Path) -> Result<()> {
    let manifest = dir.join(MANIFEST);
    let mut problems = Vec::new();
    let mut listed = Vec::new();
    for (index, line) in read_text(&manifest)?.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((hash, rel)) = line.split_once("  ") else {
            problems.push(format!("line {}: not `<sha256>  <path>`", index + 1));
            continue;
        };
        if rel.starts_with('/') || rel.split('/').any(|part| part == "..") {
            problems.push(format!(
                "line {}: {rel} is not inside the corpus",
                index + 1
            ));
            continue;
        }
        match fs::read(dir.join(rel)) {
            Ok(bytes) if sha256_hex(&bytes) == hash => {}
            Ok(_) => problems.push(format!("{rel}: SHA-256 differs from the manifest")),
            Err(e) => problems.push(format!("{rel}: {e}")),
        }
        listed.push(rel.to_owned());
    }
    for rel in files_under(dir)? {
        if !listed.contains(&rel) {
            problems.push(format!("{rel}: not listed in {MANIFEST}"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Manifest {
            dir: dir.to_path_buf(),
            problems,
        })
    }
}

impl Corpus {
    /// Load the corpus in `dir`, checking its manifest.
    pub fn load(dir: &Path) -> Result<Corpus> {
        verify(dir)?;
        let meta = read_json(&dir.join("corpus.json"))?;
        if meta.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err(corpus_error(
                dir.join("corpus.json"),
                format!("not a {FORMAT} corpus"),
            ));
        }
        let crypto = dir.join("crypto");
        let network = Network::load(&read_text(&crypto.join("network.json"))?)
            .map_err(|e| corpus_error(crypto.join("network.json"), e.to_string()))?;
        let milestones =
            Milestones::load(&read_text(&crypto.join("milestones.json"))?, &network)
                .map_err(|e| corpus_error(crypto.join("milestones.json"), e.to_string()))?;
        let genesis =
            factory::from_json(&read_json(&crypto.join("genesisBlock.json"))?, &milestones)
                .map_err(|e| corpus_error(crypto.join("genesisBlock.json"), e.to_string()))?;
        let blocks_path = dir.join("blocks.jsonl");
        let mut blocks = Vec::new();
        for (index, value) in read_jsonl(&blocks_path)?.iter().enumerate() {
            let bad = |message: &str| Error::Parse {
                path: blocks_path.clone(),
                line: index + 1,
                message: message.to_owned(),
            };
            let height = value
                .get("h")
                .and_then(Value::as_u64)
                .and_then(|h| u32::try_from(h).ok())
                .ok_or_else(|| bad("no height"))?;
            let expected = u32::try_from(index + 2).map_err(|_| bad("too many blocks"))?;
            if height != expected {
                return Err(bad(&format!("height {height}, expected {expected}")));
            }
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .and_then(|t| BlockId::from_hex(t).ok())
                .ok_or_else(|| bad("no block id"))?;
            let bytes = value
                .get("hex")
                .and_then(Value::as_str)
                .and_then(|t| hex::decode(t).ok())
                .ok_or_else(|| bad("no block bytes"))?;
            blocks.push(CorpusBlock { height, id, bytes });
        }
        let plan = dir.join("revert-plan.json");
        let revert_plan = RevertPlan::from_json(&read_json(&plan)?, &plan)?;
        let restart_path = dir.join(restart::RESTART_PLAN);
        let restart_plan = if restart_path.exists() {
            Some(RestartPlan::from_json(
                &read_json(&restart_path)?,
                &restart_path,
            )?)
        } else {
            None
        };
        Ok(Corpus {
            dir: dir.to_path_buf(),
            network,
            milestones,
            genesis,
            blocks,
            revert_plan,
            restart_plan,
            live_restarts: restart::read_live(dir)?,
            meta,
            cut: None,
        })
    }

    /// The height the judge cut the corpus at when it recorded its lines, if it did.
    pub fn judged_to(&self) -> Option<u32> {
        self.meta
            .pointer("/judge/to")
            .and_then(Value::as_u64)
            .and_then(|h| u32::try_from(h).ok())
    }

    /// The corpus cut at height `to`: its blocks up to `to`, the reverts that run after them, and
    /// the live restarts up to it. The restart plan is kept only when it was made for `to`.
    pub fn cut(mut self, to: u32) -> Result<Corpus> {
        if to < 1 || to > self.tip() {
            return Err(corpus_error(
                &self.dir,
                format!("cannot cut the chain of tip {} at {to}", self.tip()),
            ));
        }
        self.blocks.retain(|b| b.height <= to);
        self.revert_plan.steps.retain(|s| s.after_height <= to);
        self.live_restarts.retain(|l| l.height <= to);
        if self.restart_plan.as_ref().is_some_and(|p| p.tip != to) {
            self.restart_plan = None;
        }
        self.cut = Some(to);
        Ok(self)
    }

    /// The corpus as the judge recorded it: cut at `to` when given, else at the height the judge
    /// cut it at. Refuses a `to` that is not the judge's.
    pub fn as_judged(self, to: Option<u32>) -> Result<Corpus> {
        match (to, self.judged_to()) {
            (Some(to), Some(judged)) if to != judged => Err(corpus_error(
                &self.dir,
                format!(
                    "the judge's lines are for the chain cut at {judged}, not {to}: judge it again with TO={to}"
                ),
            )),
            (Some(to), None) if to != self.tip() => Err(corpus_error(
                &self.dir,
                format!(
                    "the judge's lines are for the whole chain, not cut at {to}: judge it again with TO={to}"
                ),
            )),
            (_, Some(judged)) => self.cut(judged),
            (Some(to), None) => self.cut(to),
            (None, None) => Ok(self),
        }
    }

    /// The id of the block at `height` (1 to the tip).
    pub fn block_id(&self, height: u32) -> Option<BlockId> {
        if height == 1 {
            Some(*self.genesis.id())
        } else {
            self.block(height).map(|b| b.id)
        }
    }

    /// The height of the last block.
    pub fn tip(&self) -> u32 {
        self.blocks.last().map_or(1, |b| b.height)
    }

    /// The block at `height` (2 to the tip).
    pub fn block(&self, height: u32) -> Option<&CorpusBlock> {
        let index = usize::try_from(height.checked_sub(2)?).ok()?;
        self.blocks.get(index)
    }

    /// The corpus's name: its directory's name.
    pub fn name(&self) -> String {
        self.dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }
}

/// One archive of the devnet tooling, read for import.
struct Archive {
    dir: PathBuf,
    manifest: Value,
}

impl Archive {
    fn open(dir: &Path) -> Result<Archive> {
        let manifest = read_json(&dir.join("manifest.json"))?;
        if manifest.get("format").and_then(Value::as_str) != Some(ARCHIVE_FORMAT) {
            return Err(corpus_error(dir, format!("not a {ARCHIVE_FORMAT} archive")));
        }
        let problems = manifest
            .get("problems")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if problems > 0 {
            return Err(corpus_error(
                dir,
                format!("the archive records {problems} problems"),
            ));
        }
        let files = manifest
            .get("files")
            .and_then(Value::as_object)
            .ok_or_else(|| corpus_error(dir, "the manifest lists no files"))?;
        for (rel, hash) in files {
            let path = dir.join(rel);
            let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
            if Some(sha256_hex(&bytes).as_str()) != hash.as_str() {
                return Err(corpus_error(
                    path,
                    "SHA-256 differs from the archive manifest",
                ));
            }
        }
        let verify = read_json(&dir.join("verify.json"))?;
        if verify.get("ok") != Some(&Value::Bool(true)) {
            return Err(corpus_error(
                dir.join("verify.json"),
                "the archive's strict decode did not pass",
            ));
        }
        Ok(Archive {
            dir: dir.to_path_buf(),
            manifest,
        })
    }

    fn range(&self, key: &str) -> Option<&Value> {
        self.manifest.get("range").and_then(|r| r.get(key))
    }

    fn height(&self, key: &str) -> Result<u32> {
        self.range(key)
            .and_then(Value::as_u64)
            .and_then(|h| u32::try_from(h).ok())
            .ok_or_else(|| corpus_error(&self.dir, format!("the manifest has no range.{key}")))
    }

    fn files(&self, prefix: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .manifest
            .get("files")
            .and_then(Value::as_object)
            .map(|f| {
                f.keys()
                    .filter(|k| k.starts_with(prefix))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }
}

/// The full block bytes of an archive's block line: the header with its signature, the
/// transaction lengths as u32 little-endian, then the transactions.
fn block_bytes(line: &Value) -> std::result::Result<Vec<u8>, String> {
    let header = line
        .get("header")
        .and_then(Value::as_str)
        .and_then(|h| hex::decode(h).ok())
        .ok_or("no header")?;
    let mut transactions = Vec::new();
    for t in line
        .get("transactions")
        .and_then(Value::as_array)
        .ok_or("no transactions")?
    {
        transactions.push(
            t.as_str()
                .and_then(|h| hex::decode(h).ok())
                .ok_or("a transaction is not hex")?,
        );
    }
    let mut out = header;
    for t in &transactions {
        let length = u32::try_from(t.len()).map_err(|_| "a transaction is too long")?;
        out.extend_from_slice(&length.to_le_bytes());
    }
    for t in &transactions {
        out.extend_from_slice(t);
    }
    Ok(out)
}

/// The block id of a header: the SHA-256 of the header with its signature.
fn header_id(line: &Value) -> Option<String> {
    let header = hex::decode(line.get("header")?.as_str()?).ok()?;
    Some(sha256_hex(&header))
}

/// Check that `seed` derives the generator of the genesis block in `crypto` (the four crypto
/// files, in [`CRYPTO_FILES`] order).
fn check_seed(dir: &Path, crypto: &[Vec<u8>], seed: &str) -> Result<()> {
    let parse = |i: usize| -> Option<Value> { serde_json::from_slice(crypto.get(i)?).ok() };
    let network = parse(0)
        .and_then(|n| n.get("pubKeyHash").and_then(Value::as_u64))
        .and_then(|b| u8::try_from(b).ok())
        .ok_or_else(|| corpus_error(dir, "crypto/network.json has no pubKeyHash"))?;
    let generator = parse(2)
        .and_then(|g| {
            g.get("generatorPublicKey")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| corpus_error(dir, "crypto/genesisBlock.json has no generatorPublicKey"))?;
    let derived = heartwood_crypto::genesis::derive::role_key(
        crate::synthetic::DERIVATION_TAG,
        "generator",
        seed,
        network,
    )?;
    if derived.keys.public_key().to_hex() != generator {
        return Err(corpus_error(
            dir,
            "the seed does not derive the genesis block's generator (give the chain's --seed)",
        ));
    }
    Ok(())
}

/// Add the judge's output in `work` (`ts-digests.jsonl`, `anchors.json`, and `ts-restarts.jsonl`
/// when it ran restarts) to the corpus in `dir`: copy them, remove a restart file the judge did
/// not write again, record the judge's provenance (and the height it cut the chain at) in
/// `corpus.json`, and write the manifest again. Returns whether the judge's anchors hold.
pub fn record_judge(dir: &Path, work: &Path) -> Result<bool> {
    let mut files = Vec::new();
    let optional = [restart::TS_RESTARTS, TS_PROCESSOR];
    for name in [TS_DIGESTS, ANCHORS, restart::TS_RESTARTS, TS_PROCESSOR] {
        let path = work.join(name);
        if optional.contains(&name) && !path.exists() {
            continue;
        }
        let text = read_text(&path)?;
        check_no_machine_paths(name, &text)?;
        files.push((name, text));
    }
    let anchors: Value = serde_json::from_str(&files[1].1)
        .map_err(|e| corpus_error(work.join(ANCHORS), e.to_string()))?;
    let ok = anchors.get("ok") == Some(&Value::Bool(true));
    for (name, text) in &files {
        write_file(&dir.join(name), text)?;
    }
    for name in optional {
        let stale = dir.join(name);
        if !work.join(name).exists() && stale.exists() {
            fs::remove_file(&stale).map_err(|e| Error::io(&stale, e))?;
        }
    }
    let meta_path = dir.join("corpus.json");
    let mut meta = read_json(&meta_path)?;
    meta["judge"] = json!({
        "ok": ok,
        "reference": anchors.get("reference").cloned().unwrap_or(Value::Null),
        "oracle": anchors.get("oracle").cloned().unwrap_or(Value::Null),
        "lines": anchors.get("lines").cloned().unwrap_or(Value::Null),
        "finalChainDigest": anchors.get("finalChainDigest").cloned().unwrap_or(Value::Null),
    });
    if let Some(to) = anchors.get("to") {
        meta["judge"]["to"] = to.clone();
    }
    if let Some(restarts) = anchors.get("restarts") {
        meta["judge"]["restarts"] = json!({
            "ok": restarts.get("ok").cloned().unwrap_or(Value::Null),
            "plan": restarts.get("plan").cloned().unwrap_or(Value::Null),
            "finalChainDigests": restarts.get("modes").and_then(Value::as_object).map(|modes| {
                modes
                    .iter()
                    .map(|(mode, m)| (mode.clone(), m.get("finalChainDigest").cloned().unwrap_or(Value::Null)))
                    .collect::<Map<String, Value>>()
            }),
        });
    }
    write_file(
        &meta_path,
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&meta).unwrap_or_default()
        ),
    )?;
    write_manifest(dir)?;
    Ok(ok)
}

/// Write `plan` as the restart plan of the corpus in `dir`, and the manifest again.
pub fn write_restart_plan(dir: &Path, plan: &RestartPlan) -> Result<()> {
    write_file(
        &dir.join(restart::RESTART_PLAN),
        &format!("{}\n", plan.to_json()),
    )?;
    write_manifest(dir)
}

/// The top directories whose absolute paths name a machine: home and temporary directories.
const MACHINE_DIRECTORIES: [&str; 5] = ["home", "Users", "root", "tmp", "var"];

/// Refuse text that names a machine: an absolute path under a home or temporary directory.
fn check_no_machine_paths(rel: &str, text: &str) -> Result<()> {
    for marker in MACHINE_DIRECTORIES.map(|d| format!("/{d}/")) {
        if text.contains(&marker) {
            return Err(corpus_error(
                rel,
                format!("holds a machine path ({marker}); the corpus must not"),
            ));
        }
    }
    Ok(())
}

/// Join the devnet archives `archives` (one or more increments of the same chain, in order) into
/// a corpus at `out`, checking that each increment continues the one before it. `seed` is the
/// chain's public key-derivation seed; it must derive the genesis block's generator.
pub fn import(archives: &[PathBuf], out: &Path, seed: &str) -> Result<()> {
    let opened: Vec<Archive> = archives
        .iter()
        .map(|a| Archive::open(a))
        .collect::<Result<_>>()?;
    let Some(first) = opened.first() else {
        return Err(Error::Usage(
            "corpus import needs at least one archive".into(),
        ));
    };
    if fs::read_dir(out).is_ok_and(|mut d| d.next().is_some()) {
        return Err(corpus_error(out, "the output directory is not empty"));
    }
    let chain = first
        .manifest
        .get("chain")
        .cloned()
        .ok_or_else(|| corpus_error(&first.dir, "the manifest has no chain"))?;
    if first.height("from")? != 1 {
        return Err(corpus_error(
            &first.dir,
            "the first archive does not start at the genesis block",
        ));
    }
    let crypto_of = |a: &Archive| -> Result<Vec<Vec<u8>>> {
        CRYPTO_FILES
            .iter()
            .map(|name| {
                let path = a.dir.join("crypto").join(format!("{name}.json"));
                fs::read(&path).map_err(|e| Error::io(&path, e))
            })
            .collect()
    };
    let crypto = crypto_of(first)?;
    check_seed(&first.dir, &crypto, seed)?;
    for pair in opened.windows(2) {
        let (before, next) = (&pair[0], &pair[1]);
        if next.manifest.get("chain") != Some(&chain) {
            return Err(corpus_error(
                &next.dir,
                "another chain than the first archive",
            ));
        }
        if crypto_of(next)? != crypto {
            return Err(corpus_error(
                &next.dir,
                "other crypto files than the first archive",
            ));
        }
        let previous = next.manifest.get("previous");
        let links = next.height("from")? == before.height("to")? + 1
            && previous.and_then(|p| p.get("lastHeight")) == before.range("to")
            && previous.and_then(|p| p.get("lastId")) == before.range("lastId");
        if !links {
            return Err(corpus_error(
                &next.dir,
                format!(
                    "does not continue {} (it must start at {} after block {})",
                    before.dir.display(),
                    before.height("to")? + 1,
                    before
                        .range("lastId")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                ),
            ));
        }
    }
    let genesis_id = chain
        .get("genesisId")
        .and_then(Value::as_str)
        .ok_or_else(|| corpus_error(&first.dir, "the manifest has no chain.genesisId"))?
        .to_owned();
    let round_length = chain
        .get("activeDelegates")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| corpus_error(&first.dir, "the manifest has no chain.activeDelegates"))?;

    let mut blocks = Vec::new();
    let mut meta = Vec::new();
    let mut transactions = 0usize;
    let mut previous_id: Option<String> = None;
    let mut next_height = 1u64;
    let mut rounds: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
    let mut missed = Vec::new();
    let mut rejections = Vec::new();
    let mut live_restarts = Vec::new();
    let mut checkpoints: Vec<(String, String)> = Vec::new();
    let mut archive_notes = Vec::new();
    for archive in &opened {
        for rel in archive.files("blocks/") {
            let path = archive.dir.join(&rel);
            for (index, line) in read_jsonl(&path)?.iter().enumerate() {
                let bad = |message: String| Error::Parse {
                    path: path.clone(),
                    line: index + 1,
                    message,
                };
                let height = line.get("height").and_then(Value::as_u64).unwrap_or(0);
                if height != next_height {
                    return Err(bad(format!("height {height}, expected {next_height}")));
                }
                next_height += 1;
                let id = line
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if header_id(line).as_deref() != Some(id.as_str()) {
                    return Err(bad(format!("the header does not hash to the id {id}")));
                }
                if height == 1 {
                    if id != genesis_id {
                        return Err(bad("height 1 is not the chain's genesis block".into()));
                    }
                } else {
                    let bytes = block_bytes(line).map_err(bad)?;
                    let previous = bytes.get(12..44).map(hex::encode);
                    if previous != previous_id {
                        return Err(bad("the block does not link to the block before".into()));
                    }
                    transactions += line
                        .get("transactions")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    blocks.push(json!({"h": height, "id": id, "hex": hex::encode(&bytes)}));
                    let m = line.get("meta").cloned().unwrap_or(Value::Null);
                    meta.push(json!({
                        "h": height,
                        "username": m.get("username").cloned().unwrap_or(Value::Null),
                        "burnedFee": m.get("burnedFee").cloned().unwrap_or(Value::Null),
                        "donations": m.get("donations").cloned().unwrap_or_else(|| json!({})),
                    }));
                }
                previous_id = Some(id);
            }
        }
        let mut archive_rounds: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
        for row in read_jsonl(&archive.dir.join("rounds.jsonl"))? {
            let round = row.get("round").and_then(Value::as_u64).unwrap_or(0);
            archive_rounds.entry(round).or_default().push(json!({
                "round": round,
                "publicKey": row.get("public_key").cloned().unwrap_or(Value::Null),
                "balance": row.get("balance").cloned().unwrap_or(Value::Null),
            }));
        }
        rounds.extend(archive_rounds);
        for row in read_jsonl(&archive.dir.join("missed-blocks.jsonl"))? {
            missed.push(json!({
                "timestamp": row.get("timestamp").cloned().unwrap_or(Value::Null),
                "height": row.get("height").cloned().unwrap_or(Value::Null),
                "username": row.get("username").cloned().unwrap_or(Value::Null),
            }));
        }
        let rejections_path = archive.dir.join("rejections.jsonl");
        if rejections_path.exists() {
            rejections.extend(read_jsonl(&rejections_path)?);
        }
        let restarts_path = archive.dir.join("restarts.jsonl");
        if restarts_path.exists() {
            live_restarts.extend(read_jsonl(&restarts_path)?);
        }
        for rel in archive.files("checkpoints/") {
            let text = read_text(&archive.dir.join(&rel))?;
            let name = rel.trim_start_matches("checkpoints/").to_owned();
            checkpoints.push((name, text));
        }
        archive_notes.push(json!({
            "from": archive.height("from")?,
            "to": archive.height("to")?,
            "blocks": archive.range("blocks").cloned().unwrap_or(Value::Null),
            "transactions": archive.range("transactions").cloned().unwrap_or(Value::Null),
            "verified": true,
        }));
    }
    let tip = u32::try_from(next_height - 1)
        .map_err(|_| corpus_error(&first.dir, "the chain is too long"))?;
    let tip_id = previous_id.unwrap_or_default();
    let revert_seed = hex::decode(&tip_id)
        .ok()
        .and_then(|b| b.get(..8).map(<[u8]>::to_vec))
        .and_then(|b| <[u8; 8]>::try_from(b.as_slice()).ok())
        .map_or(0, u64::from_be_bytes);
    let plan = RevertPlan::generate(revert_seed, tip, round_length);

    let mut files: Vec<(String, String)> = Vec::new();
    for (name, bytes) in CRYPTO_FILES.iter().zip(&crypto) {
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| corpus_error(&first.dir, format!("crypto/{name}.json is not UTF-8")))?;
        files.push((format!("crypto/{name}.json"), text));
    }
    files.push(("blocks.jsonl".into(), jsonl(&blocks)));
    files.push(("reference/meta.jsonl".into(), jsonl(&meta)));
    let round_rows: Vec<Value> = rounds.values().flatten().cloned().collect();
    files.push(("reference/rounds.jsonl".into(), jsonl(&round_rows)));
    files.push(("reference/missed-blocks.jsonl".into(), jsonl(&missed)));
    if !rejections.is_empty() {
        files.push(("reference/rejections.jsonl".into(), jsonl(&rejections)));
    }
    if !live_restarts.is_empty() {
        files.push((restart::LIVE_RESTARTS.into(), jsonl(&live_restarts)));
    }
    for (name, text) in &checkpoints {
        files.push((format!("reference/checkpoints/{name}"), text.clone()));
    }
    files.push((
        "revert-plan.json".into(),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&plan.to_json()).unwrap_or_default()
        ),
    ));
    let reference = first
        .manifest
        .get("solar")
        .and_then(|s| s.get("reference"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut reference_fields = Map::new();
    for key in ["commit", "keyFormat", "seriesSha256"] {
        if let Some(v) = reference.get(key) {
            reference_fields.insert(key.to_owned(), v.clone());
        }
    }
    let corpus_meta = json!({
        "format": FORMAT,
        "seed": seed,
        "chain": chain,
        "reference": reference_fields,
        "tip": tip,
        "blocks": blocks.len(),
        "transactions": transactions,
        "rounds": rounds.len(),
        "roundRows": round_rows.len(),
        "missedRows": missed.len(),
        "rejections": rejections.len(),
        "liveRestarts": live_restarts.len(),
        "checkpoints": checkpoints.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        "archives": archive_notes,
        "revertSteps": plan.steps.len(),
        "revertSeed": revert_seed.to_string(),
    });
    files.push((
        "corpus.json".into(),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&corpus_meta).unwrap_or_default()
        ),
    ));
    for (rel, text) in &files {
        check_no_machine_paths(rel, text)?;
    }
    for (rel, text) in &files {
        write_file(&out.join(rel), text)?;
    }
    write_manifest(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::testutil::TestDir;

    #[test]
    fn plans_are_seeded_and_in_domain() {
        let a = RevertPlan::generate(7, 2_650, 53);
        assert_eq!(a, RevertPlan::generate(7, 2_650, 53));
        assert_ne!(a.steps, RevertPlan::generate(8, 2_650, 53).steps);
        let round_ends = a
            .steps
            .iter()
            .filter(|s| s.blocks == 1 && s.after_height % 53 == 0)
            .count();
        assert!(round_ends >= 50, "{round_ends}");
        for tenth in [530, 1_060, 1_590, 2_120, 2_650] {
            let at: Vec<u32> = a.after(tenth).map(|s| s.blocks).collect();
            assert!(
                at.contains(&1) && at.contains(&2) && at.contains(&54),
                "{tenth}: {at:?}"
            );
        }
        assert!(
            a.steps
                .windows(2)
                .all(|w| w[0].after_height <= w[1].after_height)
        );
        for s in &a.steps {
            assert!(
                s.blocks >= 1 && s.blocks < s.after_height && s.blocks <= 102,
                "{s:?}"
            );
            assert!(s.after_height >= 2 && s.after_height <= 2_650, "{s:?}");
        }
        let json = a.to_json();
        assert_eq!(
            RevertPlan::from_json(&json, Path::new("revert-plan.json")).unwrap(),
            a
        );
    }

    #[test]
    fn short_chains_never_revert_the_genesis_block() {
        assert!(RevertPlan::generate(1, 1, 53).steps.is_empty());
        for tip in 2..60 {
            for s in RevertPlan::generate(u64::from(tip), tip, 53).steps {
                assert!(
                    s.blocks < s.after_height && s.after_height <= tip,
                    "{tip} {s:?}"
                );
            }
        }
        let bad = json!({"format": PLAN_FORMAT, "seed": "1", "roundLength": 53, "tip": 5,
                         "steps": [{"after": 3, "blocks": 3}]});
        assert!(RevertPlan::from_json(&bad, Path::new("revert-plan.json")).is_err());
    }

    /// A tiny archive in the devnet tooling's format: `heights` blocks with fake headers whose
    /// ids are their hashes, linked by previous-block ids.
    fn write_archive(
        root: &Path,
        name: &str,
        from: u64,
        to: u64,
        previous: Option<(u64, &str)>,
    ) -> (PathBuf, String) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("blocks")).unwrap();
        fs::create_dir_all(dir.join("crypto")).unwrap();
        fs::create_dir_all(dir.join("checkpoints")).unwrap();
        let mut prev = previous.map_or("00".repeat(32), |(_, id)| id.to_owned());
        let mut lines = String::new();
        let mut first_id = None;
        for h in from..=to {
            let mut header = vec![0u8; 12];
            header[4..8].copy_from_slice(&u32::try_from(h).unwrap().to_le_bytes());
            header.extend(hex::decode(&prev).unwrap());
            header.extend([h as u8; 20]);
            let id = sha256_hex(&header);
            first_id.get_or_insert(id.clone());
            lines.push_str(&format!(
                "{}\n",
                json!({"height": h, "id": id, "round": 1, "header": hex::encode(&header),
                       "transactions": if h == 1 { vec![] } else { vec!["aabb"] },
                       "txIds": [], "meta": {"burnedFee": "0", "donations": {}, "username": "genesis_1"}})
            ));
            prev = id;
        }
        fs::write(dir.join(format!("blocks/{from:09}-{to:09}.jsonl")), &lines).unwrap();
        let generator = heartwood_crypto::genesis::derive::role_key(
            crate::synthetic::DERIVATION_TAG,
            "generator",
            "test-seed",
            90,
        )
        .unwrap();
        let crypto = [
            json!({"pubKeyHash": 90}),
            json!([]),
            json!({"generatorPublicKey": generator.keys.public_key().to_hex()}),
            json!({}),
        ];
        for (f, value) in CRYPTO_FILES.iter().zip(crypto) {
            fs::write(dir.join(format!("crypto/{f}.json")), format!("{value}\n")).unwrap();
        }
        fs::write(
            dir.join("rounds.jsonl"),
            format!(
                "{}\n",
                json!({"round": 1, "public_key": "02aa", "balance": "5"})
            ),
        )
        .unwrap();
        fs::write(
            dir.join("missed-blocks.jsonl"),
            format!(
                "{}\n",
                json!({"timestamp": 8 * from, "height": from, "username": "genesis_2"})
            ),
        )
        .unwrap();
        fs::write(
            dir.join(format!("checkpoints/h{to:09}.json")),
            "{\"height\":1}\n",
        )
        .unwrap();
        let mut files = Map::new();
        for rel in files_under(&dir).unwrap() {
            files.insert(
                rel.clone(),
                json!(sha256_hex(&fs::read(dir.join(&rel)).unwrap())),
            );
        }
        let genesis_id = if from == 1 {
            first_id.clone().unwrap()
        } else {
            "g".into()
        };
        let manifest = json!({
            "format": ARCHIVE_FORMAT,
            "chain": {"genesisId": genesis_id, "activeDelegates": 53, "nethash": "n", "epoch": "e", "blockTime": 8},
            "range": {"from": from, "to": to, "blocks": to - from + 1, "transactions": to - from, "lastId": prev},
            "previous": previous.map(|(h, id)| json!({"lastHeight": h, "lastId": id})),
            "problems": [],
            "files": files,
            "solar": {"reference": {"name": "s1-ref", "tag": "s1-ref-v2", "commit": "c"}},
        });
        fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
        fs::write(dir.join("verify.json"), "{\"ok\":true}").unwrap();
        (dir, prev)
    }

    fn fix_genesis(dir: &Path, genesis_id: &str) {
        let path = dir.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        manifest["chain"]["genesisId"] = json!(genesis_id);
        fs::write(&path, manifest.to_string()).unwrap();
    }

    #[test]
    fn imports_linked_archives_and_writes_a_manifest() {
        let root = TestDir::new("corpus-import");
        let (a, last_a) = write_archive(root.path(), "a", 1, 3, None);
        let genesis: Value =
            serde_json::from_str(&fs::read_to_string(a.join("manifest.json")).unwrap()).unwrap();
        let genesis_id = genesis["chain"]["genesisId"].as_str().unwrap().to_owned();
        let (b, _) = write_archive(root.path(), "b", 4, 6, Some((3, &last_a)));
        fix_genesis(&b, &genesis_id);
        let out = root.path().join("corpus");
        let wrong = import(
            &[a.clone(), b.clone()],
            &root.path().join("wrong"),
            "other seed",
        );
        assert!(wrong.unwrap_err().to_string().contains("seed"));
        import(&[a.clone(), b.clone()], &out, "test-seed").unwrap();
        verify(&out).unwrap();
        let blocks = read_jsonl(&out.join("blocks.jsonl")).unwrap();
        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0]["h"], json!(2));
        // Header, one u32le length (2), then the transaction bytes.
        assert!(blocks[0]["hex"].as_str().unwrap().ends_with("02000000aabb"));
        let meta: Value = read_json(&out.join("corpus.json")).unwrap();
        assert_eq!(meta["tip"], json!(6));
        assert_eq!(meta["archives"].as_array().unwrap().len(), 2);
        assert_eq!(
            read_jsonl(&out.join("reference/missed-blocks.jsonl"))
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            read_jsonl(&out.join("reference/rounds.jsonl"))
                .unwrap()
                .len(),
            1
        );
        assert!(out.join("reference/checkpoints/h000000006.json").exists());
        // A changed file breaks the manifest.
        fs::write(out.join("blocks.jsonl"), "").unwrap();
        assert!(matches!(verify(&out), Err(Error::Manifest { .. })));
        // An unlisted file does too.
        write_manifest(&out).unwrap();
        fs::write(out.join("extra.txt"), "x").unwrap();
        assert!(matches!(verify(&out), Err(Error::Manifest { .. })));
    }

    #[test]
    fn refuses_archives_that_do_not_link() {
        let root = TestDir::new("corpus-links");
        let (a, _) = write_archive(root.path(), "a", 1, 3, None);
        let (b, _) = write_archive(root.path(), "b", 4, 6, Some((3, &"11".repeat(32))));
        let out = root.path().join("corpus");
        let error = import(&[a.clone(), b], &out, "test-seed")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("does not continue") || error.contains("another chain"),
            "{error}"
        );
        // An archive whose files do not match its manifest is refused.
        fs::write(a.join("rounds.jsonl"), "{}\n").unwrap();
        let error = import(&[a], &root.path().join("corpus2"), "test-seed")
            .unwrap_err()
            .to_string();
        assert!(error.contains("SHA-256"), "{error}");
    }

    #[test]
    fn refuses_machine_paths() {
        let root = TestDir::new("corpus-paths");
        let (a, _) = write_archive(root.path(), "a", 1, 2, None);
        fs::write(
            a.join("checkpoints/h000000002.json"),
            format!("{{\"dir\":\"/{}/someone/x\"}}\n", MACHINE_DIRECTORIES[0]),
        )
        .unwrap();
        let mut manifest: Value = read_json(&a.join("manifest.json")).unwrap();
        manifest["files"]["checkpoints/h000000002.json"] = json!(sha256_hex(
            &fs::read(a.join("checkpoints/h000000002.json")).unwrap()
        ));
        fs::write(a.join("manifest.json"), manifest.to_string()).unwrap();
        let error = import(&[a], &root.path().join("corpus"), "test-seed")
            .unwrap_err()
            .to_string();
        assert!(error.contains("machine path"), "{error}");
        assert!(!root.path().join("corpus").join("corpus.json").exists());
    }
}
