//! `state.indexOrders`: the address, public-key and username index orders of the state after a
//! corpus's continuous replay (its revert plan included) up to a digest line, after a restart from
//! the state tables, and after a restart that replays the chain, against the reference's live,
//! saved-state and StateBuilder orders and its order of a replay without reverts.
//!
//! The replay runs through a store, as a node's does (the store's writes of each applied and
//! reverted block, committed in chunks), so that the table path reads the rows the node's commits
//! wrote. The orders are read through the
//! repository's iterators that the API lists wallets by
//! ([`WalletRepository::by_address_index`] and its two siblings).
//!
//! Heartwood's live order and its replay path must equal the reference's live and replayed
//! orders. The restart paths differ on purpose ([`Documented::ReplayIndexOrder`]): the
//! reference's StateBuilder rebuilds the indexes in its handlers' order and its saved-state start
//! in the saved address order, and both then re-index the wallets of the current round's
//! transactions; Heartwood's replay path gives the order the chain's blocks set, and its table
//! path the order of the stored rows. [`masked`] accepts those two parts when they list the same
//! keys as the reference's, and Heartwood's replay path is the reference's replayed order. A
//! corpus with a delegate name or a voted name that reads as a number is answered as
//! [`Documented::SavedStateUsername`] where the oracle notes it, as the restart lines are.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use heartwood_blockchain::RestoreMode;
use heartwood_crypto::Block;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::identities::PublicKey;
use heartwood_crypto::utils::hex;
use heartwood_state::{Wallet, WalletRepository};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, text};
use crate::corpus::Corpus;
use crate::levels::l2::{Action, actions};
use crate::restart::HeartwoodMode;
use crate::store::{
    ScratchDir, StartProblem, StoreNode, crypto_files, restore_mode, took_its_path,
};
use crate::subject::{Case, Documented, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("state.indexOrders", index_orders)];

/// The steps written in one commit.
const CHUNK: usize = 64;

/// The three orders of `wallets`, in the oracle's shape.
fn orders(wallets: &WalletRepository) -> Value {
    let addresses: Vec<String> = (wallets.by_address_index())
        .map(|wallet| wallet.address().to_base58())
        .collect();
    let keys: Vec<String> = (wallets.by_public_key_index())
        .filter_map(Wallet::public_key)
        .map(PublicKey::to_hex)
        .collect();
    let names: Vec<&str> = (wallets.by_username_index())
        .filter_map(Wallet::delegate)
        .map(|delegate| delegate.username())
        .collect();
    json!({ "addresses": addresses, "publicKeys": keys, "usernames": names })
}

/// The directory of the corpus `name`: the committed corpora, then the repository's working ones.
fn corpus_dir(name: &str) -> Outcome<PathBuf> {
    let valid = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    if name.is_empty() || !name.chars().all(valid) {
        return Err(failed(format!("{name:?} is not a corpus name")));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    [root.join("corpus"), root.join("../../target/corpus")]
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|dir| dir.join("corpus.json").is_file())
        .ok_or_else(|| failed(format!("no corpus {name} in this tree")))
}

fn index_orders(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let name = text(input, "corpus")?;
    let dir = corpus_dir(name)?;
    let manifest = std::fs::read(dir.join("MANIFEST.sha256"))
        .map_err(|e| failed(format!("{name}: the manifest does not read: {e}")))?;
    if hex::encode(&sha256(&manifest)) != text(input, "manifest")? {
        return Err(failed(format!("{name} is not the corpus the case names")));
    }
    let corpus = Corpus::load(&dir).map_err(|e| failed(e.to_string()))?;
    let steps = actions(&corpus);
    let taken = (input["line"].as_u64())
        .and_then(|line| usize::try_from(line).ok()?.checked_sub(1))
        .and_then(|taken| steps.get(..taken))
        .ok_or_else(|| failed("input.line is not a line of the corpus's replay"))?;

    let (height, [live, saved, replayed]) =
        store_orders(&corpus, taken).map_err(|e| failed(format!("{name}: {e}")))?;
    let ours = json!({
        "height": height,
        "live": live,
        "saved": saved,
        "rebuild": replayed,
        "replayed": replayed,
    });
    // A name that the reference's saved state reads back as a number changes its saved-state
    // start (the oracle's note shows it); every other case is judged by the restart orders' mask.
    Ok(Verdict::DiffersWhereShown {
        difference: Documented::SavedStateUsername,
        verdict: Box::new(Verdict::DiffersWhereShown {
            difference: Documented::ReplayIndexOrder,
            verdict: Box::new(Verdict::Output(ours)),
        }),
    })
}

/// The height and the orders after `taken`, the first steps of `corpus`'s replay, run through a
/// new store as a node applies, reverts and commits them; then the orders after a restart from the
/// state tables and after a replay restart of that store.
fn store_orders(corpus: &Corpus, taken: &[Action]) -> Result<(u32, [Value; 3]), String> {
    let problem = |e: StartProblem| e.into_error().to_string();
    let files = crypto_files(corpus).map_err(problem)?;
    let scratch = ScratchDir::new("state-index").map_err(|e| e.to_string())?;
    let start = |mode| StoreNode::start(scratch.path(), &files, mode).map_err(problem);
    let (mut store, mut state, _) = start(RestoreMode::Auto)?.into_parts();
    let milestones = &corpus.milestones;
    let mut writer = store.chunk().map_err(|e| e.to_string())?;
    for (done, action) in (1..).zip(taken) {
        let written = match *action {
            Action::Revert => {
                let reverted = (state.revert_block(milestones, &Handlers))
                    .map_err(|e| format!("a revert at {}: {e}", state.height()))?;
                writer.reverted(&state, &reverted, milestones)
            }
            Action::Apply(height) | Action::Reapply(height) => {
                let bytes = &corpus.block(height).ok_or("a block past the tip")?.bytes;
                let block = Block::receive(bytes, milestones).map_err(|e| e.to_string())?;
                let applied = (state.apply_block(&block, milestones, &Handlers))
                    .map_err(|e| format!("block {height}: {e}"))?;
                writer.applied(&state, &block, &applied, milestones)
            }
        };
        written.map_err(|e| e.to_string())?;
        if done % CHUNK == 0 {
            writer.commit().map_err(|e| e.to_string())?;
            writer = store.chunk().map_err(|e| e.to_string())?;
        }
    }
    writer.commit().map_err(|e| e.to_string())?;
    drop(store);
    let restarted = |mode: HeartwoodMode| -> Result<Value, String> {
        let node = start(restore_mode(mode))?;
        if !took_its_path(mode, &node.path) {
            return Err(format!("the {} restart took {:?}", mode.name(), node.path));
        }
        Ok(orders(node.state.wallets()))
    };
    let live = orders(state.wallets());
    let table = restarted(HeartwoodMode::Table)?;
    Ok((
        state.height(),
        [live, table, restarted(HeartwoodMode::Replay)?],
    ))
}

/// The reference's answer `theirs` with its restart orders replaced by Heartwood's (`ours`), when
/// the two differ only as [`Documented::ReplayIndexOrder`] says: each of Heartwood's restart paths
/// lists the same keys as the reference's restart, and Heartwood's replay path is the reference's
/// replayed order. `None` otherwise, or when there is nothing to replace.
pub fn masked(theirs: &Value, ours: &Value) -> Option<Value> {
    let keys = |orders: &Value| -> Option<[BTreeSet<String>; 3]> {
        let set = |name: &str| -> Option<BTreeSet<String>> {
            (orders[name].as_array()?.iter())
                .map(|key| key.as_str().map(str::to_owned))
                .collect()
        };
        Some([set("addresses")?, set("publicKeys")?, set("usernames")?])
    };
    let same_keys = |part: &str| keys(&theirs[part]).is_some_and(|k| keys(&ours[part]) == Some(k));
    let explained = same_keys("saved")
        && same_keys("rebuild")
        && ours["rebuild"] == theirs["replayed"]
        && (theirs["saved"] != ours["saved"] || theirs["rebuild"] != ours["rebuild"]);
    explained.then(|| {
        let mut adjusted = theirs.clone();
        adjusted["saved"] = ours["saved"].clone();
        adjusted["rebuild"] = ours["rebuild"].clone();
        adjusted
    })
}
