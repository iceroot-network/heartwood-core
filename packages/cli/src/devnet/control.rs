//! Starting, stopping, restarting and resetting a devnet's nodes, the epoch re-stamp, and
//! waiting for a height.

use std::fs;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use heartwood_crypto::crypto::slots;
use heartwood_kernel::Clock;
use serde_json::Value;

use crate::devnet::manifest::{crypto_files, hashes, io, remove, write};
use crate::devnet::report::node_env;
use crate::devnet::{Manifest, ManifestNode, NodeSelection, Result, Tools, epoch_after, test_bind};
use crate::errors::{DevnetError, HttpError, ProcessError};
use crate::seams::{ProcessOps, Role};

/// The polls of the relays' APIs after they start, one a second: 30 s at most.
const API_POLLS: u64 = 30;
/// The pause between two polls of `wait`.
const WAIT_POLL: Duration = Duration::from_secs(2);
/// A start re-stamps the epoch when it is less than this far away, or past.
const RESTAMP_WITHIN_MS: i64 = 10_000;
/// A relay's chain store, under its data directory: a node that has one may hold blocks.
const STORE: &str = "database/blockchain.db";
/// The forger's signing record directory, under its data directory.
const RECORD_DIR: &str = "forger";
/// Roles of nodes to act on, in order.
type Targets = Vec<(u32, Role)>;
/// The roles in the order a start acts on them, and a stop in the other.
const START: [Role; 2] = Role::ALL;
const STOP: [Role; 2] = [Role::Forger, Role::Relay];

/// Start the selected nodes: the relays, then, once their APIs answer (30 s at most), the
/// forgers. Refused when a crypto file differs from the manifest, a node's `.env` binds a
/// listener outside loopback, a selected role runs, or a relay's address is taken. Before any
/// process starts, the epoch is re-stamped `epoch_delay` seconds after the clock's time when it
/// is less than 10 s away or past, no process of the devnet runs and no node has a chain store;
/// the re-stamp also removes every forger's signing record.
pub fn start(root: &Path, selection: &NodeSelection, tools: &Tools<'_>) -> Result<()> {
    let (root, mut manifest) = open(root)?;
    let (picked, role) = (manifest.select(selection)?, selection.role);
    preflight(&root, &manifest, &picked)?;
    for (k, role) in targets(&manifest, &picked, role, START)? {
        let instance = manifest.node(k)?.instance(role);
        if tools.ops.status(&instance)?.status.is_running() {
            let name = instance.name();
            return Err(ProcessError::AlreadyRunning { name }.into());
        }
    }
    if restamp_due(&root, &manifest, tools.ops, tools.clock)? {
        restamp(&root, &mut manifest, tools.clock)?;
    }
    launch(&manifest, &picked, role, tools)
}

/// Start node `node` (`start-node`): [`start`] of that node alone.
pub fn start_node(root: &Path, node: u32, role: Option<Role>, tools: &Tools<'_>) -> Result<()> {
    let nodes = vec![node];
    start(root, &NodeSelection { nodes, role }, tools)
}

/// Stop the selected nodes, forgers before relays; with `kill`, by SIGKILL, waiting for the exit,
/// then recording the stop. A role that does not run is skipped. Every targeted role is tried;
/// the first failure is the error.
pub fn stop(root: &Path, selection: &NodeSelection, kill: bool, tools: &Tools<'_>) -> Result<()> {
    let (_, manifest) = open(root)?;
    let picked = manifest.select(selection)?;
    stop_nodes(&manifest, &picked, selection.role, kill, tools.ops)
}

/// Stop node `node` (`stop-node`): [`stop`] of that node alone.
pub fn stop_node(root: &Path, k: u32, role: Option<Role>, kill: bool, tools: &Tools) -> Result<()> {
    let nodes = vec![k];
    stop(root, &NodeSelection { nodes, role }, kill, tools)
}

/// Restart node `node` gracefully (`restart-node`): the forger stops before the relay, the relay
/// starts before the forger, each with its environment read again. Refused, before anything
/// stops, when a selected role does not run or [`start`]'s checks fail. The epoch never moves.
pub fn restart_node(root: &Path, node: u32, role: Option<Role>, tools: &Tools<'_>) -> Result<()> {
    let (root, manifest) = open(root)?;
    let picked = [manifest.node(node)?.index];
    preflight(&root, &manifest, &picked)?;
    for (k, role) in targets(&manifest, &picked, role, START)? {
        let instance = manifest.node(k)?.instance(role);
        if !tools.ops.status(&instance)?.status.is_running() {
            let name = instance.name();
            return Err(ProcessError::NotRunning { name }.into());
        }
    }
    stop_nodes(&manifest, &picked, role, false, tools.ops)?;
    launch(&manifest, &picked, role, tools)
}

/// Stop node `node`, then delete its data, logs, temp and cache. The forger's signing record
/// stays, since the node's chain does not change: only the epoch re-stamp removes it.
pub fn reset(root: &Path, node: u32, tools: &Tools<'_>) -> Result<()> {
    let (_, manifest) = open(root)?;
    let paths = manifest.node(node)?.paths();
    stop_nodes(&manifest, &[node], None, false, tools.ops)?;
    for dir in [&paths.data, &paths.log, &paths.temp, &paths.cache] {
        let entries = match fs::read_dir(dir) {
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            entries => entries.map_err(io(dir))?,
        };
        for entry in entries {
            let entry = entry.map_err(io(dir))?;
            if dir != &paths.data || entry.file_name() != RECORD_DIR {
                remove(&entry.path())?;
            }
        }
    }
    Ok(())
}

/// Wait until node `node`'s relay, or with `None` every relay whose API answers (one at least),
/// reports a height of `height` or more, polling every 2 s for at most `timeout`. Gives the
/// lowest height reported.
pub fn wait(
    root: &Path,
    height: u32,
    node: Option<u32>,
    timeout: Duration,
    tools: &Tools,
) -> Result<u32> {
    let (_, manifest) = open(root)?;
    let relays = match node {
        Some(k) => vec![manifest.node(k)?],
        None => manifest.nodes.iter().collect(),
    };
    let polls = timeout.as_secs().div_ceil(WAIT_POLL.as_secs());
    for poll in 0..=polls {
        let answers = relays
            .iter()
            .filter_map(|n| tools.api.node_status(n.api()).ok());
        let low = answers.map(|status| status.now).min();
        if let Some(low) = low.filter(|low| *low >= height) {
            return Ok(low);
        }
        if poll < polls {
            (tools.pause)(WAIT_POLL);
        }
    }
    let (what, seconds) = (format!("height {height}"), timeout.as_secs());
    Err(DevnetError::Timeout { what, seconds })
}

/// The devnet's absolute directory and its manifest.
fn open(root: &Path) -> Result<(PathBuf, Manifest)> {
    let root = std::path::absolute(root).map_err(io(root))?;
    let manifest = Manifest::read(&root)?;
    Ok((root, manifest))
}

/// The roles to act on, role by role in `order`, node by node within a role.
fn targets(m: &Manifest, picked: &[u32], role: Option<Role>, order: [Role; 2]) -> Result<Targets> {
    let mut targets = Vec::new();
    for r in order {
        for k in picked {
            if m.node(*k)?.roles(role).contains(&r) {
                targets.push((*k, r));
            }
        }
    }
    Ok(targets)
}

/// The checks before a start: the chain's and every node's crypto files have the manifest's
/// hashes, and each picked node's `.env` binds its P2P and API listeners to loopback addresses.
fn preflight(root: &Path, manifest: &Manifest, picked: &[u32]) -> Result<()> {
    let nodes = manifest
        .nodes
        .iter()
        .map(|n| (format!("node {}", n.index), n.paths().config));
    let chain = [("the chain".to_owned(), root.join("genesis"))];
    for (holder, dir) in chain.into_iter().chain(nodes) {
        let found = hashes(&crypto_files(&dir.join("crypto"))?);
        let pairs = found.iter().zip(&manifest.crypto_hashes);
        if let Some(((name, _), _)) = pairs.clone().find(|(a, b)| a != b) {
            let reason = format!("{holder}'s crypto/{name} differs from the manifest's");
            let root = root.to_path_buf();
            return Err(DevnetError::Manifest { root, reason });
        }
    }
    for k in picked {
        let env = node_env(manifest.node(*k)?)?;
        for key in ["CORE_P2P_HOST", "CORE_API_HOST"] {
            let value = env.iter().rev().find(|(name, _)| name == key);
            let value = value.map(|(_, value)| value.to_string_lossy());
            let ip = value
                .as_deref()
                .and_then(|value| value.parse::<IpAddr>().ok());
            if !ip.is_some_and(|ip| ip.is_loopback()) {
                let text = format!("node {k}'s .env sets {key} to {value:?}: not loopback");
                return Err(DevnetError::Guard(text));
            }
        }
    }
    Ok(())
}

/// Start the picked roles: the relays (their listeners test-bound first), then, once their APIs
/// answer, the forgers.
fn launch(manifest: &Manifest, picked: &[u32], role: Option<Role>, tools: &Tools) -> Result<()> {
    let all = targets(manifest, picked, role, START)?;
    let nodes = |r: Role| -> Result<Vec<&ManifestNode>> {
        let of = all.iter().filter(|(_, role)| *role == r);
        of.map(|(k, _)| manifest.node(*k)).collect()
    };
    let relays = nodes(Role::Relay)?;
    test_bind(relays.iter().flat_map(|node| [node.p2p(), node.api()]))?;
    for node in &relays {
        begin(tools, node, Role::Relay)?;
    }
    // Any answer shows that an API listens, a refusal or an unready node's included.
    let answers = |node: &ManifestNode| match tools.api.node_status(node.api()) {
        Ok(_) => true,
        Err(error) => matches!(error, HttpError::Status { .. } | HttpError::Body { .. }),
    };
    let mut waiting = relays;
    for poll in 0..=API_POLLS {
        waiting.retain(|node| !answers(node));
        if waiting.is_empty() {
            break;
        }
        if poll == API_POLLS {
            let name = |n: &&ManifestNode| format!("node {}", n.index);
            let names: Vec<String> = waiting.iter().map(name).collect();
            let what = format!("the API of {}", names.join(", "));
            let seconds = API_POLLS;
            return Err(DevnetError::Timeout { what, seconds });
        }
        (tools.pause)(Duration::from_secs(1));
    }
    for node in nodes(Role::Forger)? {
        begin(tools, node, Role::Forger)?;
    }
    Ok(())
}

/// Start `role` on `node`, with the node's environment.
fn begin(tools: &Tools, node: &ManifestNode, role: Role) -> Result<()> {
    Ok(tools.ops.start(&node.instance(role), &node_env(node)?)?)
}

/// Stop the picked roles, forgers first, skipping those that do not run.
pub(crate) fn stop_nodes(
    manifest: &Manifest,
    picked: &[u32],
    role: Option<Role>,
    kill: bool,
    ops: &dyn ProcessOps,
) -> Result<()> {
    let mut first = None;
    for (k, r) in targets(manifest, picked, role, STOP)? {
        let instance = manifest.node(k)?.instance(r);
        let running = ops.status(&instance).map(|row| row.status.is_running());
        let stopped = running.and_then(|running| match (running, kill) {
            (false, _) => Ok(()),
            (true, true) => ops.kill(&instance),
            (true, false) => ops.stop(&instance),
        });
        if let Err(error) = stopped {
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), |error| Err(error.into()))
}

/// Whether a start re-stamps the epoch: it is less than 10 s away or past, no process of the
/// devnet runs, and no node has a chain store (a node that had one may hold blocks above
/// height 1, whose slots a new epoch would move).
fn restamp_due(root: &Path, m: &Manifest, ops: &dyn ProcessOps, clock: &dyn Clock) -> Result<bool> {
    let Some(epoch) = slots::epoch_unix_ms(&m.epoch) else {
        let (root, reason) = (
            root.to_path_buf(),
            format!("its epoch {:?} is not valid", m.epoch),
        );
        return Err(DevnetError::Manifest { root, reason });
    };
    if epoch.saturating_sub(clock.now_ms()) >= RESTAMP_WITHIN_MS {
        return Ok(false);
    }
    for node in &m.nodes {
        if fs::symlink_metadata(node.paths().data.join(STORE)).is_ok() {
            return Ok(false);
        }
        for role in node.roles(None) {
            if ops.status(&node.instance(role))?.status.is_running() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Move the epoch to `epoch_delay` seconds after the clock's time: in the chain's and every
/// node's `milestones.json` (nothing else in them changes, so the nethash and the genesis id
/// stay), with every forger's signing record removed, then in the manifest.
fn restamp(root: &Path, manifest: &mut Manifest, clock: &dyn Clock) -> Result<()> {
    let epoch = epoch_after(clock.now_ms(), manifest.epoch_delay)?;
    let path = root.join("genesis/crypto/milestones.json");
    let old = fs::read_to_string(&path).map_err(io(&path))?;
    let Some(new) = restamped(&old, &manifest.epoch, &epoch) else {
        let reason = format!("its epoch {} cannot be re-stamped", manifest.epoch);
        let root = root.to_path_buf();
        return Err(DevnetError::Manifest { root, reason });
    };
    write(&path, new.as_bytes())?;
    for node in &manifest.nodes {
        let paths = node.paths();
        write(&paths.config.join("crypto/milestones.json"), new.as_bytes())?;
        remove(&paths.data.join(RECORD_DIR))?;
    }
    manifest.crypto_hashes = hashes(&crypto_files(&root.join("genesis/crypto"))?);
    manifest.epoch = epoch;
    manifest.write(root)
}

/// The milestones text `text` with its epoch `old` replaced by `new`, byte for byte elsewhere;
/// `None` when `old` is not an epoch of it, or also stands anywhere else.
fn restamped(text: &str, old: &str, new: &str) -> Option<String> {
    let quoted = |epoch: &str| Value::from(epoch).to_string();
    let replaced = text.replace(&quoted(old), &quoted(new));
    let mut expected: Value = serde_json::from_str(text).ok()?;
    let mut changed = false;
    for milestone in expected.as_array_mut()? {
        let epoch = milestone.get_mut("epoch").filter(|epoch| *epoch == old);
        if let Some(epoch) = epoch {
            *epoch = Value::from(new);
            changed = true;
        }
    }
    let replaced_value = serde_json::from_str::<Value>(&replaced).ok()?;
    (changed && replaced_value == expected).then_some(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_re_stamp_changes_the_epoch_and_nothing_else() {
        let text = "[\n    {\n        \"height\": 1,\n        \"epoch\": \"2026-01-01T00:00:00.000Z\"\n    }\n]\n";
        let new = restamped(text, "2026-01-01T00:00:00.000Z", "2026-02-01T00:00:00.000Z");
        assert_eq!(new.unwrap(), text.replace("2026-01-01", "2026-02-01"));
        assert_eq!(restamped(text, "2025-01-01T00:00:00.000Z", "x"), None);
        let twice = "[{\"epoch\": \"E\", \"note\": \"E\"}]";
        assert_eq!(restamped(twice, "E", "F"), None);
    }
}
