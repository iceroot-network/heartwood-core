//! The peer verifier, answered with `heartwood_p2p::nsect` and `heartwood_p2p::verifier`:
//!
//! - `nsect.find` `{n, low, high, common, stop?}` -> `{probes, result}`;
//! - `verifier.checkState` `{chain, delegates, rounds, peer, claimed, fast, deadline?}` ->
//!   `{result, probes, fetched}` (see `tools/oracle/lib/ops/p2p-verifier.js` for the input).
//!
//! The verifier runs over a chain view and a fake peer built from the input, as the oracle's stubs
//! are. Where the reference throws (`result` "stopped"), Heartwood's verification stops too. A
//! fork segment that Heartwood refuses because its headers do not chain is
//! `Documented::UnlinkedForkSegment`, which holds when the headers the peer served from the common
//! height on do not chain (previous id against the id of the block before, as the input's chain
//! gives it).
//!
//! The deadline runs on tokio's paused clock, so the case's own work takes no time: the peer
//! answers the last request allowed only when the deadline has passed, so the verifier finds it
//! passed before its next request, however loaded the machine is.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use heartwood_crypto::blocks::serialiser::block_id;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockHeader, BlockId, Milestones, Network, PublicKeyBytes};
use heartwood_kernel::{ManualClock, RoundInfo};
use heartwood_p2p::download::DownloadedBlock;
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::nsect::NSect;
use heartwood_p2p::seams::{Deadline, VerifierLink};
use heartwood_p2p::verifier::{Outcome as Checked, PeerVerifier, Refusal};
use heartwood_p2p::view::{BlockLookup, BlockPing, ChainView, DelegateInfo};
use heartwood_p2p::{Peer, PeerState, SharedPeer};
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;
use heartwood_p2p_wire::messages::blocks::BlockHeader as ListedHeader;
use heartwood_p2p_wire::messages::peer::StatusHeader;
use heartwood_p2p_wire::replies::CommonBlock;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{Env, Outcome, Run, failed, integer};
use crate::subject::{Case, Documented, SubjectError, Verdict};

/// The operations of the verifier.
pub(super) const OPS: &[(&str, Run)] = &[
    ("nsect.find", nsect_find),
    ("verifier.checkState", check_state),
];

/// How long the deadline of a case with one runs, on the paused clock.
const DEADLINE: Duration = Duration::from_millis(250);

/// Run `future` to its end on a runtime of this thread, whose clock is paused: it moves only when
/// every task waits for a timer.
fn block_on<F: std::future::Future>(future: F) -> Outcome<F::Output> {
    thread_local! {
        static RUNTIME: Option<tokio::runtime::Runtime> = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .ok();
    }
    RUNTIME.with(|runtime| {
        runtime
            .as_ref()
            .map(|runtime| runtime.block_on(future))
            .ok_or_else(|| failed("no tokio runtime"))
    })
}

fn nsect_find(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let n: u32 = integer(input, "n")?;
    let low: u32 = integer(input, "low")?;
    let high: u32 = integer(input, "high")?;
    let common: u32 = integer(input, "common")?;
    let stop = input.get("stop").and_then(Value::as_u64);
    let probes = Mutex::new(Vec::<Vec<u32>>::new());
    let result = block_on(NSect::new(n).find(low, high, |heights: Vec<u32>| {
        let mut asked = probes.lock().unwrap_or_else(PoisonError::into_inner);
        let index = u64::try_from(asked.len()).unwrap_or(u64::MAX);
        let answer = heights.iter().rev().find(|&&h| h <= common).copied();
        asked.push(heights);
        std::future::ready(Ok::<_, SubjectError>(
            answer.filter(|_| stop != Some(index)),
        ))
    }))??;
    let probes = probes.into_inner().unwrap_or_else(PoisonError::into_inner);
    Ok(Verdict::Output(json!({"probes": probes, "result": result})))
}

/// A header as the peer sends it: the serialised header, with fields replaced (`set`).
struct Sent {
    header: BlockHeader,
    set: Map<String, Value>,
}

fn sent(value: &Value, what: &str, milestones: &Milestones) -> Outcome<Sent> {
    let (text, set) = match value {
        Value::String(text) => (text.as_str(), Map::new()),
        Value::Object(object) => (
            object
                .get("hex")
                .and_then(Value::as_str)
                .ok_or_else(|| failed(format!("{what}.hex is not text")))?,
            object
                .get("set")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        ),
        _ => return Err(failed(format!("{what} is not a header"))),
    };
    Ok(Sent {
        header: decode_header(text, what, milestones)?,
        set,
    })
}

fn decode_header(text: &str, what: &str, milestones: &Milestones) -> Outcome<BlockHeader> {
    let bytes = hex::decode(text).map_err(|e| failed(format!("{what}: {e}")))?;
    Block::decode_trusted(&bytes, milestones)
        .map(|block| block.header().clone())
        .map_err(|e| failed(format!("{what}: {e}")))
}

/// The header of `sent` as the reply check leaves it.
fn listed(sent: &Sent) -> Outcome<ListedBlock> {
    let header = &sent.header;
    let mut listed = ListedHeader {
        id: Some(block_id(header).to_hex()),
        version: Some(header.version),
        timestamp: Some(header.timestamp),
        previous_block: Some(hex::encode(&header.previous_block)),
        height: Some(header.height),
        number_of_transactions: Some(header.number_of_transactions),
        total_amount: Some(header.total_amount.to_string()),
        total_fee: Some(header.total_fee.to_string()),
        reward: Some(header.reward.to_string()),
        payload_length: Some(header.payload_length),
        payload_hash: Some(hex::encode(&header.payload_hash)),
        generator_public_key: Some(header.generator_public_key.to_hex()),
        block_signature: Some(header.block_signature.to_hex()),
        ..ListedHeader::default()
    };
    for (key, value) in &sent.set {
        let text = |value: &Value| match value {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text.clone())),
            _ => Err(failed(format!("the header's {key} is not text"))),
        };
        let number = |value: &Value| match value {
            Value::Null => Ok(None),
            value => value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| SubjectError::Unsupported(format!("the header's {key}: {value}"))),
        };
        let amount = |value: &Value| match value {
            Value::String(text) => Ok(Some(text.clone())),
            _ => Err(SubjectError::Unsupported(format!(
                "the header's {key}: {value}"
            ))),
        };
        match key.as_str() {
            "id" => listed.id = text(value)?,
            "previousBlock" => listed.previous_block = text(value)?,
            "payloadHash" => listed.payload_hash = text(value)?,
            "generatorPublicKey" => {
                listed.generator_public_key = text(value)?.map(|key| key.to_lowercase());
            }
            "blockSignature" => listed.block_signature = text(value)?,
            "version" => listed.version = number(value)?,
            "timestamp" => listed.timestamp = number(value)?,
            "height" => listed.height = number(value)?,
            "numberOfTransactions" => listed.number_of_transactions = number(value)?,
            "payloadLength" => listed.payload_length = number(value)?,
            "totalAmount" => listed.total_amount = amount(value)?,
            "totalFee" => listed.total_fee = amount(value)?,
            "reward" => listed.reward = amount(value)?,
            _ => return Err(failed(format!("{key} is not a header field"))),
        }
    }
    let parse = |text: &Option<String>| {
        Amount::parse(text.as_deref().unwrap_or_default())
            .map_err(|e| SubjectError::Unsupported(format!("an amount: {e}")))
    };
    Ok(ListedBlock {
        total_amount: parse(&listed.total_amount)?,
        total_fee: parse(&listed.total_fee)?,
        reward: parse(&listed.reward)?,
        header: listed,
        transactions: Vec::new(),
    })
}

/// The state a peer reports with `listed` as its last block's header.
fn state(height: u32, listed: ListedBlock) -> PeerState {
    let h = listed.header;
    PeerState {
        height,
        forging_allowed: false,
        current_slot: 0,
        header: StatusHeader {
            id: h.id,
            version: h.version,
            timestamp: h.timestamp,
            previous_block: h.previous_block,
            height: h.height,
            number_of_transactions: h.number_of_transactions,
            total_amount: h.total_amount,
            total_fee: h.total_fee,
            reward: h.reward,
            payload_length: h.payload_length,
            payload_hash: h.payload_hash,
            generator_public_key: h.generator_public_key,
            block_signature: h.block_signature,
            ..StatusHeader::default()
        },
        total_amount: listed.total_amount,
        total_fee: listed.total_fee,
        reward: listed.reward,
    }
}

/// Our node, as the input describes it.
struct Ours {
    last: Arc<Block>,
    blocks: Vec<Arc<Block>>,
    delegates: HashMap<PublicKeyBytes, String>,
    rounds: BTreeMap<u64, Vec<PublicKeyBytes>>,
    milestones: Milestones,
    network: Network,
}

impl ChainView for Ours {
    fn last_block(&self) -> Arc<Block> {
        self.last.clone()
    }

    fn last_blocks(&self) -> Vec<Arc<Block>> {
        let from = self.blocks.len().saturating_sub(100);
        self.blocks.iter().skip(from).cloned().collect()
    }

    fn milestones(&self) -> &Milestones {
        &self.milestones
    }

    fn network(&self) -> &Network {
        &self.network
    }

    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
        self.delegates.get(key).map(|username| DelegateInfo {
            username: username.clone(),
            rank: None,
        })
    }

    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        self.delegates.keys().copied().collect()
    }

    fn is_synced(&self) -> bool {
        true
    }

    fn is_idle(&self) -> bool {
        false
    }

    fn block_ping(&self) -> Option<BlockPing> {
        None
    }
}

impl BlockLookup for Ours {
    fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockHeader>, LookupError> {
        // As the reference's lookup: the blocks it has, in the order asked.
        Ok(heights
            .iter()
            .filter_map(|&h| {
                let index = usize::try_from(h).ok()?.checked_sub(1)?;
                self.blocks.get(index).map(|block| block.header().clone())
            })
            .collect())
    }

    fn round_delegate_keys(&self, round: RoundInfo) -> Result<Vec<PublicKeyBytes>, LookupError> {
        self.rounds
            .get(&round.round)
            .cloned()
            .ok_or_else(|| LookupError(format!("round {} is not stored", round.round)))
    }
}

/// A getCommonBlocks answer of the input's script.
#[derive(Debug, Clone)]
enum Script {
    Honest,
    Answer(Option<CommonBlock>),
}

/// What the fake peer was asked.
#[derive(Default)]
struct Log {
    requests: usize,
    probes: Vec<Vec<u32>>,
    fetched: Vec<(u32, u32)>,
}

/// The peer of a case.
struct FakePeer {
    served: Vec<ListedBlock>,
    stored: HashMap<BlockId, u32>,
    our_heights: HashMap<BlockId, u32>,
    script: Vec<Script>,
    per_reply: Option<u32>,
    allowed: Option<usize>,
    deadline: Instant,
    log: Mutex<Log>,
}

impl FakePeer {
    /// Count a request; the last one the deadline allows is answered when the deadline has passed.
    async fn request(&self, record: impl FnOnce(&mut Log)) {
        let index = {
            let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
            record(&mut log);
            log.requests = log.requests.saturating_add(1);
            log.requests
        };
        if self.allowed == Some(index) {
            tokio::time::sleep_until(self.deadline).await;
        }
    }
}

impl VerifierLink for FakePeer {
    async fn common_blocks(
        &self,
        _peer: &SharedPeer,
        ids: &[BlockId],
        _timeout: Duration,
    ) -> Option<CommonBlock> {
        let heights: Vec<u32> = ids
            .iter()
            .map(|id| self.our_heights.get(id).copied().unwrap_or(0))
            .collect();
        let mut probe = 0;
        self.request(|log| {
            probe = log.probes.len();
            log.probes.push(heights);
        })
        .await;
        if let Some(Script::Answer(answer)) = self.script.get(probe) {
            return answer.clone();
        }
        ids.iter()
            .filter_map(|id| self.stored.get(id).map(|&height| (height, id)))
            .max_by_key(|(height, _)| *height)
            .map(|(height, id)| CommonBlock {
                height,
                id: id.to_hex(),
            })
    }

    async fn headers(&self, _peer: &SharedPeer, from: u32, limit: u32) -> Vec<DownloadedBlock> {
        self.request(|log| log.fetched.push((from, limit))).await;
        let count = limit.min(self.per_reply.unwrap_or(u32::MAX));
        self.served
            .iter()
            .skip(usize::try_from(from).unwrap_or(usize::MAX))
            .take(usize::try_from(count).unwrap_or(usize::MAX))
            .map(|header| DownloadedBlock {
                header: header.clone(),
                transactions: None,
                ip: IpAddr::from([192, 0, 2, 7]),
            })
            .collect()
    }
}

fn keys_of(value: &Value, what: &str) -> Outcome<Vec<PublicKeyBytes>> {
    value
        .as_array()
        .ok_or_else(|| failed(format!("{what} is not a list")))?
        .iter()
        .map(|key| {
            key.as_str()
                .and_then(|key| PublicKeyBytes::from_hex(key).ok())
                .ok_or_else(|| failed(format!("{what} has a key that is not one")))
        })
        .collect()
}

fn script_of(value: Option<&Value>) -> Outcome<Vec<Script>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| failed("peer.common is not a list"))?;
    entries
        .iter()
        .map(|entry| match entry {
            Value::String(text) if text == "honest" => Ok(Script::Honest),
            Value::Null => Ok(Script::Answer(None)),
            Value::Object(common) => Ok(Script::Answer(Some(CommonBlock {
                height: common
                    .get("height")
                    .and_then(Value::as_u64)
                    .and_then(|h| u32::try_from(h).ok())
                    .ok_or_else(|| failed("a scripted height is not one"))?,
                id: common
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| failed("a scripted id is not text"))?
                    .to_owned(),
            }))),
            _ => Err(failed("peer.common has an entry that is not one")),
        })
        .collect()
}

fn check_state(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let milestones = env.milestones(case)?;
    let milestones: &Milestones = &milestones;
    let network = env.devnet()?.network.clone();

    let chain = input["chain"]
        .as_array()
        .filter(|chain| !chain.is_empty())
        .ok_or_else(|| failed("input.chain lists no header"))?;
    let mut blocks = Vec::with_capacity(chain.len());
    for (i, hex) in chain.iter().enumerate() {
        let what = format!("chain[{i}]");
        let text = hex
            .as_str()
            .ok_or_else(|| failed(format!("{what} is not text")))?;
        let bytes = hex::decode(text).map_err(|e| failed(format!("{what}: {e}")))?;
        let block = Block::decode_trusted(&bytes, milestones)
            .map_err(|e| failed(format!("{what}: {e}")))?;
        blocks.push(Arc::new(block));
    }
    let ours_ids: Vec<BlockId> = blocks.iter().map(|b| *b.id()).collect();
    let our_heights: HashMap<BlockId, u32> = blocks.iter().map(|b| (*b.id(), b.height())).collect();

    let mut delegates = HashMap::new();
    for entry in input["delegates"].as_array().into_iter().flatten() {
        let key = entry[0]
            .as_str()
            .and_then(|key| PublicKeyBytes::from_hex(key).ok())
            .ok_or_else(|| failed("input.delegates has a key that is not one"))?;
        delegates.insert(key, entry[1].as_str().unwrap_or_default().to_owned());
    }
    let mut rounds = BTreeMap::new();
    for entry in input["rounds"].as_array().into_iter().flatten() {
        let round = entry[0]
            .as_u64()
            .ok_or_else(|| failed("input.rounds has a round that is not one"))?;
        rounds.insert(round, keys_of(&entry[1], "input.rounds")?);
    }

    let peer = &input["peer"];
    let fork: u32 = integer(peer, "fork")?;
    let fork_index = usize::try_from(fork).map_err(|_| failed("peer.fork"))?;
    if fork_index > blocks.len() {
        return Err(failed("peer.fork must be a height of our chain"));
    }
    let branch: Vec<Sent> = peer["branch"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, header)| sent(header, &format!("peer.branch[{i}]"), milestones))
        .collect::<Outcome<_>>()?;
    let mut served = Vec::new();
    let mut stored = HashMap::new();
    // The ids of the peer's chain, as it signed its blocks: the block before each height.
    let mut signed_ids = Vec::new();
    for block in blocks.iter().take(fork_index) {
        served.push(listed(&Sent {
            header: block.header().clone(),
            set: Map::new(),
        })?);
        stored.insert(*block.id(), block.height());
        signed_ids.push(*block.id());
    }
    for (i, header) in branch.iter().enumerate() {
        served.push(listed(header)?);
        let id = block_id(&header.header);
        stored.insert(
            id,
            fork.saturating_add(u32::try_from(i).unwrap_or(u32::MAX))
                .saturating_add(1),
        );
        signed_ids.push(id);
    }

    let claimed = &input["claimed"];
    let claimed_height: u32 = integer(claimed, "height")?;
    let claimed = state(
        claimed_height,
        listed(&sent(&claimed["header"], "claimed.header", milestones)?)?,
    );

    let allowed = match input.get("deadline") {
        None | Some(Value::Null) => None,
        Some(_) => Some(integer::<usize>(input, "deadline")?),
    };
    // The time on the runtime's paused clock, which the verifier and the fake peer read.
    let start = block_on(async { Instant::now() })?;
    let deadline = match allowed {
        Some(0) => Deadline(start),
        Some(_) => Deadline(start + DEADLINE),
        None => Deadline(start + Duration::from_secs(3_600)),
    };
    let fake = FakePeer {
        served,
        stored,
        our_heights,
        script: script_of(peer.get("common"))?,
        per_reply: match peer.get("perReply") {
            None | Some(Value::Null) => None,
            Some(_) => Some(integer(peer, "perReply")?),
        },
        allowed,
        deadline: deadline.0,
        log: Mutex::new(Log::default()),
    };
    let last = blocks
        .last()
        .cloned()
        .ok_or_else(|| failed("input.chain lists no header"))?;
    let ours = Arc::new(Ours {
        last,
        blocks,
        delegates,
        rounds,
        milestones: milestones.clone(),
        network,
    });
    let verifier = PeerVerifier::new(ours.clone(), ours, Arc::new(ManualClock::new(0)));
    let shared = SharedPeer::new(Peer::new(IpAddr::from([192, 0, 2, 7]), 4002));

    let fast = input["fast"].as_bool().unwrap_or(false);
    let result = if fast {
        match verifier.verify_fast(&claimed) {
            Err(_) => json!("stopped"),
            Ok(None) => Value::Null,
            Ok(Some(v)) => {
                let mut out = json!({"myHeight": v.my_height, "theirHeight": v.their_height});
                if let Some(common) = v.highest_common_height {
                    out["highestCommonHeight"] = json!(common);
                }
                out
            }
        }
    } else {
        match block_on(verifier.verify(&fake, &shared, &claimed, deadline))? {
            Checked::Verified(v) => json!({
                "myHeight": v.my_height,
                "theirHeight": v.their_height,
                "highestCommonHeight": v.highest_common_height,
            }),
            Checked::Refused(Refusal::Unlinked) => {
                let log = fake.log.lock().unwrap_or_else(PoisonError::into_inner);
                return Ok(Verdict::RefusedByDesign {
                    difference: Documented::UnlinkedForkSegment,
                    holds: unlinked(&log.fetched, &fake.served, &ours_ids, &signed_ids),
                });
            }
            Checked::Refused(_) => Value::Null,
            Checked::Stopped(_) => json!("stopped"),
        }
    };
    let log = fake
        .log
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    let fetched: Vec<[u32; 2]> = log
        .fetched
        .iter()
        .map(|&(from, limit)| [from, limit])
        .collect();
    Ok(Verdict::Output(json!({
        "result": result,
        "probes": log.probes,
        "fetched": fetched,
    })))
}

/// The predicate of the unlinked fork segment: from the first height fetched to the last one
/// served, some header's previous id (as sent) is not the id of the block before it: our block at
/// the common height for the first, then the peer's block as the input signs it.
fn unlinked(
    fetched: &[(u32, u32)],
    served: &[ListedBlock],
    ours: &[BlockId],
    signed: &[BlockId],
) -> bool {
    let Some(&(common, _)) = fetched.first() else {
        return false;
    };
    let last = fetched
        .iter()
        .map(|&(from, limit)| from.saturating_add(limit))
        .max()
        .unwrap_or(common);
    let at = |ids: &[BlockId], height: u32| {
        usize::try_from(height)
            .ok()
            .and_then(|h| h.checked_sub(1))
            .and_then(|i| ids.get(i).copied())
    };
    (common.saturating_add(1)..=last).any(|height| {
        let before = if height == common.saturating_add(1) {
            at(ours, common)
        } else {
            at(signed, height.saturating_sub(1))
        };
        let Some(block) = usize::try_from(height)
            .ok()
            .and_then(|h| h.checked_sub(1))
            .and_then(|i| served.get(i))
        else {
            return false;
        };
        let previous = block
            .header
            .previous_block
            .as_deref()
            .and_then(|text| hex::decode_array::<32>(text).ok());
        before.is_some() && previous != before.map(|id| *id.as_bytes())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_runs_futures() {
        assert_eq!(block_on(async { 7 }).expect("a runtime"), 7);
    }

    #[test]
    fn a_scripted_reply_reads_each_form() {
        let script =
            script_of(Some(&json!(["honest", null, {"height": 3, "id": "ab"}]))).expect("a script");
        assert!(matches!(script[0], Script::Honest));
        assert!(matches!(script[1], Script::Answer(None)));
        assert!(matches!(&script[2], Script::Answer(Some(c)) if c.height == 3 && c.id == "ab"));
    }
}
