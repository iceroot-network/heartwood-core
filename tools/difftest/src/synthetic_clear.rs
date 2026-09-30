//! The synthetic clear cases on a corpus chain's tip, which `run l2 DIR --synthetic` runs with the
//! other synthetic cases: `state clear` and `pool clear`, then a start, on both sides, with the
//! node's directories made with mode 0700 and again under a umask of 002.
//!
//! - `clear-state`: the command writes its marker, the relay's start honours it and restores the
//!   state by a replay of the stored chain, and the reference restarts without a saved state. The
//!   two restart lines equal the line of the state before the command, and the online record
//!   (the delegates' announced versions, which a reference node keeps in memory only) is the one
//!   the store held before the command.
//! - `clear-pool`: a transfer in the pool of each side, the command's marker, and a start: the
//!   reference's pool flushed, Heartwood's opened with the marker. Both pools are empty, also
//!   after their storage is opened again, and the chain's line is the one it was.
//!
//! The directories of the first layout are the command's own (the temp directory is made by it,
//! with mode 0700); those of the second are made by a child process under a umask of 002, so that
//! they have mode 0775, as under a default Ubuntu or Debian account. The relay's start must
//! honour the marker in both.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use heartwood_blockchain::RestoreMode;
use heartwood_cli::instance::{InstanceFlags, resolve};
use heartwood_cli::recovery::{Marker, request};
use heartwood_cli::seams::{Instance, Role};
use heartwood_core::markers::honoured;
use heartwood_crypto::Block;
use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_kernel::clock::ManualClock;
use heartwood_pool::{DelegateVersions, Entry, Pool, PoolContext, Processor};
use heartwood_state::State;
use heartwood_state::digest::{self, Step};
use heartwood_transactions::{Handlers, OnlineDelegates};
use serde_json::{Value, json};

use crate::corpus::Corpus;
use crate::errors::{Error, Result};
use crate::levels::l2::call;
use crate::levels::pool::{VERSION, contents, judge, options};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};
use crate::restart::{HeartwoodMode, differing_keys};
use crate::store::{self, ScratchDir, StartProblem, StoreNode, guarded};
use crate::synthetic::{FEE, Keys, Sides, corpus_keys, transfer};

/// The keys of a restart line that a restart must give as the state before it.
const LINE_KEYS: [&str; 9] = ["h", "step", "id", "b", "s", "r", "m", "w", "sd"];
/// The time of the case's transfer, in milliseconds.
const NOW_MS: i64 = 1_800_000_000_000;

/// How the node's directories are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// The command makes the temp directory itself: mode 0700.
    Private,
    /// A child process makes them under a umask of 002: mode 0775.
    Group,
}

impl Layout {
    const ALL: [Layout; 2] = [Layout::Private, Layout::Group];

    const fn state_case(self) -> &'static str {
        match self {
            Layout::Private => "clear-state",
            Layout::Group => "clear-state-umask-002",
        }
    }

    const fn pool_case(self) -> &'static str {
        match self {
            Layout::Private => "clear-pool",
            Layout::Group => "clear-pool-umask-002",
        }
    }

    /// The mode the temp directory must have after the command.
    const fn temp_mode(self) -> u32 {
        match self {
            Layout::Private => 0o700,
            Layout::Group => 0o775,
        }
    }
}

/// A node's directories under a scratch directory, and the relay instance a command resolves.
struct Dirs {
    _scratch: ScratchDir,
    data: PathBuf,
    temp: PathBuf,
    instance: Instance,
}

impl Dirs {
    fn new(layout: Layout) -> Result<Dirs> {
        let scratch = ScratchDir::new("synthetic-clear")?;
        let root = scratch.path().to_path_buf();
        let (data, temp) = (root.join("data"), root.join("tmp/node/devnet"));
        let run = data.join("run");
        match layout {
            Layout::Private => {
                let made = fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&run);
                made.map_err(|e| Error::io(&run, e))?;
            }
            Layout::Group => {
                for dir in [&run, &temp] {
                    mkdir_umask_002(dir)?;
                }
            }
        }
        let vars = [
            ("CORE_PATH_DATA", data.display().to_string()),
            ("CORE_PATH_TEMP", temp.display().to_string()),
        ];
        let env = |name: &str| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
        };
        let flags = InstanceFlags {
            token: Some("difftest".into()),
            network: Some("devnet".into()),
        };
        let instance = resolve(Role::Relay, &flags, &env, &root, &root)
            .map_err(|e| Error::Usage(e.to_string()))?;
        Ok(Dirs {
            _scratch: scratch,
            data,
            temp,
            instance,
        })
    }

    /// The marker's mode and its directory's, and whether the relay honours it.
    fn written(&self, marker: &Path) -> (u32, u32, bool) {
        let mode = |path: &Path| fs::metadata(path).map_or(0, |m| m.mode() & 0o777);
        (mode(marker), mode(&self.temp), honoured(marker, &self.data))
    }
}

/// `dir` made with a umask of 002, in a child process so that no other thread sees the umask.
fn mkdir_umask_002(dir: &Path) -> Result<()> {
    let made = Command::new("sh")
        .args(["-c", "umask 002 && mkdir -p \"$1\"", "sh"])
        .arg(dir)
        .status()
        .map_err(|e| Error::io(dir, e))?;
    if made.success() {
        Ok(())
    } else {
        Err(Error::Usage(format!("mkdir -p {} failed", dir.display())))
    }
}

/// The restart line of `state`: what every restart of it must give.
fn restart_line(state: &State, s: &Sides<'_>) -> Value {
    let line = digest::height_digest(state, &s.milestones, Step::Restart, &[], None, &[0; 32]);
    crate::digests::line_value(&line)
}

/// The outcome of a restart line `got` against the line `want`.
fn same_line(got: &Value, want: &Value) -> Outcome {
    if differing_keys(got, want, &LINE_KEYS).is_empty() {
        Outcome::Pass
    } else {
        Outcome::Mismatch(crate::compare::diff(want, got))
    }
}

/// `holds` as an outcome that says `what` when it does not.
fn expect(holds: bool, what: impl FnOnce() -> String) -> Outcome {
    if holds {
        Outcome::Pass
    } else {
        Outcome::HarnessError(what())
    }
}

fn failed(what: impl std::fmt::Display) -> StartProblem {
    StartProblem::Failed(what.to_string())
}

/// What the command's marker looks like and whether the relay reads it, as outcomes.
fn marker_outcomes(dirs: &Dirs, marker: &Path, layout: Layout) -> Vec<(&'static str, Outcome)> {
    let (file, dir, read) = dirs.written(marker);
    vec![
        (
            "the marker is a private file",
            expect(file == 0o600, || format!("its mode is {file:o}")),
        ),
        (
            "the temp directory has the mode of its layout",
            expect(dir == layout.temp_mode(), || format!("its mode is {dir:o}")),
        ),
        (
            "the relay's start honours the marker",
            expect(read, || "the relay ignored the marker".into()),
        ),
    ]
}

/// Heartwood's store at the tip of `corpus` under `dirs`, its state, and the lines to compare.
fn build_store(corpus: &Corpus, dirs: &Dirs) -> std::result::Result<StoreNode, StartProblem> {
    let files = store::crypto_files(corpus)?;
    let mut node = StoreNode::start(&dirs.data, &files, RestoreMode::Auto)?;
    let milestones = corpus.milestones.clone();
    for block in &corpus.blocks {
        let decoded = Block::receive(&block.bytes, &milestones)
            .map_err(|e| failed(format!("block {}: {e}", block.height)))?;
        let applied = (node.state.apply_block(&decoded, &milestones, &Handlers))
            .map_err(|e| failed(format!("Heartwood refused block {}: {e}", block.height)))?;
        let chunk = guarded(
            || -> std::result::Result<(), heartwood_blockchain::StoreError> {
                let mut writer = node.store.chunk()?;
                writer.applied(&node.state, &decoded, &applied, &milestones)?;
                writer.commit()
            },
        )?;
        chunk.map_err(|e| failed(format!("the store refused a chunk: {e}")))?;
    }
    Ok(node)
}

/// A delegate's name and public key in `state`.
fn a_delegate(state: &State) -> Option<(String, PublicKeyBytes)> {
    (state.wallets().delegates()).find_map(|w| {
        Some((
            w.delegate()?.username().to_owned(),
            w.public_key()?.to_compressed(),
        ))
    })
}

/// Heartwood's side of `clear-state`: the outcomes of the store, the command and the start.
fn heartwood_state_clear(
    s: &Sides<'_>,
    dirs: &Dirs,
    layout: Layout,
    want: &Value,
) -> std::result::Result<Vec<(&'static str, Outcome)>, StartProblem> {
    let mut node = build_store(s.corpus, dirs)?;
    let mut out = vec![(
        "Heartwood's store at the tip",
        same_line(&restart_line(&node.state, s), want),
    )];
    let (name, key) = a_delegate(&node.state).ok_or_else(|| failed("the chain has no delegate"))?;
    let mut record = DelegateVersions::new();
    record.set(key, 1, VERSION.into());
    record.set(PublicKeyBytes::from_array([2; 33]), 2, "4.3.0".into());
    let saved = guarded(|| node.store.save_online(&record))?;
    saved.map_err(|e| failed(format!("the online record was not saved: {e}")))?;
    let marker = request(
        &dirs.instance,
        Marker::ClearSavedStates,
        true,
        &mut std::io::stdin(),
    )
    .map_err(failed)?;
    out.extend(marker_outcomes(dirs, &marker, layout));
    let mode = if honoured(&marker, &dirs.data) {
        RestoreMode::Replay
    } else {
        RestoreMode::Auto
    };
    let node = node.restart(mode)?;
    let _ = fs::remove_file(&marker);
    out.push((
        "Heartwood's start by a replay",
        expect(
            store::took_its_path(HeartwoodMode::Replay, &node.path),
            || format!("the start took the path {:?}", node.path),
        ),
    ));
    out.push((
        "Heartwood's line after the replay",
        same_line(&restart_line(&node.state, s), want),
    ));
    let kept = guarded(|| node.store.online())?.map_err(failed)?;
    out.push((
        "the online record is kept",
        expect(kept == record, || "the record changed".into()),
    ));
    let online = kept.view(node.state.wallets()).is_online(&name);
    out.push((
        "the delegate is still online for the pool's vote check",
        expect(online, || format!("{name} is not online")),
    ));
    Ok(out)
}

/// `clear-state` for `layout`.
fn state_clear(
    corpus: &Corpus,
    keys: &Keys,
    layout: Layout,
    server: &mut OracleServer,
) -> Result<Vec<CaseResult>> {
    let mut s = Sides::open(corpus, keys, server, layout.state_case(), false, None, None)?;
    let want = restart_line(&s.state, &s);
    let dirs = Dirs::new(layout)?;
    match heartwood_state_clear(&s, &dirs, layout, &want) {
        Ok(steps) => steps
            .into_iter()
            .for_each(|(what, outcome)| s.record(what, outcome)),
        Err(problem) => s.record("Heartwood's clear", problem.outcome()),
    }
    let restarted = call(
        server,
        "chain.restart",
        1,
        &json!({"session": s.session, "mode": "rebuild"}),
    )?;
    let outcome = match restarted.pointer("/refused/reason").and_then(Value::as_str) {
        Some(reason) => Outcome::HarnessError(format!("the reference refused to start: {reason}")),
        None => same_line(restarted.get("line").unwrap_or(&Value::Null), &want),
    };
    s.record("the reference's start without a saved state", outcome);
    Ok(s.results)
}

/// Heartwood's side of `clear-pool`: the transfer pooled, the command's marker, the pool opened
/// with it and again without it. The oracle's contents after each step are `flushed` and
/// `restarted`.
fn heartwood_pool_clear(
    s: &Sides<'_>,
    dirs: &Dirs,
    layout: Layout,
    transfer_bytes: &[u8],
    (flushed, restarted): (&Value, &Value),
) -> std::result::Result<Vec<(&'static str, Outcome)>, StartProblem> {
    let opened = options(&json!({}), &dirs.data).map_err(failed)?;
    let online = DelegateVersions::new();
    let ctx = PoolContext {
        wallets: s.state.wallets(),
        tip: s.state.height(),
        milestones: &s.milestones,
        network: &s.corpus.network,
        online: &online,
    };
    let stored_id = guarded(|| -> std::result::Result<String, String> {
        let mut pool = Pool::open(opened.clone(), None).map_err(|e| e.to_string())?;
        let batch = vec![Entry::Bytes(transfer_bytes.to_vec())];
        let clock = ManualClock::new(NOW_MS);
        Processor::new()
            .process(&mut pool, &ctx, &clock, batch)
            .map_err(|e| e.to_string())?;
        Ok(contents(&pool)?["all"].to_string())
    })?
    .map_err(failed)?;
    let mut out = vec![(
        "Heartwood's pool holds the transfer",
        expect(stored_id.contains('"'), || {
            "the transfer was not admitted".into()
        }),
    )];
    let marker = request(
        &dirs.instance,
        Marker::ClearPool,
        true,
        &mut std::io::stdin(),
    )
    .map_err(failed)?;
    out.extend(marker_outcomes(dirs, &marker, layout));
    let marker_to_use = Some(marker.clone()).filter(|m| honoured(m, &dirs.data));
    let judged = |op: &str, theirs: &Value, ours: Value| {
        let input = json!({"steps": [{"op": op}]});
        let theirs = json!({"results": [{"result": {}, "contents": theirs}]});
        judge(
            &input,
            &theirs,
            &json!({"results": [{"result": {}, "contents": ours}]}),
        )
    };
    let mut pool =
        guarded(|| Pool::open(opened.clone(), marker_to_use.as_deref()))?.map_err(failed)?;
    out.push((
        "the marker is removed by the start",
        expect(!marker.exists(), || "the marker is still there".into()),
    ));
    let pooled = guarded(|| contents(&pool))?.map_err(failed)?;
    out.push((
        "Heartwood's pool after the start",
        judged("flush", flushed, pooled),
    ));
    drop(pool);
    pool = guarded(|| Pool::open(opened.clone(), None))?.map_err(failed)?;
    guarded(|| pool.readd(&ctx, &[]))?.map_err(failed)?;
    let again = guarded(|| contents(&pool))?.map_err(failed)?;
    out.push((
        "Heartwood's pool after its storage is opened again",
        judged("restart", restarted, again),
    ));
    Ok(out)
}

/// `clear-pool` for `layout`.
fn pool_clear(
    corpus: &Corpus,
    keys: &Keys,
    layout: Layout,
    server: &mut OracleServer,
) -> Result<Vec<CaseResult>> {
    let pool = Some(json!({}));
    let mut s = Sides::open(corpus, keys, server, layout.pool_case(), false, None, pool)?;
    let want = restart_line(&s.state, &s);
    let from = (keys.wallets().find(|k| s.funded(k, 10_000_000_000)))
        .ok_or_else(|| Error::Oracle("no funded wallet".into()))?;
    let to = (keys.wallets().find(|k| k.address != from.address))
        .ok_or_else(|| Error::Oracle("no second wallet".into()))?;
    let builder = transfer(&[(to.address, 1_000)])
        .nonce(s.nonce(&from.address) + 1)
        .fee(FEE);
    let tx = s.build(builder, from)?;
    let session = s.session.clone();
    let processed =
        json!({"session": session, "entries": [{"hex": hex(tx.bytes())}], "now": NOW_MS});
    call(server, "pool.process", 1, &processed)?;
    let id = tx.id().to_hex();
    let held = call(server, "pool.contents", 1, &json!({"session": session}))?;
    let holds = held["contents"]["all"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| *v == id));
    s.premise(holds, "the reference's pool holds the transfer");
    // The reference's start flushes the pool's storage when the marker is there.
    let flushed = call(server, "pool.flush", 1, &json!({"session": session}))?;
    let restarted = call(server, "pool.restart", 1, &json!({"session": session}))?;
    let dirs = Dirs::new(layout)?;
    let theirs = (&flushed["contents"], &restarted["contents"]);
    match heartwood_pool_clear(&s, &dirs, layout, tx.bytes(), theirs) {
        Ok(steps) => steps
            .into_iter()
            .for_each(|(what, outcome)| s.record(what, outcome)),
        Err(problem) => s.record("Heartwood's pool clear", problem.outcome()),
    }
    let line = call(
        server,
        "chain.restart",
        1,
        &json!({"session": session, "mode": "rebuild"}),
    )?;
    let outcome = match line.pointer("/refused/reason").and_then(Value::as_str) {
        Some(reason) => Outcome::HarnessError(format!("the reference refused to start: {reason}")),
        None => same_line(line.get("line").unwrap_or(&Value::Null), &want),
    };
    s.record("the chain's line after the pool clear", outcome);
    Ok(s.results)
}

fn hex(bytes: &[u8]) -> String {
    heartwood_crypto::utils::hex::encode(bytes)
}

/// Run the clear cases on top of `corpus` against the oracle `server`.
pub fn run(corpus: &Corpus, server: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let keys = corpus_keys(corpus)?;
    let mut out = Vec::new();
    for layout in Layout::ALL {
        out.extend(state_clear(corpus, &keys, layout, server)?);
        out.extend(pool_clear(corpus, &keys, layout, server)?);
    }
    Ok(out)
}
