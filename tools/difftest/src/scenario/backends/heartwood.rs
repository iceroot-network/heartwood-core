//! The heartwood backend: a devnet of this tree (`heartwood devnet`, mode B), driven by the
//! `heartwood` binary's own commands and the relays' APIs. The devnet's directory holds its
//! manifest, from which the relays' addresses and each forger's delegates are read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use heartwood_cli::Role;
use heartwood_cli::devnet::{Manifest, ManifestNode};
use heartwood_cli::recovery::{self, Marker};

use super::super::backend::{self, Backend, BackendError, Submitted, Tip};
use super::super::format::{CompiledTx, HeartwoodRestart, Part, RestartModes, Via};
use super::http;

/// The variable that names the `heartwood` binary.
pub const BINARY_VARIABLE: &str = "HEARTWOOD_BIN";

fn failed(action: &'static str, message: impl ToString) -> BackendError {
    BackendError::Failed {
        action,
        message: message.to_string(),
    }
}

/// The `heartwood` binary: the variable's, else the one beside the running binary, else the one
/// on `PATH`.
pub fn binary() -> PathBuf {
    if let Some(path) = std::env::var_os(BINARY_VARIABLE) {
        return PathBuf::from(path);
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("heartwood")));
    beside
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("heartwood"))
}

/// The arguments of `heartwood devnet --dir root command node [--role part] [--kill]`.
pub fn devnet_args(root: &Path, command: &str, node: u32, part: Part, kill: bool) -> Vec<String> {
    let mut args = vec![
        "devnet".to_owned(),
        "--dir".into(),
        root.display().to_string(),
        command.into(),
        node.to_string(),
    ];
    match part {
        Part::Relay => args.extend(["--role".to_owned(), "relay".into()]),
        Part::Forger => args.extend(["--role".to_owned(), "forger".into()]),
        Part::Node => {}
    }
    if kill {
        args.push("--kill".into());
    }
    args
}

/// Ask node `node`'s relay to restore its state by a replay at its next start: the reference's
/// marker `clear-saved-states.lock` in the relay's temp directory, as `heartwood state clear
/// --force` writes it. The marker's path.
pub fn request_replay(node: &ManifestNode) -> Result<PathBuf, BackendError> {
    let instance = node.instance(Role::Relay);
    recovery::request(
        &instance,
        Marker::ClearSavedStates,
        true,
        &mut std::io::stdin(),
    )
    .map_err(|e| failed("ask for a replay", e))
}

/// A devnet of this tree.
pub struct Heartwood {
    root: PathBuf,
    binary: PathBuf,
    manifest: Manifest,
    rules: http::Rules,
}

impl Heartwood {
    /// The devnet in `root`, started or not.
    pub fn open(root: &Path) -> Result<Heartwood, BackendError> {
        let manifest = Manifest::read(root).map_err(|e| failed("read the devnet's manifest", e))?;
        Ok(Heartwood {
            root: std::path::absolute(root)
                .map_err(|e| failed("read the devnet's directory", e))?,
            binary: binary(),
            manifest,
            rules: http::Rules::default(),
        })
    }

    fn node(&self, node: u32) -> Result<&ManifestNode, BackendError> {
        self.manifest
            .nodes
            .iter()
            .find(|n| n.index == node)
            .ok_or(BackendError::NoNode(node))
    }

    fn control(
        &self,
        action: &'static str,
        command: &str,
        node: u32,
        part: Part,
        kill: bool,
    ) -> Result<(), BackendError> {
        self.node(node)?;
        let output = Command::new(&self.binary)
            .args(devnet_args(&self.root, command, node, part, kill))
            .output()
            .map_err(|e| failed(action, format!("{}: {e}", self.binary.display())))?;
        if output.status.success() {
            return Ok(());
        }
        Err(failed(
            action,
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

/// Copy the directory `from` to `to`, when it exists.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

impl Backend for Heartwood {
    fn name(&self) -> &'static str {
        "heartwood"
    }

    fn nodes(&self) -> u32 {
        u32::try_from(self.manifest.nodes.len()).unwrap_or(u32::MAX)
    }

    fn held_back(&self) -> BTreeSet<u32> {
        backend::held_back_of(&self.root)
    }

    fn start(&mut self, node: u32, part: Part) -> Result<(), BackendError> {
        self.control("start a node", "start-node", node, part, false)
    }

    fn stop(&mut self, node: u32, part: Part, kill: bool) -> Result<(), BackendError> {
        self.control("stop a node", "stop-node", node, part, kill)
    }

    fn restart(&mut self, node: u32, part: Part, modes: RestartModes) -> Result<(), BackendError> {
        if modes.heartwood == HeartwoodRestart::Replay {
            if part == Part::Forger {
                return Err(BackendError::Unsupported {
                    backend: "heartwood",
                    action: "restart a forger by a replay (a forger keeps no state)",
                });
            }
            request_replay(self.node(node)?)?;
        }
        self.control("restart a node", "restart-node", node, part, false)
    }

    fn partition(&mut self, _: &[Vec<u32>]) -> Result<(), BackendError> {
        Err(BackendError::Unsupported {
            backend: "heartwood",
            action: "partition",
        })
    }

    fn heal(&mut self) -> Result<(), BackendError> {
        Err(BackendError::Unsupported {
            backend: "heartwood",
            action: "heal",
        })
    }

    fn api_url(&self, node: u32) -> Result<String, BackendError> {
        Ok(format!("http://{}", self.node(node)?.api()))
    }

    fn archive(&mut self, dir: &Path) -> Result<(), BackendError> {
        for node in &self.manifest.nodes {
            let paths = node.paths();
            let name = format!("node{}", node.index);
            let copied = copy_tree(
                &paths.data.join("database"),
                &dir.join(&name).join("database"),
            )
            .and_then(|()| copy_tree(&paths.log, &dir.join("logs").join(&name)));
            copied.map_err(|e| failed("archive the nodes", format!("{name}: {e}")))?;
        }
        Ok(())
    }

    fn rules(&mut self, crypto: &serde_json::Value) -> Result<(), BackendError> {
        self.rules = http::Rules::new(crypto)?;
        Ok(())
    }

    fn submit(
        &mut self,
        node: u32,
        via: Via,
        transactions: &[CompiledTx],
    ) -> Result<Vec<Submitted>, BackendError> {
        if via == Via::P2p {
            return Err(BackendError::Unsupported {
                backend: "heartwood",
                action: "submit over the peer-to-peer network",
            });
        }
        http::submit(&self.api_url(node)?, &self.rules, transactions)
    }

    fn tip(&self, node: u32) -> Result<Tip, BackendError> {
        http::tip(&self.api_url(node)?)
    }

    fn forger_delegates(&self, node: u32) -> Result<Vec<String>, BackendError> {
        Ok(self.node(node)?.delegates.clone())
    }
}
