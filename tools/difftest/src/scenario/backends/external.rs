//! The external backend: the executable `DIFFTEST_SCENARIO_BACKEND` names, called once per action as
//! `<executable> <verb> <network-dir> [arguments]`. Its exit status 3 says the network does not offer
//! the verb, 1 that the action failed (the reason on stderr). The runner reads tips and submits
//! transactions through the API, at the address the `api-url` verb prints; `post-transactions` is
//! the peer-to-peer way in.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::super::backend::{Backend, BackendError, Submitted, Tip};
use super::super::format::{
    CompiledTx, HeartwoodRestart, Part, ReferenceRestart, RestartModes, Via,
};
use super::http;

/// The variable that names the executable.
pub const VARIABLE: &str = "DIFFTEST_SCENARIO_BACKEND";

/// The exit status that says the network does not offer a verb.
const NOT_OFFERED: i32 = 3;

fn failed(action: &'static str, message: impl ToString) -> BackendError {
    BackendError::Failed {
        action,
        message: message.to_string(),
    }
}

fn part_word(part: Part) -> &'static str {
    match part {
        Part::Relay => "relay",
        Part::Forger => "forger",
        Part::Node => "node",
    }
}

/// The arguments of the verb that restarts `part` of `node`.
pub fn restart_args(node: u32, part: Part, modes: RestartModes) -> Vec<String> {
    let mut args = vec![node.to_string(), part_word(part).to_owned()];
    if modes.reference == ReferenceRestart::Rebuild {
        args.extend(["--reference".to_owned(), "rebuild".into()]);
    }
    if modes.heartwood == HeartwoodRestart::Replay {
        args.extend(["--heartwood".to_owned(), "replay".into()]);
    }
    args
}

/// A network the tooling's executable drives.
pub struct External {
    program: PathBuf,
    root: PathBuf,
    nodes: u32,
    rules: http::Rules,
}

impl External {
    /// The network in `root`, driven by `program`.
    pub fn open(program: &Path, root: &Path) -> Result<External, BackendError> {
        let mut external = External {
            program: program.to_path_buf(),
            root: root.to_path_buf(),
            nodes: 0,
            rules: http::Rules::default(),
        };
        let info = external.call("read the network's description", "info", &[])?;
        let info: Value =
            serde_json::from_str(&info).map_err(|e| failed("read the network's description", e))?;
        external.nodes = info["nodes"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        Ok(external)
    }

    /// One call of the executable; its stdout.
    fn call(
        &self,
        action: &'static str,
        verb: &str,
        args: &[String],
    ) -> Result<String, BackendError> {
        let output = Command::new(&self.program)
            .arg(verb)
            .arg(&self.root)
            .args(args)
            .output()
            .map_err(|e| failed(action, format!("{}: {e}", self.program.display())))?;
        match output.status.code() {
            Some(0) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            Some(NOT_OFFERED) => Err(BackendError::Unsupported {
                backend: "external",
                action,
            }),
            _ => Err(failed(
                action,
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            )),
        }
    }
}

impl Backend for External {
    fn name(&self) -> &'static str {
        "external"
    }

    fn nodes(&self) -> u32 {
        self.nodes
    }

    fn start(&mut self, node: u32, part: Part) -> Result<(), BackendError> {
        self.call(
            "start a node",
            "start",
            &[node.to_string(), part_word(part).into()],
        )
        .map(drop)
    }

    fn stop(&mut self, node: u32, part: Part, kill: bool) -> Result<(), BackendError> {
        let mut args = vec![node.to_string(), part_word(part).to_owned()];
        if kill {
            args.push("--kill".into());
        }
        self.call("stop a node", "stop", &args).map(drop)
    }

    fn restart(&mut self, node: u32, part: Part, modes: RestartModes) -> Result<(), BackendError> {
        self.call(
            "restart a node",
            "restart",
            &restart_args(node, part, modes),
        )
        .map(drop)
    }

    fn partition(&mut self, groups: &[Vec<u32>]) -> Result<(), BackendError> {
        let words: Vec<String> = groups
            .iter()
            .map(|g| {
                g.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect();
        self.call("partition", "partition", &words).map(drop)
    }

    fn heal(&mut self) -> Result<(), BackendError> {
        self.call("heal", "heal", &[]).map(drop)
    }

    fn api_url(&self, node: u32) -> Result<String, BackendError> {
        let out = self.call("find a node's API", "api-url", &[node.to_string()])?;
        Ok(out.trim().to_owned())
    }

    fn archive(&mut self, dir: &Path) -> Result<(), BackendError> {
        self.call("archive the nodes", "archive", &[dir.display().to_string()])
            .map(drop)
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
        if via == Via::Api {
            return http::submit(&self.api_url(node)?, &self.rules, transactions);
        }
        let file =
            std::env::temp_dir().join(format!("scenario-submit-{}-{node}.txt", std::process::id()));
        let lines: Vec<&str> = transactions.iter().map(|t| t.bytes.as_str()).collect();
        std::fs::write(&file, lines.join("\n") + "\n").map_err(|e| failed("submit", e))?;
        let out = self.call(
            "submit over the peer-to-peer network",
            "post-transactions",
            &[node.to_string(), file.display().to_string()],
        );
        let _ = std::fs::remove_file(&file);
        let answers: Vec<Value> = out?
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        transactions
            .iter()
            .map(|t| {
                let answer = answers
                    .iter()
                    .find(|a| a["id"] == t.id.as_str())
                    .ok_or_else(|| failed("submit", format!("no answer for {}", t.reference)))?;
                Ok(Submitted {
                    reference: t.reference.clone(),
                    accepted: answer["accepted"].as_bool().unwrap_or(false),
                    code: None,
                })
            })
            .collect()
    }

    fn tip(&self, node: u32) -> Result<Tip, BackendError> {
        http::tip(&self.api_url(node)?)
    }
}
