//! The synthetic rollback cases on a corpus chain's tip, which `run l2 DIR --synthetic` runs with
//! the other synthetic cases: a rollback on both sides and the start after it, compared line by
//! line and row by row, with a stored pool, and with an older saved state kept on the reference's
//! side.
//!
//! - `rollback-last-round` (on the one-node corpus, `--height 264`, which lands on 212 with
//!   missed-slot rows left below it): the reference's rollback, its start without a saved state
//!   (which rebuilds, starts round 5 and saves its rows) and its next block; Heartwood's
//!   rollback, its start (a replay for want of a state, which writes round 5's rows) and the same
//!   block. The rollbacks' answers, the starts' lines (and the judge's at the target), the round's
//!   rows as sets, the stores' summaries after the starts and the next block's lines must be
//!   equal; then Heartwood's re-sync to the tip gives the judge's lines and the store's anchors.
//! - `rollback-no-missed-row` (`--height 133`, which lands on 106 with no missed-slot row left,
//!   where the reference's start runs its productivity backfill): the same.
//! - `rollback-stored-pool`: transfers in both pools at the tip, then `--height 133` and the start;
//!   both pools re-add their stored transactions against the rolled-back state, and their contents
//!   must be equal.
//! - `rollback-older-saved-state`: as `rollback-last-round`, with the reference's states saved 12
//!   blocks below the target and at the tip's three blocks (as a relay that drained after a sync
//!   chunk and then after each block keeps them). The reference's start refuses the three newest,
//!   loads the older one and cuts its chain back to that state's height, a documented difference
//!   ([`older_saved_state`]); Heartwood starts at the target. After both re-sync to the tip, their
//!   lines at the tip and their stores' summaries must be equal.
//!
//! The heights follow the corpus's tip: the first case rolls back with `--height tip - 1`, the
//! second and third with `--height 133` (skipped below a tip of 160).

use heartwood_crypto::Transaction;
use heartwood_crypto::utils::hex;
use heartwood_kernel::clock::ManualClock;
use heartwood_pool::{DelegateVersions, Entry, Pool, PoolContext, Processor};
use heartwood_state::State;
use serde_json::{Value, json};

use crate::corpus::Corpus;
use crate::digests;
use crate::errors::{Error, Result};
use crate::levels::l2::{call, corpus_crypto};
use crate::levels::l2_rollback::{
    Flags, TipStore, oracle_open, oracle_rollback, resync, roll_back, start_after,
};
use crate::levels::pool::{contents, judge, options};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};
use crate::restart::SAME_STATE;
use crate::store::{self, ScratchDir};
use crate::store_summary;
use crate::synthetic::{FEE, Sides, corpus_keys, transfer};

/// The keys a line of a block shares with the line of the same block on the other side (the step
/// and the running digest aside).
const SAME_STEP: [&str; 9] = ["h", "id", "rw", "b", "s", "r", "m", "w", "sd"];

/// The reason of the older-saved-state difference, for reports.
pub const OLDER_SAVED_STATE: &str = "a start whose newer saved states name blocks no longer stored while an older one \
     survives below the tip: the reference's loader refuses the newer ones, loads the older one \
     and cuts the chain back to its height; Heartwood keeps no saved-state file, so its start \
     replays to the stored tip";

/// The results of the cases, in order.
struct Results {
    file: String,
    results: Vec<CaseResult>,
}

impl Results {
    fn push(&mut self, op: &str, label: &str, height: u32, outcome: Outcome) {
        let line = self.results.len() + 1;
        self.results.push(CaseResult {
            file: self.file.clone(),
            line,
            op: op.to_owned(),
            label: format!("{op}: {label}"),
            height: u64::from(height),
            outcome,
        });
    }
}

/// `Pass` when `ours` and `theirs` agree on `keys`, else the differences.
fn same_keys(theirs: &Value, ours: &Value, keys: &[&str]) -> Outcome {
    let pick = |line: &Value| {
        let pairs = keys.iter().map(|k| ((*k).to_owned(), line[*k].clone()));
        Value::Object(pairs.collect())
    };
    equal(&pick(theirs), &pick(ours))
}

/// `Pass` when `ours` equals `theirs`, else the differences.
fn equal(theirs: &Value, ours: &Value) -> Outcome {
    if theirs == ours {
        Outcome::Pass
    } else {
        Outcome::Mismatch(crate::compare::diff(theirs, ours))
    }
}

/// The keys of a start's line that a restart shares with the chain's line at its height.
fn state_keys() -> Vec<&'static str> {
    ["h", "id"].into_iter().chain(SAME_STATE).collect()
}

/// Whether the reference's start after a rollback to `target` shows the older-saved-state
/// difference: it started at `kept`, the height of the state it kept below the target, and its
/// line there, `theirs`, is the judge's line at that height, `judge`.
pub fn older_saved_state(kept: u32, target: u32, theirs: &Value, judge: &Value) -> bool {
    let at_kept = theirs["h"].as_u64() == Some(u64::from(kept));
    let same = matches!(same_keys(judge, theirs, &state_keys()), Outcome::Pass);
    kept < target && at_kept && same
}

/// Apply the corpus's block at `height` in the oracle's `session`: its line.
fn oracle_apply(
    server: &mut OracleServer,
    session: &Value,
    corpus: &Corpus,
    height: u32,
) -> Result<Value> {
    let block = corpus
        .block(height)
        .ok_or_else(|| Error::Oracle(format!("the corpus has no block {height}")))?;
    let args = json!({"session": session, "hex": hex::encode(&block.bytes)});
    let applied = call(server, "chain.apply", 1, &args)?;
    if applied["accepted"] != Value::Bool(true) {
        return Err(Error::Oracle(format!(
            "the oracle refused block {height}: {applied}"
        )));
    }
    Ok(applied["line"].clone())
}

/// A rollback with `flags` on both sides from the tip, the start after it, the next block, and
/// Heartwood's re-sync.
fn equal_start(
    op: &str,
    corpus: &Corpus,
    flags: Flags,
    server: &mut OracleServer,
    out: &mut Results,
) -> Result<()> {
    let tip = match TipStore::applied(corpus) {
        Ok(tip) => tip,
        Err(problem) => {
            out.push(
                op,
                "Heartwood's store at the tip",
                corpus.tip(),
                problem.outcome(),
            );
            return Ok(());
        }
    };
    let forward = store::forward_lines(corpus)?;
    let session = oracle_open(server, corpus)?;
    let theirs = oracle_rollback(server, &session, flags, "rebuild")?;
    let ours = roll_back(&tip, flags)?;
    let target = u32::try_from(theirs["rollback"]["target"].as_u64().unwrap_or(0)).unwrap_or(0);
    let what = format!("{}: the rollback", flags.name());
    out.push(op, &what, target, equal(&theirs["rollback"], &ours.answer));
    if theirs.get("active").is_none() || !ours.rolled {
        call(server, "chain.close", 1, &json!({"session": session}))?;
        let why = Outcome::HarnessError(format!("no start to compare: {theirs}"));
        out.push(op, "the start after it", target, why);
        return Ok(());
    }
    let started = match start_after(&ours.scratch, &tip.files, &tip.chain_digest) {
        Ok(started) => started,
        Err(problem) => {
            call(server, "chain.close", 1, &json!({"session": session}))?;
            out.push(op, "Heartwood's start after it", target, problem.outcome());
            return Ok(());
        }
    };
    let replayed = started.path
        == heartwood_blockchain::RestorePath::Replay(heartwood_blockchain::ReplayReason::NoState);
    let path = if replayed {
        Outcome::Pass
    } else {
        Outcome::HarnessError(format!("the start took the path {:?}", started.path))
    };
    out.push(
        op,
        "Heartwood's start replays for want of a state",
        target,
        path,
    );
    let line = &started.restart["line"];
    let judge = forward.get(&target).cloned().unwrap_or(Value::Null);
    let at = format!("the start's line at {target} is the judge's");
    out.push(op, &at, target, same_keys(&judge, line, &state_keys()));
    let both = equal(&theirs["restart"]["line"], line);
    out.push(
        op,
        "the starts' lines, the running digest included",
        target,
        both,
    );
    let rows = equal(&theirs["active"], &started.active);
    out.push(
        op,
        "the next round's rows the starts saved, as sets",
        target,
        rows,
    );
    let summaries = equal(&theirs["restart"]["summary"], &started.restart["summary"]);
    out.push(
        op,
        "the stores' summaries after the starts",
        target,
        summaries,
    );
    let next = target.saturating_add(1);
    let theirs_next = oracle_apply(server, &session, corpus, next)?;
    call(server, "chain.close", 1, &json!({"session": session}))?;
    let resynced = resync(started.node, corpus, &ours.scratch)?;
    let ours_next = resynced
        .lines
        .first()
        .map_or(Value::Null, digests::line_value);
    let next_block = format!("block {next}, the next after the start, on both sides");
    out.push(
        op,
        &next_block,
        next,
        same_keys(&theirs_next, &ours_next, &SAME_STEP),
    );
    let mismatch = resynced
        .lines
        .iter()
        .map(digests::line_value)
        .find_map(|line| {
            let height = u32::try_from(line["h"].as_u64().unwrap_or(0)).unwrap_or(0);
            let judge = forward.get(&height).cloned().unwrap_or(Value::Null);
            match same_keys(&judge, &line, &SAME_STEP) {
                Outcome::Pass => None,
                other => Some(other),
            }
        });
    let resync_outcome = match (&resynced.refused, mismatch) {
        (Some((height, why)), _) => Outcome::HarnessError(format!("block {height}: {why}")),
        (None, Some(outcome)) => outcome,
        (None, None) => Outcome::Pass,
    };
    out.push(
        op,
        "Heartwood's re-sync to the tip, every line the judge's",
        corpus.tip(),
        resync_outcome,
    );
    let round_length = u32::try_from(corpus.milestones.at(1).active_delegates()).unwrap_or(0);
    let last_round = store::last_round(corpus.tip(), round_length);
    match store::read_rows(resynced.store.database(), corpus.tip(), last_round) {
        Ok(rows) => {
            for (label, outcome) in store::anchors(&corpus.dir, &rows, corpus.tip(), last_round) {
                out.push(
                    op,
                    &format!("after the re-sync, {label}"),
                    corpus.tip(),
                    outcome,
                );
            }
        }
        Err(why) => out.push(
            op,
            "the rows after the re-sync",
            corpus.tip(),
            Outcome::HarnessError(why),
        ),
    }
    Ok(())
}

/// Heartwood's pool of a case: its directory, its options and the pool.
struct PoolSide {
    scratch: ScratchDir,
    pool: Pool,
}

impl PoolSide {
    fn open(scratch: ScratchDir) -> Result<PoolSide> {
        let options = options(&json!({}), scratch.path()).map_err(Error::Oracle)?;
        let pool = Pool::open(options, None).map_err(|e| Error::Oracle(format!("pool: {e}")))?;
        Ok(PoolSide { scratch, pool })
    }

    /// The pool opened again from its storage, re-added against `state`, as a node's start
    /// re-adds it; its contents.
    fn restart(&mut self, state: &State, corpus: &Corpus) -> Result<Value> {
        let options = options(&json!({}), self.scratch.path()).map_err(Error::Oracle)?;
        let fresh = Pool::open(options, None).map_err(|e| Error::Oracle(format!("pool: {e}")))?;
        drop(std::mem::replace(&mut self.pool, fresh));
        let online = DelegateVersions::new();
        let ctx = context(state, corpus, &online);
        (self.pool.readd(&ctx, &[])).map_err(|e| Error::Oracle(format!("pool: {e}")))?;
        contents(&self.pool).map_err(Error::Oracle)
    }
}

fn context<'a>(
    state: &'a State,
    corpus: &'a Corpus,
    online: &'a DelegateVersions,
) -> PoolContext<'a> {
    PoolContext {
        wallets: state.wallets(),
        tip: state.height(),
        milestones: &corpus.milestones,
        network: &corpus.network,
        online,
    }
}

/// `rollback-stored-pool`: transfers in both pools, a rollback to `height`, the start after it,
/// and both pools' re-add.
fn stored_pool(
    corpus: &Corpus,
    height: i64,
    server: &mut OracleServer,
    out: &mut Results,
) -> Result<()> {
    const OP: &str = "rollback-stored-pool";
    let keys = corpus_keys(corpus)?;
    let s = Sides::open(corpus, &keys, server, OP, false, None, Some(json!({})))?;
    let funded: Vec<_> = keys
        .wallets()
        .filter(|k| s.funded(k, 10_000_000_000))
        .take(2)
        .collect();
    let recipients: Vec<_> = keys.wallets().map(|k| k.address).collect();
    let mut batch: Vec<Transaction> = Vec::new();
    for (i, from) in funded.iter().enumerate() {
        for n in 0..2_u64 {
            let to = recipients[(i * 2 + usize::try_from(n).unwrap_or(0) + 5) % recipients.len()];
            let builder = transfer(&[(to, 1_000 + n)]).nonce(s.nonce(&from.address) + 1 + n);
            batch.push(s.build(builder.fee(FEE), from)?);
        }
    }
    if batch.len() < 4 {
        return Err(Error::Oracle(format!(
            "{OP}: fewer than two funded wallets"
        )));
    }
    let now = 1_800_000_000_000_i64;
    let entries: Vec<Value> = batch
        .iter()
        .map(|t| json!({"hex": hex::encode(t.bytes())}))
        .collect();
    let session = s.session.clone();
    let process = json!({"session": session, "entries": entries, "now": now});
    let theirs_result = call(server, "pool.process", 1, &process)?;
    let theirs_contents =
        call(server, "pool.contents", 1, &json!({"session": session}))?["contents"].clone();
    let mut side = PoolSide::open(ScratchDir::new("rollback-pool")?)?;
    let online = DelegateVersions::new();
    let clock = ManualClock::new(now);
    let entries_ours = batch
        .iter()
        .map(|t| Entry::Bytes(t.bytes().to_vec()))
        .collect();
    let ctx = context(&s.state, corpus, &online);
    let ours_result = Processor::new()
        .process(&mut side.pool, &ctx, &clock, entries_ours)
        .map_err(|e| Error::Oracle(format!("pool: {e}")))?;
    let step = json!({"op": "process", "entries": entries, "now": now});
    let before = judge(
        &json!({"steps": [step]}),
        &json!({"results": [{"result": theirs_result, "contents": theirs_contents}]}),
        &json!({"results": [{"result": ours_result.to_json(), "contents": contents(&side.pool).map_err(Error::Oracle)?}]}),
    );
    out.push(
        OP,
        "four transfers in both pools at the tip",
        corpus.tip(),
        before,
    );
    let flags = Flags::height(height);
    let theirs = oracle_rollback(server, &session, flags, "rebuild")?;
    let theirs_after =
        call(server, "pool.contents", 1, &json!({"session": session}))?["contents"].clone();
    call(server, "chain.close", 1, &json!({"session": session}))?;
    let tip = TipStore::applied(corpus).map_err(|p| Error::Oracle(format!("{p:?}")))?;
    let ours = roll_back(&tip, flags)?;
    let started = start_after(&ours.scratch, &tip.files, &tip.chain_digest)
        .map_err(|p| Error::Oracle(format!("{p:?}")))?;
    let target = started.node.state.height();
    out.push(
        OP,
        &format!("{}: the rollback", flags.name()),
        target,
        equal(&theirs["rollback"], &ours.answer),
    );
    let ours_after = side.restart(&started.node.state, corpus)?;
    let readd = judge(
        &json!({"steps": [{"op": "restart"}]}),
        &json!({"results": [{"result": {}, "contents": theirs_after}]}),
        &json!({"results": [{"result": {}, "contents": ours_after}]}),
    );
    out.push(
        OP,
        "both pools re-added against the rolled-back state",
        target,
        readd,
    );
    Ok(())
}

/// `rollback-older-saved-state`: states kept on the reference's side 12 blocks below the target
/// and at the tip's three blocks, then `--height tip - 1`.
fn older_state(corpus: &Corpus, server: &mut OracleServer, out: &mut Results) -> Result<()> {
    const OP: &str = "rollback-older-saved-state";
    let tip_height = corpus.tip();
    let flags = Flags::height(i64::from(tip_height) - 1);
    let tip = TipStore::applied(corpus).map_err(|p| Error::Oracle(format!("{p:?}")))?;
    let forward = store::forward_lines(corpus)?;
    let plan =
        heartwood_blockchain::rollback::plan(flags.height, None, tip_height, &corpus.milestones);
    let target = plan
        .map_err(|r| Error::Oracle(format!("{OP}: refused: {r:?}")))?
        .target;
    let kept = target.saturating_sub(12);
    let opened = call(
        server,
        "chain.open",
        1,
        &json!({"crypto": corpus_crypto(corpus)?}),
    )?;
    let session = opened["session"].clone();
    for height in 2..=tip_height {
        oracle_apply(server, &session, corpus, height)?;
        if height == kept || height + 2 >= tip_height {
            call(server, "chain.saveState", 1, &json!({"session": session}))?;
        }
    }
    let rollback = call(
        server,
        "chain.rollback",
        1,
        &flags.with(json!({"session": session})),
    )?;
    let restart = call(
        server,
        "chain.restart",
        1,
        &json!({"session": session, "mode": "kept"}),
    )?;
    let theirs_line = restart["line"].clone();
    let judge = forward.get(&kept).cloned().unwrap_or(Value::Null);
    let outcome = if older_saved_state(kept, target, &theirs_line, &judge) {
        Outcome::Documented(OLDER_SAVED_STATE.to_owned())
    } else {
        Outcome::HarnessError(format!(
            "the reference's start is not the documented one: {restart}"
        ))
    };
    out.push(
        OP,
        &format!("the reference's start at the kept state's height {kept}"),
        kept,
        outcome,
    );
    let ours = roll_back(&tip, flags)?;
    out.push(
        OP,
        &format!("{}: the rollback", flags.name()),
        target,
        equal(&rollback, &ours.answer),
    );
    let started = start_after(&ours.scratch, &tip.files, &tip.chain_digest)
        .map_err(|p| Error::Oracle(format!("{p:?}")))?;
    let at = forward.get(&target).cloned().unwrap_or(Value::Null);
    let ours_line = started.restart["line"].clone();
    out.push(
        OP,
        &format!("Heartwood's start at {target}, the judge's line"),
        target,
        same_keys(&at, &ours_line, &state_keys()),
    );
    let mut theirs_last = Value::Null;
    let from = theirs_line["h"]
        .as_u64()
        .and_then(|h| u32::try_from(h).ok())
        .unwrap_or(tip_height);
    for height in from.saturating_add(1)..=tip_height {
        theirs_last = oracle_apply(server, &session, corpus, height)?;
    }
    let theirs_summary = call(
        server,
        "chain.restart",
        1,
        &json!({"session": session, "mode": "rebuild"}),
    )?["summary"]
        .clone();
    call(server, "chain.close", 1, &json!({"session": session}))?;
    let resynced = resync(started.node, corpus, &ours.scratch)?;
    let ours_last = resynced
        .lines
        .last()
        .map_or(Value::Null, digests::line_value);
    out.push(
        OP,
        "both re-synced: the lines at the tip",
        tip_height,
        same_keys(&theirs_last, &ours_last, &SAME_STEP),
    );
    let ours_summary = store_summary::summary(ours.scratch.path())?;
    out.push(
        OP,
        "both re-synced: the stores' summaries",
        tip_height,
        equal(&theirs_summary, &ours_summary),
    );
    Ok(())
}

/// A case: its run on a corpus against the oracle, into the results.
type Case = fn(&Corpus, &mut OracleServer, &mut Results) -> Result<()>;

/// Run the rollback cases on top of `corpus` against the oracle `server`.
pub fn run(corpus: &Corpus, server: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let mut out = Results {
        file: format!("{}/synthetic", corpus.name()),
        results: Vec::new(),
    };
    let tip = corpus.tip();
    let cases: [(&str, Case); 4] = [
        ("rollback-last-round", |c, s, o| {
            let flags = Flags::height(i64::from(c.tip()) - 1);
            equal_start("rollback-last-round", c, flags, s, o)
        }),
        ("rollback-no-missed-row", |c, s, o| {
            equal_start("rollback-no-missed-row", c, Flags::height(133), s, o)
        }),
        ("rollback-stored-pool", |c, s, o| stored_pool(c, 133, s, o)),
        ("rollback-older-saved-state", older_state),
    ];
    for (name, case) in cases {
        if tip < 160 {
            out.push(
                name,
                "the chain is too short for the case",
                tip,
                Outcome::Skipped("a tip below 160".into()),
            );
            continue;
        }
        if let Err(e) = case(corpus, server, &mut out) {
            out.push(name, "the case", tip, Outcome::HarnessError(e.to_string()));
        }
    }
    Ok(out.results)
}
