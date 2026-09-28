//! `difftest corpus from-blocks`: a Heartwood relay's block files turned into a corpus that the
//! judge replays, as `corpus import` turns the devnet tooling's archives into one.
//!
//! A block file (`blocks-NNNN.bin`, one per start of the relay, read in the order given) holds one
//! JSON line per block the relay applied, `{h, id, hex}`, each written whole; a kill leaves at most
//! a torn last line, which is dropped and counted. Every line's bytes must decode to its height
//! and id, the chain's crypto files must be the seed's (the genesis generator derives from it),
//! and every block must link to the block below it. A block at or below a height applied before
//! is the relay's switch to another branch: the blocks above it that were applied before are
//! abandoned, and the lines after it build the branch again.
//!
//! The corpus's rows of `reference/` (the round rows, the missed rows and the block columns the
//! judge's anchors compare with) are those of a replay of the chain through Heartwood's store,
//! marked `"anchorSource": "heartwood-replay"` in `corpus.json`; a live check compares each relay's
//! own store with them too, so the reference's judge and the relays' stores meet in them. Nothing
//! is written to `out` (which must be empty) before the corpus replays.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_crypto::{Block, Milestones, Network, blocks::factory, utils::hex};
use heartwood_database::{Database, Settings};
use serde_json::{Value, json};

use crate::corpus::{self, Corpus, RevertPlan};
use crate::errors::{Error, Result};
use crate::live::run::read_json;
use crate::store::{self, ScratchDir};

/// The value of `corpus.json`'s `anchorSource` for a corpus whose rows a Heartwood replay wrote.
pub const ANCHOR_SOURCE: &str = "heartwood-replay";

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).map_err(|e| Error::io(path, e))
}

fn lines(path: &Path, values: impl IntoIterator<Item = Value>) -> Result<()> {
    let text: String = values.into_iter().map(|v| format!("{v}\n")).collect();
    write(path, &text)
}

/// Copy the directory `from` into `to`, recursively.
pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| Error::io(from, e))? {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let target = to.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|e| Error::io(entry.path(), e))?
            .is_dir()
        {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| Error::io(&target, e))?;
        }
    }
    Ok(())
}

/// A chain by height: each block with its line of the block file.
type Chain = BTreeMap<u32, (Block, Value)>;

/// The blocks of the files `blocks` (in start order) as the relay's chain holds them at the end:
/// each height's block with its line, and the number of torn last lines dropped.
fn chain_of(
    blocks: &[PathBuf],
    milestones: &Milestones,
    genesis: &Block,
) -> Result<(Chain, usize)> {
    let mut chain = Chain::new();
    let mut torn = 0;
    for path in blocks {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        for (index, part) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if !part.ends_with(b"\n") {
                torn += 1;
                continue;
            }
            let parse_error = |message: String| Error::Parse {
                path: path.clone(),
                line: index + 1,
                message,
            };
            let v: Value = serde_json::from_slice(part).map_err(|e| parse_error(e.to_string()))?;
            let raw = v["hex"]
                .as_str()
                .and_then(|s| hex::decode(s).ok())
                .ok_or_else(|| parse_error("no block bytes".into()))?;
            let block = Block::decode(&raw, milestones).map_err(|e| parse_error(e.to_string()))?;
            let h = block.height();
            if v["h"].as_u64() != Some(u64::from(h))
                || v["id"].as_str() != Some(&block.id().to_hex())
            {
                return Err(parse_error(
                    "its height or id is not its block bytes'".into(),
                ));
            }
            if h == 1 {
                if block.id() != genesis.id() {
                    return Err(parse_error("another genesis block".into()));
                }
                continue;
            }
            let previous = if h == 2 {
                Some(genesis.id())
            } else {
                chain.get(&(h - 1)).map(|(b, _)| b.id())
            };
            if previous.map(|id| id.to_hex()).as_deref()
                != Some(hex::encode(&block.header().previous_block).as_str())
            {
                return Err(parse_error(format!(
                    "the block of height {h} does not link to the block below it (a gap in the files)"
                )));
            }
            // A block at or below a height applied before abandons the blocks above it.
            chain.split_off(&h);
            chain.insert(h, (block, v));
        }
    }
    Ok((chain, torn))
}

/// Build in `out` (empty or missing) the corpus of the block files `blocks` (in start order) of
/// the chain whose crypto files are in `crypto`, with the chain's key-derivation `seed`.
pub fn from_blocks(blocks: &[PathBuf], crypto: &Path, seed: &str, out: &Path) -> Result<()> {
    let bad = |message: String| Error::Corpus {
        path: out.to_owned(),
        message,
    };
    if blocks.is_empty() || seed.is_empty() {
        return Err(bad("block files and a seed are required".into()));
    }
    if std::fs::read_dir(out).is_ok_and(|mut d| d.next().is_some()) {
        return Err(bad("the output directory is not empty".into()));
    }
    let text = |name: &str| {
        let path = crypto.join(format!("{name}.json"));
        std::fs::read_to_string(&path).map_err(|e| Error::io(path, e))
    };
    let network = Network::load(&text("network")?)?;
    let milestones = Milestones::load(&text("milestones")?, &network)?;
    let genesis = factory::from_json(&read_json(&crypto.join("genesisBlock.json"))?, &milestones)
        .map_err(|e| bad(format!("crypto/genesisBlock.json: {e}")))?;
    let derived = heartwood_crypto::genesis::derive::role_key(
        crate::synthetic::DERIVATION_TAG,
        "generator",
        seed,
        network.pub_key_hash,
    )?;
    if derived.keys.public_key().to_hex() != genesis.header().generator_public_key.to_hex() {
        return Err(bad(
            "the seed does not derive the genesis block's generator (give the chain's --seed)"
                .into(),
        ));
    }
    let (chain, torn) = chain_of(blocks, &milestones, &genesis)?;
    let Some((&tip, (tip_block, _))) = chain.last_key_value() else {
        return Err(bad("no whole block line after the genesis block".into()));
    };
    let params = milestones.at(1);
    let length = u32::try_from(params.active_delegates()).map_err(|e| bad(e.to_string()))?;
    let scratch = ScratchDir::new("block-corpus")?;
    let staged = scratch.path().join("corpus");
    for dir in ["crypto", "reference"] {
        let dir = staged.join(dir);
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    }
    for name in corpus::CRYPTO_FILES {
        write(&staged.join(format!("crypto/{name}.json")), &text(name)?)?;
    }
    lines(
        &staged.join("blocks.jsonl"),
        chain.values().map(|(_, v)| v.clone()),
    )?;
    // The revert plan's seed from the tip's id, as `corpus import` derives it.
    let tip_id = tip_block.id().to_hex();
    let revert_seed = hex::decode(&tip_id)
        .ok()
        .and_then(|b| b.get(..8).and_then(|b| <[u8; 8]>::try_from(b).ok()))
        .map_or(0, u64::from_be_bytes);
    let plan = RevertPlan::generate(revert_seed, tip, length);
    write(
        &staged.join("revert-plan.json"),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&plan.to_json()).unwrap_or_default()
        ),
    )?;
    let meta = json!({
        "format": corpus::FORMAT, "seed": seed, "tip": tip, "blocks": chain.len(),
        "anchorSource": ANCHOR_SOURCE, "tornRecords": torn,
        "chain": {"nethash": network.nethash, "genesisId": genesis.id().to_hex(),
            "epoch": params.epoch(), "activeDelegates": length, "blockTime": params.block_time()},
        "revertSteps": plan.steps.len(), "revertSeed": revert_seed.to_string(),
    });
    write(
        &staged.join("corpus.json"),
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&meta).unwrap_or_default()
        ),
    )?;
    corpus::write_manifest(&staged)?;
    // The rows of a replay through Heartwood's store.
    let loaded = Corpus::load(&staged)?;
    let data = scratch.path().join("data");
    store::replay(&loaded, &data, None)?;
    let db = Database::open(&Settings::in_data_dir(&data)).map_err(|e| bad(e.to_string()))?;
    let rows = store::read_rows(&db, tip, store::last_round(tip, length)).map_err(bad)?;
    lines(
        &staged.join("reference/rounds.jsonl"),
        rows.rounds.iter().flat_map(|(round, rows)| {
            rows.iter().map(move |r| {
                json!({"round": round, "publicKey": r.public_key, "balance": r.balance.to_string()})
            })
        }),
    )?;
    lines(
        &staged.join("reference/missed-blocks.jsonl"),
        rows.missed
            .iter()
            .map(|r| json!({"timestamp": r.timestamp, "height": r.height, "username": r.username})),
    )?;
    let mut columns = Vec::new();
    for r in &rows.blocks {
        let donations: Value = serde_json::from_str(&r.donations)
            .map_err(|e| bad(format!("the donations of block {}: {e}", r.height)))?;
        columns.push(json!({"h": r.height, "username": r.username,
            "burnedFee": r.burned_fee.to_string(), "donations": donations}));
    }
    lines(&staged.join("reference/meta.jsonl"), columns)?;
    corpus::write_manifest(&staged)?;
    Corpus::load(&staged)?;
    copy_dir(&staged, out)
}
