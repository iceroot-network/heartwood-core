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
//! path the order of the stored rows. The output's notes hold that order of the stored rows, read
//! from the store apart from the restart (`storedRows`: each row's address while its wallet holds
//! something, and the keys and names the rows hold, in row order). [`masked`] accepts the two
//! restart parts when they list the same keys as the reference's, Heartwood's replay path is the
//! reference's replayed order, and its table path is the stored rows' order.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use heartwood_blockchain::{ChainStore, RestoreMode};
use heartwood_crypto::Block;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::identities::PublicKey;
use heartwood_crypto::utils::hex;
use heartwood_state::persist::WalletRecord;
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
use crate::subject::{Case, Documented, NOTES, Verdict};

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

    let (height, [live, saved, replayed, rows]) =
        store_orders(&corpus, taken).map_err(|e| failed(format!("{name}: {e}")))?;
    let ours = json!({
        "height": height,
        "live": live,
        "saved": saved,
        "rebuild": replayed,
        "replayed": replayed,
        NOTES: { STORED_ROWS: rows },
    });
    Ok(verdict(ours))
}

/// Heartwood's verdict on a case it answers with `ours`: the restart orders are judged by the
/// mask of [`Documented::ReplayIndexOrder`], everything else exactly. A delegate name that the
/// reference's saved state reads back as a number (the oracle's note `savedStateUsername`) excuses
/// nothing here: the saved-state order then lists another key, which the mask refuses, so such a
/// corpus shows as a failure until its records are judged part by part.
fn verdict(ours: Value) -> Verdict {
    Verdict::DiffersWhereShown {
        difference: Documented::ReplayIndexOrder,
        verdict: Box::new(Verdict::Output(ours)),
    }
}

/// The note of Heartwood's output that holds the stored rows' order ([`row_orders`]).
const STORED_ROWS: &str = "storedRows";

/// The three orders the store's wallet rows give when each index takes them in row order, read
/// from the rows apart from any restart: the address of each row whose wallet holds something (a
/// key, a balance, a nonce, a second key or a delegate record), and the key and the name of each
/// row that holds one.
fn row_orders(store: &ChainStore, network: u8) -> Result<Value, String> {
    let mut wallets = Vec::new();
    for row in store.database().wallets().map_err(|e| e.to_string())? {
        let record: WalletRecord = serde_json::from_str(&row.record).map_err(|e| e.to_string())?;
        wallets.push(Wallet::from_record(record, network).map_err(|e| e.to_string())?);
    }
    let holds = |wallet: &&Wallet| {
        wallet.public_key().is_some()
            || wallet.balance() != 0
            || wallet.nonce() != 0
            || wallet.second_public_key().is_some()
            || wallet.is_delegate()
    };
    let addresses: Vec<String> = (wallets.iter().filter(holds))
        .map(|wallet| wallet.address().to_base58())
        .collect();
    let keys: Vec<String> = (wallets.iter().filter_map(Wallet::public_key))
        .map(PublicKey::to_hex)
        .collect();
    let names: Vec<&str> = (wallets.iter().filter_map(Wallet::delegate))
        .map(|delegate| delegate.username())
        .collect();
    Ok(json!({ "addresses": addresses, "publicKeys": keys, "usernames": names }))
}

/// The height and the orders after `taken`, the first steps of `corpus`'s replay, run through a
/// new store as a node applies, reverts and commits them; then the orders after a restart from the
/// state tables and after a replay restart of that store, and the stored rows' order.
fn store_orders(corpus: &Corpus, taken: &[Action]) -> Result<(u32, [Value; 4]), String> {
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
    let rows = row_orders(&store, state.wallets().network())?;
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
        [live, table, restarted(HeartwoodMode::Replay)?, rows],
    ))
}

/// The reference's answer `theirs` with its restart orders replaced by Heartwood's (`ours`), when
/// the two differ only as [`Documented::ReplayIndexOrder`] says: each of Heartwood's restart paths
/// lists the same keys as the reference's restart, Heartwood's replay path is the reference's
/// replayed order, and its table path is the order of the stored rows (the note `storedRows`).
/// `None` otherwise, or when there is nothing to replace.
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
        && ours["saved"] == ours[NOTES][STORED_ROWS]
        && (theirs["saved"] != ours["saved"] || theirs["rebuild"] != ours["rebuild"]);
    explained.then(|| {
        let mut adjusted = theirs.clone();
        adjusted["saved"] = ours["saved"].clone();
        adjusted["rebuild"] = ours["rebuild"].clone();
        adjusted
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::l0::judge_case;
    use crate::subject::{Registry, Subject, SubjectError};
    use crate::vectors::Expected;

    /// A subject that answers every case with the verdict of `ours`.
    struct Answer(Value);

    impl Subject for Answer {
        fn op(&self) -> &'static str {
            "state.indexOrders"
        }
        fn run(&self, _case: &Case<'_>) -> Result<Verdict, SubjectError> {
            Ok(verdict(self.0.clone()))
        }
    }

    fn orders(addresses: &[&str], keys: &[&str], names: &[&str]) -> Value {
        json!({ "addresses": addresses, "publicKeys": keys, "usernames": names })
    }

    /// The reference's answer: every part in one order, the restarts in another.
    fn theirs() -> Value {
        let live = orders(&["a", "b", "c"], &["k1", "k2"], &["n1", "n2"]);
        let restart = orders(&["c", "a", "b"], &["k2", "k1"], &["n2", "n1"]);
        json!({
            "height": 9, "live": live, "saved": restart, "rebuild": restart, "replayed": live,
            NOTES: { "rebuildOrder": true },
        })
    }

    /// Heartwood's answer: the table path in the rows' order `saved`, noted as `rows`, and the
    /// replay path in `replayed`.
    fn ours(saved: &Value, rows: &Value, replayed: &Value) -> Value {
        let live = orders(&["a", "b", "c"], &["k1", "k2"], &["n1", "n2"]);
        json!({
            "height": 9, "live": live, "saved": saved, "rebuild": replayed, "replayed": replayed,
            NOTES: { STORED_ROWS: rows },
        })
    }

    #[test]
    fn the_restart_orders_are_taken_only_as_the_difference_says() {
        let theirs = theirs();
        let live = &theirs["live"];
        let rows = orders(&["b", "a", "c"], &["k1", "k2"], &["n1", "n2"]);
        let taken = masked(&theirs, &ours(&rows, &rows, live)).expect("the difference");
        assert_eq!(taken["saved"], rows);
        assert_eq!(taken["rebuild"], *live);
        assert_eq!(taken["live"], *live);

        // The table path in another order than the stored rows'.
        let other = orders(&["a", "b", "c"], &["k1", "k2"], &["n1", "n2"]);
        assert_eq!(masked(&theirs, &ours(&other, &rows, live)), None);
        // The replay path in another order than the reference's replayed one.
        let replayed = orders(&["b", "a", "c"], &["k1", "k2"], &["n1", "n2"]);
        assert_eq!(masked(&theirs, &ours(&rows, &rows, &replayed)), None);
        // A restart that lists another wallet.
        let more = orders(&["b", "a", "c", "d"], &["k1", "k2"], &["n1", "n2"]);
        assert_eq!(masked(&theirs, &ours(&more, &more, live)), None);
        // Nothing to replace: the case is compared as it is.
        let same = ours(&theirs["saved"], &theirs["saved"], live);
        let mut equal = theirs.clone();
        equal["rebuild"] = live.clone();
        assert_eq!(masked(&equal, &same), None);
    }

    #[test]
    fn a_name_that_reads_as_a_number_excuses_no_order() {
        let mut theirs = theirs();
        theirs[NOTES]["savedStateUsername"] = json!(true);
        let rows = orders(&["b", "a", "c"], &["k1", "k2"], &["n1", "n2"]);
        let mut ours = ours(&rows, &rows, &theirs["live"]);
        ours["live"] = orders(&["c", "b", "a"], &["k1", "k2"], &["n1", "n2"]);
        let mut registry = Registry::new();
        registry
            .register(Box::new(Answer(ours)))
            .expect("one subject");
        let input = json!({});
        let case = Case {
            op: "state.indexOrders",
            network: "devnet",
            height: 9,
            input: &input,
        };
        let outcome = judge_case(&case, &Expected::Output(theirs), &registry);
        assert!(outcome.is_failure(), "{outcome:?}");
    }
}
