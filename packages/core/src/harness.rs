//! The harness's seams of `relay run` (feature `difftest`), which no release build has.
//!
//! - [`Recorder`]: `--difftest-digest` writes one state digest line per applied, reverted and
//!   restarted state (the replay's format), and `--difftest-blocks` one line per applied block
//!   (`h`, `id`, `hex`, a corpus's block lines). Each line is written whole and flushed, so a kill
//!   leaves at most a torn last line. The first line of a process is its start's (`genesis` at a
//!   first start, else `restart`), whose running digest starts again from zero. An applied
//!   block's line has the missed slots the round before it detects, and its reward, which the
//!   processor checked against the state's; a block at or below a height applied before is a
//!   `reapply`.
//! - [`SeedRules`]: `--difftest-seed ip:port` names one seed that the address rules let through
//!   (loopback and the host's own addresses included), for a mock peer on the same host; every
//!   other rule is the client's.
//! - [`DIR_VARIABLE`]: a directory for a devnet relay's files, which its supervisor passes on (a
//!   devnet's argv carries no harness flag). Each start of the relay without the two flags writes
//!   the next free pair `digest-NNNN.jsonl` and `blocks-NNNN.bin` there, and every start appends
//!   to `network-state.jsonl` one line per getNetworkState answer the relay gives
//!   ([`NetworkStateRecord`]). Each start also writes `await-NNNN.jsonl` there, with the number of
//!   its digest file (`await-0000.jsonl` for a start given `--difftest-digest`): one line per public
//!   request's AwaitBlock step ([`AwaitRecordFile`]).

use std::cmp::Ordering;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use heartwood_blockchain::events::CommitObserver;
use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockHeader, Milestones, Params};
use heartwood_kernel::timestamp::UtcDateTime;
use heartwood_p2p::node_services::{AwaitEnd, AwaitRecord};
use heartwood_p2p::seams::PeerRules;
pub use heartwood_p2p::testing::attestation::AttestationRecordFile;
use heartwood_p2p::utils::rules::Rules;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;
use heartwood_p2p_wire::errors::ReplyRefused;
use heartwood_p2p_wire::internal::NetworkState;
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
use heartwood_p2p_wire::messages::peer::{GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress, UnconfirmedReply};
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use heartwood_state::digest::{self, Step};
use heartwood_state::{RoundState, State};
use serde_json::json;
use tokio::time::Instant;

/// The digest and block files of a relay run.
pub struct Recorder {
    milestones: Arc<Milestones>,
    digests: Option<File>,
    blocks: Option<File>,
    first_start: bool,
    previous: [u8; 32],
    /// The last block's header and the round after it, which detect the next block's missed slots.
    last: Option<(BlockHeader, RoundState)>,
    /// The highest height applied or restored.
    highest: u32,
}

impl Recorder {
    /// A recorder appending to `digests` and `blocks`, when given; `first_start` when the store
    /// was empty.
    pub fn open(
        milestones: Arc<Milestones>,
        digests: Option<&Path>,
        blocks: Option<&Path>,
        first_start: bool,
    ) -> std::io::Result<Recorder> {
        let open = |path: &Path| OpenOptions::new().create(true).append(true).open(path);
        Ok(Recorder {
            milestones,
            digests: digests.map(open).transpose()?,
            blocks: blocks.map(open).transpose()?,
            first_start,
            previous: [0; 32],
            last: None,
            highest: 0,
        })
    }

    fn line(&mut self, state: &State, step: Step, block: Option<&Block>) {
        let missed = match (block, &self.last) {
            (Some(block), Some((last, round))) => {
                round.detect_missed_blocks(last, block.header(), &self.milestones)
            }
            _ => Vec::new(),
        };
        let reward = block.map(|block| block.header().reward);
        let line = digest::height_digest(
            state,
            &self.milestones,
            step,
            &missed,
            reward,
            &self.previous,
        );
        self.previous = line.chain_digest;
        write(&mut self.digests, &line.to_json());
        if let Some(block) = block {
            let hex = hex::encode(&serialiser::serialise(block));
            write(
                &mut self.blocks,
                &json!({"h": block.height(), "id": block.id().to_hex(), "hex": hex}).to_string(),
            );
        }
        self.last = Some((state.last_block().header().clone(), state.round().clone()));
        self.highest = self.highest.max(state.height());
    }
}

/// Write `line` and a line break in one write, then flush; a failure is logged, as the harness
/// finds the gap.
fn write(file: &mut Option<File>, line: &str) {
    let written = file.as_mut().map(|file| {
        let bytes = format!("{line}\n");
        file.write_all(bytes.as_bytes()).and_then(|()| file.flush())
    });
    if let Some(Err(error)) = written {
        heartwood_kernel::error!("the harness's record could not be written: {error}");
    }
}

impl CommitObserver for Recorder {
    fn applied(&mut self, state: &State, block: &Block) {
        let step = if block.height() <= self.highest {
            Step::Reapply
        } else {
            Step::Apply
        };
        self.line(state, step, Some(block));
    }

    fn reverted(&mut self, state: &State) {
        self.line(state, Step::Revert, None);
    }

    fn restarted(&mut self, state: &State) {
        self.previous = [0; 32];
        let step = if self.first_start {
            Step::Genesis
        } else {
            Step::Restart
        };
        self.line(state, step, None);
    }
}

/// The client's rules, with one seed that the address rules let through.
pub struct SeedRules {
    rules: Rules,
    seed: Option<IpAddr>,
}

impl SeedRules {
    /// `rules`, letting `seed` through the address rules when given.
    pub fn new(rules: Rules, seed: Option<IpAddr>) -> SeedRules {
        SeedRules { rules, seed }
    }
}

impl PeerRules for SeedRules {
    fn encode_request(&self, request: &ClientRequest, headers: &Headers) -> Vec<u8> {
        self.rules.encode_request(request, headers)
    }
    fn check_peers(&self, peers: &[ListedPeer]) -> Result<Vec<PeerAddress>, ReplyRefused> {
        self.rules.check_peers(peers)
    }
    fn check_common_blocks(
        &self,
        reply: &GetCommonBlocksResponse,
    ) -> Result<Option<CommonBlock>, ReplyRefused> {
        self.rules.check_common_blocks(reply)
    }
    fn check_blocks(&self, blocks: &mut [ListedBlock]) -> Result<(), ReplyRefused> {
        self.rules.check_blocks(blocks)
    }
    fn check_post_block(&self, reply: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused> {
        self.rules.check_post_block(reply)
    }
    fn check_unconfirmed(
        &self,
        reply: &UnconfirmedReply,
        max_transactions: usize,
    ) -> Result<(), ReplyRefused> {
        self.rules.check_unconfirmed(reply, max_transactions)
    }
    fn check_post_transactions(&self, accept: &[String]) -> Result<(), ReplyRefused> {
        self.rules.check_post_transactions(accept)
    }
    fn throttle(&self, ip: IpAddr, route: RouteId, now: Instant) -> Option<Duration> {
        self.rules.throttle(ip, route, now)
    }
    fn would_throttle(&self, ip: IpAddr, route: RouteId, now: Instant) -> bool {
        self.rules.would_throttle(ip, route, now)
    }
    fn is_valid_peer(&self, ip: &mut String, allow_private: bool, interfaces: &[IpAddr]) -> bool {
        let seed = self.seed.is_some_and(|seed| ip.parse() == Ok(seed));
        seed || self.rules.is_valid_peer(ip, allow_private, interfaces)
    }
    fn is_valid_version(
        &self,
        version: Option<&str>,
        milestone: &Params,
        configured: &[String],
        network_name: &str,
    ) -> bool {
        self.rules
            .is_valid_version(version, milestone, configured, network_name)
    }
    fn same_subnet(&self, a: IpAddr, b: IpAddr) -> bool {
        self.rules.same_subnet(a, b)
    }
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        self.rules.compare_versions(a, b)
    }
}

/// The variable that names a relay's directory of harness files.
pub const DIR_VARIABLE: &str = "HEARTWOOD_DIFFTEST_DIR";

/// The next free pair of a start's files in `dir` (made when missing): `digest-NNNN.jsonl` and
/// `blocks-NNNN.bin`, `NNNN` one more than the highest number of a file there, from 0001.
pub fn next_start_files(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let failed = |error: std::io::Error| format!("{}: {error}", dir.display());
    std::fs::create_dir_all(dir).map_err(failed)?;
    let number = |name: &str| {
        let name = name
            .strip_prefix("digest-")
            .or(name.strip_prefix("blocks-"))?;
        let digits = name.strip_suffix(".jsonl").or(name.strip_suffix(".bin"))?;
        digits.parse::<u32>().ok()
    };
    let entries = std::fs::read_dir(dir).map_err(failed)?;
    let names = entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok());
    let highest = names.filter_map(|name| number(&name)).max().unwrap_or(0);
    let next = highest.saturating_add(1);
    Ok((
        dir.join(format!("digest-{next:04}.jsonl")),
        dir.join(format!("blocks-{next:04}.bin")),
    ))
}

/// The network-state record of a relay: one JSON line per getNetworkState answer, `{t, caller,
/// status, nodeHeight, hasQuorum, noQuorum, delegates: {hasQuorum, noQuorum, noResponse}}`, `t`
/// the UTC time, appended to `network-state.jsonl` and flushed line by line.
pub struct NetworkStateRecord {
    file: Mutex<Option<File>>,
}

impl NetworkStateRecord {
    /// The record in `dir` (made when missing).
    pub fn open(dir: &Path) -> Result<NetworkStateRecord, String> {
        let failed = |error: std::io::Error| format!("{}: {error}", dir.display());
        std::fs::create_dir_all(dir).map_err(failed)?;
        let path = dir.join("network-state.jsonl");
        let file = OpenOptions::new().create(true).append(true).open(&path);
        let file = file.map_err(failed)?;
        Ok(NetworkStateRecord {
            file: Mutex::new(Some(file)),
        })
    }

    /// Record `state`, served to `caller` at `now_ms`.
    pub fn write(&self, now_ms: i64, caller: IpAddr, state: &NetworkState) {
        let t = UtcDateTime::from_unix_ms(now_ms).map(|time| time.to_iso_string());
        let (details, lists) = (&state.quorum_details, &state.quorum_details.delegates);
        let line = json!({
            "t": t, "caller": caller.to_string(), "status": state.status as u8,
            "nodeHeight": state.node_height, "hasQuorum": details.has_quorum,
            "noQuorum": details.no_quorum,
            "delegates": {
                "hasQuorum": lists.has_quorum, "noQuorum": lists.no_quorum,
                "noResponse": lists.no_response,
            },
        });
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        write(&mut file, &line.to_string());
    }
}

/// The AwaitBlock record of one start of a relay: one JSON line per public request's AwaitBlock
/// step, appended to its `await-NNNN.jsonl` and flushed line by line.
pub struct AwaitRecordFile {
    file: Mutex<Option<File>>,
}

impl AwaitRecordFile {
    /// The record at `path` (created when missing, else appended to).
    pub fn open(path: &Path) -> Result<AwaitRecordFile, String> {
        let file = OpenOptions::new().create(true).append(true).open(path);
        let file = file.map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(AwaitRecordFile {
            file: Mutex::new(Some(file)),
        })
    }

    /// Record `record`: `{t, route, caller, waitMs, end, tipBefore, tipAfter}`, `t` the UTC time
    /// the request reached the step, `end` one of `none`, `drained`, `bound` and `closed`.
    pub fn write(&self, record: &AwaitRecord) {
        let t = UtcDateTime::from_unix_ms(record.at_ms).map(|time| time.to_iso_string());
        let end = match record.end {
            AwaitEnd::NoWait => "none",
            AwaitEnd::Drained => "drained",
            AwaitEnd::Bound => "bound",
            AwaitEnd::SocketClosed => "closed",
        };
        let line = json!({
            "t": t, "route": record.route.name(), "caller": record.caller.to_string(),
            "waitMs": u64::try_from(record.waited.as_millis()).unwrap_or(u64::MAX),
            "end": end, "tipBefore": record.tip_before, "tipAfter": record.tip_after,
        });
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        write(&mut file, &line.to_string());
    }
}

/// The recorder, the network-state record, the AwaitBlock record and the attestation record of a
/// relay run, when the harness asks for them.
pub type HarnessFiles = (
    Option<Box<dyn CommitObserver>>,
    Option<NetworkStateRecord>,
    Option<AwaitRecordFile>,
    Option<AttestationRecordFile>,
);

/// The harness's files of a relay run: the recorder of `--difftest-digest` and
/// `--difftest-blocks`, or else of the next free pair of per-start files in the directory that
/// [`DIR_VARIABLE`] names; and the network-state record and this start's AwaitBlock and
/// attestation records there.
pub fn files(
    relay: &crate::config::RelayConfig,
    run: &crate::commands::relay::RelayRun,
    milestones: &Arc<Milestones>,
    first_start: bool,
) -> Result<HarnessFiles, String> {
    let dir = relay.env.get(DIR_VARIABLE).filter(|dir| !dir.is_empty());
    let dir = dir.map(PathBuf::from);
    let (mut digests, mut blocks) = (run.difftest_digest.clone(), run.difftest_blocks.clone());
    let mut number = "0000.jsonl".to_owned();
    if let (None, None, Some(dir)) = (&digests, &blocks, &dir) {
        let (digest, block) = next_start_files(dir)?;
        let name = digest.file_name().and_then(|name| name.to_str());
        if let Some(digits) = name.and_then(|name| name.strip_prefix("digest-")) {
            digits.clone_into(&mut number);
        }
        (digests, blocks) = (Some(digest), Some(block));
    }
    let observer: Option<Box<dyn CommitObserver>> = match (&digests, &blocks) {
        (None, None) => None,
        _ => Some(Box::new(
            Recorder::open(
                Arc::clone(milestones),
                digests.as_deref(),
                blocks.as_deref(),
                first_start,
            )
            .map_err(|error| error.to_string())?,
        )),
    };
    let record = (dir.as_ref().map(|dir| NetworkStateRecord::open(dir))).transpose()?;
    let awaits =
        (dir.as_ref()).map(|dir| AwaitRecordFile::open(&dir.join(format!("await-{number}"))));
    let attests = dir.map(|dir| AttestationRecordFile::open(&dir.join(format!("attest-{number}"))));
    Ok((observer, record, awaits.transpose()?, attests.transpose()?))
}
