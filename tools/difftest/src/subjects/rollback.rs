//! `chain.rollback`: `snapshot rollback` of a corpus chain's store at its tip, and the start after
//! it, as the rollback class's records hold them:
//!
//! - `chain.rollback` `{corpus, height?, number?}` -> `{rollback, restart?, active?}`: `corpus`
//!   names a committed corpus (`tools/difftest/corpus`); `rollback` is the oracle's
//!   `chain.rollback` answer on that chain at its tip (`{refused: {lines}}`, or `{target, round,
//!   lines, deleted, summary}`); when it rolled back, `restart` is its start without a saved state
//!   (`{line, summary}`) and `active` the rows that start saved for the round after the target,
//!   sorted by public key (see `tools/oracle/lib/vectors/rollback.js`).
//!
//! Heartwood's side is the store of the chain applied once from a first start, a copy of it rolled
//! back through `heartwood_blockchain::rollback`, and the relay's start after it, which must
//! restore by a replay for want of a state ([`crate::levels::l2_rollback`]). Its restart line
//! follows the running digest of the chain's lines, as the oracle's session's does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use heartwood_blockchain::{ReplayReason, RestorePath};
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed};
use crate::corpus::Corpus;
use crate::levels::l2_rollback::{Flags, TipImage, TipStore, roll_back, start_after};
use crate::store::StartProblem;
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("chain.rollback", rollback)];

/// The store of each committed corpus at its tip, built once for the class's cases.
static TIPS: Mutex<BTreeMap<String, TipImage>> = Mutex::new(BTreeMap::new());

/// The store of the committed corpus `name` at its tip, applied once from a first start.
fn tip_store(name: &str) -> Result<TipStore, SubjectError> {
    let mut tips = TIPS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(image) = tips.get(name) {
        return image.store().map_err(|e| failed(e.to_string()));
    }
    let corpus = Corpus::load(&committed(name)?).map_err(|e| failed(e.to_string()))?;
    let tip = TipStore::applied(&corpus).map_err(start)?;
    let image = tip.image().map_err(|e| failed(e.to_string()))?;
    tips.insert(name.to_owned(), image);
    Ok(tip)
}

/// The committed corpus `name`.
fn committed(name: &str) -> Result<PathBuf, SubjectError> {
    let plain = (name.bytes()).all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if name.is_empty() || !plain {
        return Err(failed(format!("{name:?} is not a corpus name")));
    }
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join(name))
}

fn start(problem: StartProblem) -> SubjectError {
    match problem {
        StartProblem::Unbuilt(_) => SubjectError::NotImplemented,
        StartProblem::Failed(why) => failed(why),
    }
}

fn rollback(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let name = input["corpus"]
        .as_str()
        .ok_or_else(|| failed("corpus must be a name"))?;
    let flag = |key: &str| match &input[key] {
        Value::Null => Ok(None),
        value => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| failed(format!("{key} must be an integer"))),
    };
    let flags = Flags {
        height: flag("height")?,
        number: flag("number")?,
    };
    let tip = tip_store(name)?;
    let ours = roll_back(&tip, flags).map_err(|e| failed(e.to_string()))?;
    if !ours.rolled {
        return Ok(Verdict::Output(json!({ "rollback": ours.answer })));
    }
    let started = start_after(&ours.scratch, &tip.files, &tip.chain_digest).map_err(start)?;
    if started.path != RestorePath::Replay(ReplayReason::NoState) {
        return Err(failed(format!(
            "the start after the rollback took the path {:?}",
            started.path
        )));
    }
    Ok(Verdict::Output(json!({
        "rollback": ours.answer,
        "restart": started.restart,
        "active": started.active,
    })))
}
