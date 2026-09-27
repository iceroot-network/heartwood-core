//! `chain.process`: the block processor on a chain, answered with
//! `heartwood_blockchain::processor`: `{chain, hex, now, state?}` -> `{result, handler, unchained?,
//! missed, lastDownloaded, forked, clearQueue, wakeUp, removed, line?}` (see
//! `tools/oracle/lib/ops/process.js` for the input and the answer).
//!
//! The chain (its crypto files, read with the node's strict loader, and its blocks) is applied to
//! a state once and kept, at most [`KEPT_CHAINS`] of them; each case processes the block on a copy
//! of the tip's state, with a chain state, a clock, the stored ids and a pool made from the case's
//! `state`. A case on a chain session (`session`) has no Rust side: sessions live in the oracle.
//!
//! Two documented differences: a block whose bytes Heartwood refuses to rebuild where the
//! reference rebuilds it (a removed type, for example) is the decoder's documented difference;
//! and a block refused in its apply after missed slots is `Documented::MissedRowsOfRefusedBlock`,
//! which holds when Heartwood's processor refused the block at its apply and the state detects
//! missed slots before it (the reference has written their rows by then).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use heartwood_blockchain::ChainState;
use heartwood_blockchain::processor::{
    self, BlockProcessorResult, ForgedIds, Handler, PoolSteps, ProcessContext,
};
use heartwood_crypto::blocks::factory;
use heartwood_crypto::errors::BlockError;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockId, Milestones, Params, Transaction, TransactionId};
use heartwood_kernel::config::crypto::{self as crypto_files, CryptoTexts};
use heartwood_kernel::{ChainLink, ManualClock};
use heartwood_state::State;
use heartwood_state::digest::{self, Step};
use heartwood_state::round_state::MissedBlock;
use heartwood_transactions::Handlers;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{Env, Outcome, Run, block_refused, failed, text, unclassified};
use crate::subject::{Case, Documented, SubjectError, Verdict};

/// The operations of the processor.
pub(super) const OPS: &[(&str, Run)] = &[("chain.process", process)];

/// Chains kept at their tip, as the oracle keeps them.
pub const KEPT_CHAINS: usize = 16;
/// The most blocks a case may leave unstored.
const MAX_UNSTORED: usize = 100;

/// A chain applied to its tip.
struct Kept {
    key: [u8; 32],
    milestones: Milestones,
    state: State,
    blocks: Vec<Block>,
    /// The ids of every transaction of the chain, the genesis block's included.
    ids: BTreeSet<TransactionId>,
}

static KEPT: Mutex<Vec<Arc<Kept>>> = Mutex::new(Vec::new());

/// The chain of `value` at its tip: kept from an earlier case, or applied now.
fn kept(value: &Value) -> Outcome<Arc<Kept>> {
    let key: [u8; 32] = Sha256::digest(value.to_string().as_bytes()).into();
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(at) = kept.iter().position(|chain| chain.key == key) {
        let chain = kept.remove(at);
        kept.push(Arc::clone(&chain));
        return Ok(chain);
    }
    let chain = Arc::new(apply_chain(value, key)?);
    kept.push(Arc::clone(&chain));
    if kept.len() > KEPT_CHAINS {
        kept.remove(0);
    }
    Ok(chain)
}

fn apply_chain(value: &Value, key: [u8; 32]) -> Outcome<Kept> {
    let files = &value["crypto"];
    let file = |name: &str| -> Outcome<Option<String>> {
        match &files[name] {
            Value::Null => Err(failed(format!("input.chain.crypto.{name} is missing"))),
            file => Ok(Some(file.to_string())),
        }
    };
    let texts = CryptoTexts {
        network: file("network")?,
        milestones: file("milestones")?,
        genesis: file("genesisBlock")?,
        exceptions: file("exceptions")?,
    };
    let files = crypto_files::parse(&texts)
        .map_err(|e| failed(format!("the chain's crypto files do not load: {e}")))?;
    let milestones = files.milestones;
    let genesis = factory::from_json(&files.genesis, &milestones)
        .map_err(|e| failed(format!("the chain's genesis block: {e}")))?;
    let mut state = State::from_genesis(&genesis, &milestones, &Handlers)
        .map_err(|e| failed(format!("the chain's genesis block is refused: {e}")))?;
    let mut ids: BTreeSet<TransactionId> = genesis.transaction_ids().copied().collect();
    let mut blocks = Vec::new();
    let list = value["blocks"]
        .as_array()
        .ok_or_else(|| failed("input.chain.blocks is not a list"))?;
    for (index, item) in list.iter().enumerate() {
        let bytes = item
            .as_str()
            .and_then(|text| hex::decode(text).ok())
            .ok_or_else(|| failed(format!("input.chain.blocks[{index}] is not hex")))?;
        let block = Block::receive(&bytes, &milestones)
            .map_err(|e| failed(format!("input.chain.blocks[{index}]: {e}")))?;
        state
            .apply_block(&block, &milestones, &Handlers)
            .map_err(|e| failed(format!("input.chain.blocks[{index}] is refused: {e}")))?;
        ids.extend(block.transaction_ids().copied());
        blocks.push(block);
    }
    Ok(Kept {
        key,
        milestones,
        state,
        blocks,
        ids,
    })
}

/// The stored ids: the chain's, less those of its unstored blocks, and the case's own.
struct Stored<'a> {
    chain: &'a BTreeSet<TransactionId>,
    unstored: BTreeSet<TransactionId>,
    extra: BTreeSet<TransactionId>,
}

impl ForgedIds for Stored<'_> {
    fn is_stored(&self, id: &TransactionId) -> bool {
        self.extra.contains(id) || (self.chain.contains(id) && !self.unstored.contains(id))
    }
}

/// The pool: the ids the processor asked it to remove, in order.
#[derive(Default)]
struct Removed(Vec<TransactionId>);

impl PoolSteps for Removed {
    fn remove_forged(&mut self, transaction: &Transaction, _params: &Params) {
        self.0.push(*transaction.id());
    }
}

/// The case's chain state, as the oracle sets it.
struct CaseState {
    chain: ChainState,
    unstored: usize,
    forged: BTreeSet<TransactionId>,
    /// The block has no delegate name, as a block enqueued before its delegate registered.
    unnamed: bool,
}

fn case_state(input: &Value, kept: &Kept) -> Outcome<CaseState> {
    let state = match input.get("state") {
        None => &Value::Null,
        Some(Value::Object(_)) => &input["state"],
        Some(_) => return Err(failed("input.state is not an object")),
    };
    let tip = kept.state.last_block();
    let mut chain = ChainState::at(Arc::new(tip.clone()));
    chain.started = true;
    if let Some(link) = state.get("lastDownloaded") {
        chain.last_downloaded = match link {
            Value::Null => None,
            link => Some(chain_link(link)?),
        };
    }
    if let Some(forked) = state.get("forked").filter(|forked| !forked.is_null()) {
        let bytes = forked
            .as_str()
            .and_then(|text| hex::decode(text).ok())
            .ok_or_else(|| failed("input.state.forked is not hex"))?;
        let block = Block::receive(&bytes, &kept.milestones)
            .map_err(|e| failed(format!("input.state.forked: {e}")))?;
        chain.forked_block = Some(Arc::new(block));
    }
    if let Some(started) = state.get("started") {
        chain.started = started
            .as_bool()
            .ok_or_else(|| failed("input.state.started is not a boolean"))?;
    }
    let unstored = match state.get("unstored") {
        None => 0,
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n <= MAX_UNSTORED && n <= kept.blocks.len())
            .ok_or_else(|| failed("input.state.unstored is out of range"))?,
    };
    chain.last_stored_height = tip
        .height()
        .saturating_sub(u32::try_from(unstored).unwrap_or(u32::MAX));
    let mut forged = BTreeSet::new();
    if let Some(list) = state.get("forged") {
        for id in list
            .as_array()
            .ok_or_else(|| failed("input.state.forged is not a list"))?
        {
            let id = id
                .as_str()
                .and_then(|text| TransactionId::from_hex(text).ok())
                .ok_or_else(|| failed("input.state.forged holds a bad id"))?;
            forged.insert(id);
        }
    }
    let unnamed = match state.get("unnamed") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| failed("input.state.unnamed is not a boolean"))?,
    };
    Ok(CaseState {
        chain,
        unstored,
        forged,
        unnamed,
    })
}

fn chain_link(value: &Value) -> Outcome<ChainLink> {
    let id = |key: &str| {
        value[key]
            .as_str()
            .and_then(|text| BlockId::from_hex(text).ok())
            .ok_or_else(|| {
                failed(format!(
                    "input.state.lastDownloaded.{key} is not a block id"
                ))
            })
    };
    let number = |key: &str| {
        value[key]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| failed(format!("input.state.lastDownloaded.{key} is not a number")))
    };
    Ok(ChainLink {
        id: id("id")?,
        previous_block: id("previousBlock")?,
        height: number("height")?,
        timestamp: number("timestamp")?,
    })
}

fn missed_json(missed: &[MissedBlock]) -> Value {
    missed
        .iter()
        .map(|m| json!({"timestamp": m.timestamp, "height": m.height, "username": m.username}))
        .collect()
}

fn process(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    if input.get("session").is_some() {
        return Err(SubjectError::Unsupported(
            "a chain session exists only in the oracle".into(),
        ));
    }
    let kept = kept(&input["chain"])?;
    let now = input["now"]
        .as_i64()
        .ok_or_else(|| failed("input.now is not a time"))?;
    let Some(bytes) = hex::decode(text(input, "hex")?).ok() else {
        return Ok(unclassified("the block is not hex"));
    };
    let block = match Block::receive(&bytes, &kept.milestones) {
        Ok(block) => block,
        Err(BlockError::Decode(error)) => return Ok(block_refused(&error, &bytes)),
        Err(error) => return Ok(unclassified(error)),
    };
    let CaseState {
        mut chain,
        unstored,
        forged,
        unnamed,
    } = case_state(input, &kept)?;
    let pending: Vec<Block> = kept.blocks[kept.blocks.len() - unstored..].to_vec();
    let stored = Stored {
        chain: &kept.ids,
        unstored: pending
            .iter()
            .flat_map(Block::transaction_ids)
            .copied()
            .collect(),
        extra: forged,
    };
    let mut state = kept.state.clone();
    let gap = state.detect_missed_blocks(block.header(), &kept.milestones);
    let mut pool = Removed::default();
    let clock = ManualClock::new(now);
    let name = if unnamed {
        None
    } else {
        processor::delegate_name(&state, &block).map(str::to_owned)
    };
    let traced = {
        let mut ctx = ProcessContext {
            state: &mut state,
            handlers: &Handlers,
            milestones: &kept.milestones,
            chain: &mut chain,
            forged: &stored,
            pending: &pending,
            pool: &mut pool,
            clock: &clock,
        };
        processor::trace_named(&mut ctx, &block, name.as_deref())
    };
    let processed = traced.processed;
    if traced.handler == Handler::Accept
        && processed.result == BlockProcessorResult::Rejected
        && !gap.is_empty()
    {
        return Ok(Verdict::RefusedByDesign {
            difference: Documented::MissedRowsOfRefusedBlock,
            holds: true,
        });
    }
    let missed = processed
        .applied
        .as_ref()
        .map_or_else(Vec::new, |applied| applied.missed.clone());
    let mut out = Map::new();
    out.insert("result".into(), json!(processed.result.name()));
    out.insert("handler".into(), json!(traced.handler.name()));
    if let Some(status) = processed.unchained {
        out.insert("unchained".into(), json!(status.name()));
    }
    out.insert("missed".into(), missed_json(&missed));
    out.insert(
        "lastDownloaded".into(),
        chain.last_downloaded.map_or(
            Value::Null,
            |link| json!({"height": link.height, "id": link.id.to_hex()}),
        ),
    );
    out.insert(
        "forked".into(),
        chain
            .forked_block
            .as_ref()
            .map_or(Value::Null, |block| json!(block.height())),
    );
    out.insert("clearQueue".into(), json!(processed.unchained.is_some()));
    out.insert("wakeUp".into(), json!(chain.last_accepted_ms.is_some()));
    let removed: Vec<String> = pool.0.iter().map(TransactionId::to_hex).collect();
    out.insert("removed".into(), json!(removed));
    if processed.result == BlockProcessorResult::Accepted {
        let line = digest::height_digest(
            &state,
            &kept.milestones,
            Step::Apply,
            &missed,
            Some(block.header().reward),
            &[0; 32],
        );
        let mut line: Value = serde_json::from_str(&line.to_json())
            .map_err(|e| failed(format!("a digest line is not JSON: {e}")))?;
        if let Value::Object(fields) = &mut line {
            fields.shift_remove("cd");
        }
        out.insert("line".into(), line);
    }
    Ok(Verdict::Output(Value::Object(out)))
}
