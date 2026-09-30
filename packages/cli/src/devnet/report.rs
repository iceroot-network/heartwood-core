//! What the devnet shows: its status, its nodes' logs merged in time order, and a node's
//! environment.

use std::fmt;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use heartwood_crypto::utils::js;
use heartwood_kernel::config::dot_env::{self, EnvValue};

use serde_json::{Value, json};

use crate::devnet::manifest::io;
use crate::devnet::procfs::tree_resources;
use crate::devnet::{Consistency, DevnetStatus, Manifest, ManifestNode, NodeSelection};
use crate::devnet::{NodeStatus, QUORUM, RelayView, Result, Tools};
use crate::errors::DevnetError;
use crate::seams::{ApiBlock, ApiPeer, Env, Instance, Role, StatusRow, SupervisedStatus};

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
    let mut lists = Vec::with_capacity(manifest.nodes.len());
    for node in &manifest.nodes {
        let status = |role: Role| tools.ops.status(&node.instance(role));
        let forger = node.forger_address.filter(|_| !node.core);
        let forger = forger.map(|_| status(Role::Forger)).transpose()?;
        let (api, address) = (tools.api, node.api());
        let view = || {
            let status = api.node_status(address).ok()?;
            let last_block = api.last_block(address).ok()?;
            let list = api.peers(address).ok()?;
            let peers = list.len();
            Some((
                RelayView {
                    status,
                    last_block,
                    peers,
                },
                list,
            ))
        };
        let role = if node.core { Role::Core } else { Role::Relay };
        let (index, relay, (api, list)) = (node.index, status(role)?, view().unzip());
        lists.push((index, list.unwrap_or_default()));
        let delegates = u32::try_from(node.delegates.len()).unwrap_or(u32::MAX);
        nodes.push(NodeStatus {
            node: index,
            relay,
            forger,
            core: node.core,
            api,
            reported: None,
            delegates,
        });
    }
    for (status, node) in nodes.iter_mut().zip(&manifest.nodes) {
        status.reported = reported(node, &lists);
    }
    let online = |node: &&NodeStatus| {
        let row = node.forger.as_ref().or(node.core.then_some(&node.relay));
        let status = row.map(|row| row.status);
        matches!(
            status,
            Some(SupervisedStatus::Online | SupervisedStatus::OnlineUnsupervised)
        )
    };
    let online_delegates = nodes.iter().filter(online).map(|node| node.delegates).sum();
    let (consistency, compared) = consistency(tools, &manifest, &nodes);
    let relays = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
    Ok(DevnetStatus {
        nodes,
        consistency,
        compared,
        relays,
        online_delegates,
    })
}

/// The address at which the other relays list `node`, as `ip:port`: the first entry of their
/// peer lists (`lists`, by node) with the node's address, at the port it lists; `None` when none
/// lists it.
fn reported(node: &ManifestNode, lists: &[(u32, Vec<ApiPeer>)]) -> Option<SocketAddr> {
    let others = lists.iter().filter(|(index, _)| *index != node.index);
    let mut peers = others.flat_map(|(_, list)| list);
    let peer = peers.find(|peer| peer.ip.parse() == Ok(node.address));
    peer.map(|peer| SocketAddr::new(node.address, peer.port))
}

/// The relays' agreement: their block ids at the lowest height among their last blocks, and how
/// many relays gave one (a relay whose block at that height cannot be read is not compared).
fn consistency(tools: &Tools<'_>, manifest: &Manifest, nodes: &[NodeStatus]) -> (Consistency, u32) {
    let last = |status: &NodeStatus| Some(status.api.as_ref()?.last_block.clone());
    let pairs = manifest.nodes.iter().zip(nodes);
    let answered: Vec<_> = pairs
        .filter_map(|(n, s)| Some((n.api(), last(s)?)))
        .collect();
    let Some(height) = answered.iter().map(|(_, block)| block.height).min() else {
        return (Consistency::Unknown, 0);
    };
    let id = |(address, block): &(SocketAddr, ApiBlock)| match block.height == height {
        true => Some(block.id.clone()),
        false => Some(tools.api.block_at(*address, height).ok()??.id),
    };
    let mut ids: Vec<String> = answered.iter().filter_map(id).collect();
    let compared = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    ids.sort_unstable();
    ids.dedup();
    match ids.len() {
        0 | 1 => (Consistency::Consistent(height), compared),
        _ => (Consistency::Diverged(height), compared),
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
    /// The chain line: `chain consistent at height 6 (5 of 5 relays); delegates with an online
    /// forger: 53 of 53 (27 needed)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (chain, online, all) = (self.consistency, self.online_delegates, self.delegates());
        let (compared, relays) = (self.compared, self.relays);
        write!(
            f,
            "chain {chain} ({compared} of {relays} relays); delegates with an online forger: {online} of {all} "
        )?;
        write!(f, "({QUORUM} needed)")
    }
}

/// The machine-readable form of [`status`] (`status --json`): every node's processes (role,
/// status, pid, uptime in seconds, restarts), its API's URL and reads, and the address its peers
/// report for it; the consistency, the counts compared and the delegates online.
pub fn status_json(root: &Path, tools: &Tools<'_>) -> Result<Value> {
    let (status, manifest) = (status(root, tools)?, Manifest::read(root)?);
    let row = |role: Role, row: &StatusRow| {
        let (state, pid) = (row.status.name(), row.pid);
        let uptime = row.uptime.map(|time| time.as_secs_f64());
        json!({ "role": role.name(), "status": state, "pid": pid, "uptime": uptime,
                "restarts": row.restarts })
    };
    let node = |(node, at): (&NodeStatus, &ManifestNode)| {
        let first = if node.core { Role::Core } else { Role::Relay };
        let rows = [(first, Some(&node.relay))].into_iter();
        let rows = rows.chain([(Role::Forger, node.forger.as_ref())]);
        let processes: Vec<Value> = rows.filter_map(|(r, s)| Some(row(r, s?))).collect();
        let api = node.api.as_ref().map(|api| {
            json!({ "height": api.last_block.height, "lastBlockId": api.last_block.id,
                    "synced": api.status.synced, "peers": api.peers })
        });
        let reported = node.reported.map(|address| address.to_string());
        json!({ "node": node.node, "address": at.address.to_string(), "core": node.core,
                "apiUrl": format!("http://{}", at.api()), "processes": processes, "api": api,
                "reportedAddress": reported,
                "delegates": node.delegates })
    };
    let nodes: Vec<Value> = status.nodes.iter().zip(&manifest.nodes).map(node).collect();
    let (kind, height) = match status.consistency {
        Consistency::Consistent(height) => ("consistent", Some(height)),
        Consistency::Diverged(height) => ("diverged", Some(height)),
        Consistency::Unknown => ("unknown", None),
    };
    Ok(json!({
        "chain": { "consistency": kind, "height": height, "compared": status.compared,
                   "relays": status.relays },
        "delegates": { "total": status.delegates(), "online": status.online_delegates,
                       "quorum": QUORUM, "hasQuorum": status.has_quorum() },
        "nodes": nodes,
    }))
}

/// The resources of one process tree (a supervisor's node and its children), Linux only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Resources {
    /// Resident memory of the whole tree, in bytes.
    pub rss: u64,
    /// Proportional memory of the whole tree (`smaps_rollup`), in bytes, where readable.
    pub pss: Option<u64>,
    /// CPU time of the whole tree, user and system, in milliseconds.
    pub cpu_ms: u64,
    /// Bytes the tree has written (`/proc/<pid>/io`), where readable.
    pub written: Option<u64>,
}

impl fmt::Display for Resources {
    /// `rss 512 MiB, pss 400 MiB, cpu 1234 ms, written 8 MiB`; a figure that could not be read
    /// shows as `n/a`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mib =
            |bytes: Option<u64>| bytes.map_or("n/a".to_owned(), |b| format!("{} MiB", b >> 20));
        let (rss, pss, written) = (mib(Some(self.rss)), mib(self.pss), mib(self.written));
        write!(
            f,
            "rss {rss}, pss {pss}, cpu {} ms, written {written}",
            self.cpu_ms
        )
    }
}

/// The resources of every running process of the devnet (`status --resources`), labelled
/// `node<k>/<role>`, in node and role order. The resources are those of the process tree under
/// the node's pid, read from `/proc`; a process that does not run is left out. Refused where
/// there is no `/proc`.
pub fn resources(root: &Path, tools: &Tools<'_>) -> Result<Vec<(String, Resources)>> {
    resources_in(root, tools, Path::new("/proc"))
}

/// [`resources`] over the `/proc` tree at `proc`.
pub fn resources_in(
    root: &Path,
    tools: &Tools<'_>,
    proc: &Path,
) -> Result<Vec<(String, Resources)>> {
    if !proc.join("self").exists() {
        return Err(DevnetError::Guard(
            "status --resources needs Linux's /proc".to_owned(),
        ));
    }
    let manifest = Manifest::read(root)?;
    let mut rows = Vec::new();
    for node in &manifest.nodes {
        for role in node.roles(None) {
            let row = tools.ops.status(&node.instance(role))?;
            let used = row.pid.and_then(|pid| tree_resources(proc, pid));
            rows.extend(used.map(|used| (format!("node{}/{role}", node.index), used)));
        }
    }
    Ok(rows)
}

/// How long [`follow`] sleeps between two looks at the logs.
const FOLLOW_POLL: Duration = Duration::from_millis(200);

/// [`logs`], then follow the files as they grow (`logs --follow`), with the node and role
/// prefixes, until `stop` answers true. A file that does not exist yet is picked up when it
/// appears; one that shrinks (a rotation) is read again from its start. Only whole lines are
/// written: a line still being written waits for its end.
pub fn follow(
    root: &Path,
    selection: &NodeSelection,
    stream: LogStream,
    lines: usize,
    out: &mut dyn Write,
    stop: &dyn Fn() -> bool,
) -> Result<()> {
    logs(root, selection, stream, lines, out)?;
    let manifest = Manifest::read(root)?;
    let mut sources = Vec::new();
    for k in manifest.select(selection)? {
        let node = manifest.node(k)?;
        for role in node.roles(selection.role) {
            let path = log_path(&node.instance(role), stream);
            let start = fs::metadata(&path).map_or(0, |meta| meta.len());
            sources.push(Followed {
                label: format!("node{k}/{role}: "),
                path,
                seen: start,
                partial: Vec::new(),
            });
        }
    }
    let prefixed = sources.len() > 1;
    while !stop() {
        for source in &mut sources {
            source.read(prefixed, out)?;
        }
        out.flush().map_err(io(Path::new("the output")))?;
        std::thread::sleep(FOLLOW_POLL);
    }
    Ok(())
}

/// A log [`follow`] tails: how far it has been read, and the start of a line not yet ended.
struct Followed {
    label: String,
    path: std::path::PathBuf,
    seen: u64,
    partial: Vec<u8>,
}

impl Followed {
    /// Write the new whole lines of the file to `out`.
    fn read(&mut self, prefixed: bool, out: &mut dyn Write) -> Result<()> {
        let Ok(mut file) = File::open(&self.path) else {
            return Ok(());
        };
        let length = file.metadata().map_err(io(&self.path))?.len();
        if length < self.seen {
            (self.seen, self.partial) = (0, Vec::new());
        }
        file.seek(SeekFrom::Start(self.seen))
            .map_err(io(&self.path))?;
        let mut data = Vec::new();
        file.take(length.saturating_sub(self.seen))
            .read_to_end(&mut data)
            .map_err(io(&self.path))?;
        self.seen = self
            .seen
            .saturating_add(u64::try_from(data.len()).unwrap_or(0));
        self.partial.extend(data);
        let label = if prefixed { self.label.as_str() } else { "" };
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            write!(out, "{label}{line}").map_err(io(Path::new("the output")))?;
        }
        Ok(())
    }
}

/// The path of `stream`'s log of `instance`.
fn log_path(instance: &Instance, stream: LogStream) -> std::path::PathBuf {
    match stream {
        LogStream::Out => instance.out_log_path(),
        LogStream::Error => instance.error_log_path(),
        LogStream::File => instance.log_path(),
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
            let path = log_path(&node.instance(role), stream);
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
