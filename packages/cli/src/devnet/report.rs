//! What the devnet shows: its status, its nodes' logs merged in time order, and a node's
//! environment.

use std::fmt;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::Path;

use heartwood_crypto::utils::js;
use heartwood_kernel::config::dot_env::{self, EnvValue};

use crate::devnet::manifest::io;
use crate::devnet::{Consistency, DevnetStatus, Manifest, ManifestNode, NodeSelection};
use crate::devnet::{NodeStatus, QUORUM, RelayView, Result, Tools};
use crate::seams::{ApiBlock, Env, Role, SupervisedStatus};

/// The most bytes read from a log's end to find its last lines.
const WINDOW: u64 = 1024 * 1024;
/// The length of a log line's time, `[YYYY-MM-DD HH:MM:SS.mmm]`.
const STAMP: usize = 25;

/// The devnet's status: each node's supervised roles and its relay's API, the chain's
/// consistency at the lowest common height of the relays that answered, and the delegates whose
/// forger is online.
pub fn status(root: &Path, tools: &Tools<'_>) -> Result<DevnetStatus> {
    let manifest = Manifest::read(root)?;
    let mut nodes = Vec::with_capacity(manifest.nodes.len());
    for node in &manifest.nodes {
        let status = |role: Role| tools.ops.status(&node.instance(role));
        let forger = node
            .forger_address
            .map(|_| status(Role::Forger))
            .transpose()?;
        let (api, address) = (tools.api, node.api());
        let view = || {
            let status = api.node_status(address).ok()?;
            let last_block = api.last_block(address).ok()?;
            let peers = api.peers(address).ok()?.len();
            Some(RelayView {
                status,
                last_block,
                peers,
            })
        };
        let (index, relay, api) = (node.index, status(Role::Relay)?, view());
        let delegates = u32::try_from(node.delegates.len()).unwrap_or(u32::MAX);
        nodes.push(NodeStatus {
            node: index,
            relay,
            forger,
            api,
            delegates,
        });
    }
    let online = |node: &&NodeStatus| {
        let status = node.forger.as_ref().map(|row| row.status);
        matches!(
            status,
            Some(SupervisedStatus::Online | SupervisedStatus::OnlineUnsupervised)
        )
    };
    let online_delegates = nodes.iter().filter(online).map(|node| node.delegates).sum();
    let consistency = consistency(tools, &manifest, &nodes);
    Ok(DevnetStatus {
        nodes,
        consistency,
        online_delegates,
    })
}

/// The relays' agreement: their block ids at the lowest height among their last blocks.
fn consistency(tools: &Tools<'_>, manifest: &Manifest, nodes: &[NodeStatus]) -> Consistency {
    let last = |status: &NodeStatus| Some(status.api.as_ref()?.last_block.clone());
    let pairs = manifest.nodes.iter().zip(nodes);
    let answered: Vec<_> = pairs
        .filter_map(|(n, s)| Some((n.api(), last(s)?)))
        .collect();
    let Some(height) = answered.iter().map(|(_, block)| block.height).min() else {
        return Consistency::Unknown;
    };
    let id = |(address, block): &(SocketAddr, ApiBlock)| match block.height == height {
        true => Some(block.id.clone()),
        false => Some(tools.api.block_at(*address, height).ok()??.id),
    };
    let mut ids: Vec<String> = answered.iter().filter_map(id).collect();
    ids.sort_unstable();
    ids.dedup();
    match ids.len() {
        0 | 1 => Consistency::Consistent(height),
        _ => Consistency::Diverged(height),
    }
}

impl DevnetStatus {
    /// Every delegate of the devnet.
    pub fn delegates(&self) -> u32 {
        self.nodes.iter().map(|node| node.delegates).sum()
    }

    /// Whether the delegates whose forger is online reach the quorum of 27.
    pub fn has_quorum(&self) -> bool {
        self.online_delegates >= QUORUM
    }
}

impl fmt::Display for DevnetStatus {
    /// The chain line: `chain consistent at height 6; delegates with an online forger: 53 of 53
    /// (27 needed)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (chain, online, all) = (self.consistency, self.online_delegates, self.delegates());
        write!(
            f,
            "chain {chain}; delegates with an online forger: {online} of {all} "
        )?;
        write!(f, "({QUORUM} needed)")
    }
}

/// Which log of each process [`logs`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    /// The supervisor's capture of the node's standard output.
    Out,
    /// The supervisor's capture of the node's standard error.
    Error,
    /// The node's own log file, `<name>-current.log`.
    File,
}

/// Write the last `lines` lines of `stream` of each selected process to `out`, from the last
/// megabyte of each file. With several processes, each line is prefixed with its node and role
/// (`node3/forger: `) and the lines are merged in the order of their times; a line without a
/// time keeps the time of the line before it. A log that does not exist yet is skipped.
pub fn logs(
    root: &Path,
    selection: &NodeSelection,
    stream: LogStream,
    lines: usize,
    out: &mut dyn Write,
) -> Result<()> {
    let manifest = Manifest::read(root)?;
    let mut sources = Vec::new();
    for k in manifest.select(selection)? {
        let node = manifest.node(k)?;
        for role in node.roles(selection.role) {
            let instance = node.instance(role);
            let path = match stream {
                LogStream::Out => instance.out_log_path(),
                LogStream::Error => instance.error_log_path(),
                LogStream::File => instance.log_path(),
            };
            sources.push((format!("node{k}/{role}: "), path));
        }
    }
    let (mut merged, prefixed) = (Vec::new(), sources.len() > 1);
    for (label, path) in &sources {
        let text = match tail(path) {
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            text => text.map_err(io(path))?,
        };
        let all: Vec<&str> = text.lines().collect();
        let (mut time, label) = ("", if prefixed { label.as_str() } else { "" });
        for line in all.iter().skip(all.len().saturating_sub(lines)) {
            time = stamp(line).unwrap_or(time);
            merged.push((time.to_owned(), format!("{label}{line}")));
        }
    }
    merged.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, line) in merged {
        writeln!(out, "{line}").map_err(io(Path::new("the output")))?;
    }
    Ok(())
}

/// The time at the start of a log line, `[YYYY-MM-DD HH:MM:SS.mmm]`.
fn stamp(line: &str) -> Option<&str> {
    let stamp = line.get(..STAMP)?;
    (stamp.starts_with('[') && stamp.ends_with(']')).then_some(stamp)
}

/// The text of the last [`WINDOW`] bytes of the file `path`, from its first whole line when the
/// window starts inside the file.
fn tail(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    let whole = if start > 0 {
        data.iter().position(|b| *b == b'\n')
    } else {
        None
    };
    let data = data
        .get(whole.map_or(0, |at| at.saturating_add(1))..)
        .unwrap_or_default();
    Ok(String::from_utf8_lossy(data).into_owned())
}

/// Node `node`'s environment: the values of its `.env`, then its five `CORE_PATH_*` variables.
/// Passed to its processes, they win over the caller's environment.
pub fn env(root: &Path, node: u32) -> Result<Env> {
    node_env(Manifest::read(root)?.node(node)?)
}

/// The environment of `node`, as [`env()`] gives it.
pub(crate) fn node_env(node: &ManifestNode) -> Result<Env> {
    let paths = node.paths();
    let path = paths.config.join(".env");
    let text = fs::read_to_string(&path).map_err(io(&path))?;
    let value = |value: EnvValue| match value {
        EnvValue::Number(number) => js::number_to_string(number),
        EnvValue::Bool(flag) => flag.to_string(),
        EnvValue::Text(text) => text,
    };
    let pairs = dot_env::parse(&text).into_iter();
    let mut env: Env = pairs
        .map(|(key, v)| (key.into(), value(v).into()))
        .collect();
    let dirs = [
        ("CONFIG", paths.config),
        ("DATA", paths.data),
        ("CACHE", paths.cache),
    ];
    let dirs = dirs
        .into_iter()
        .chain([("LOG", paths.log), ("TEMP", paths.temp)]);
    env.extend(dirs.map(|(kind, dir)| (format!("CORE_PATH_{kind}").into(), dir.into())));
    Ok(env)
}
