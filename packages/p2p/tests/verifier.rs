//! The peer verifier against a fake link that serves the committed devnet chain
//! (`tools/difftest/corpus/txgen-one-node`: 265 blocks, rounds of 53 delegates) and branches of it
//! signed by the chain's own delegate keys: the same chain, a peer behind or ahead, forks below our
//! height and across a round end, bogus common-block replies, fork segments that fail their checks
//! or do not chain, short replies and the deadline. Time is tokio's paused clock.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use heartwood_crypto::blocks::factory;
use heartwood_crypto::blocks::serialiser::{block_id, signing_hash};
use heartwood_crypto::crypto::sig::{self, SchemeId, SigningDomain};
use heartwood_crypto::genesis::derive::role_key;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{
    Aux, Block, BlockHeader, BlockId, KeyPair, Milestones, Network, PublicKeyBytes,
};
use heartwood_kernel::{ManualClock, RoundInfo, calculate_round};
use heartwood_p2p::download::DownloadedBlock;
use heartwood_p2p::errors::{LookupError, VerificationStopped};
use heartwood_p2p::seams::{Deadline, StateCheck, VerifierLink};
use heartwood_p2p::verifier::{Outcome, PeerVerifier, Refusal};
use heartwood_p2p::view::{BlockLookup, BlockPing, ChainView, DelegateInfo};
use heartwood_p2p::{FastVerification, Peer, PeerState, SharedPeer, Verification};
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;
use heartwood_p2p_wire::messages::blocks::BlockHeader as ListedHeader;
use heartwood_p2p_wire::messages::peer::StatusHeader;
use heartwood_p2p_wire::replies::CommonBlock;
use serde_json::Value;
use tokio::time::Instant;

/// The committed chain, from this crate's directory.
const CORPUS: &str = "../../tools/difftest/corpus/txgen-one-node";
/// The wall-clock time of the violations the verifier records.
const NOW_MS: i64 = 1_800_000_000_000;

/// The committed chain: its blocks, the round rows its node stored, and its delegates' keys.
struct Chain {
    network: Network,
    milestones: Milestones,
    blocks: Vec<Arc<Block>>,
    heights: HashMap<BlockId, u32>,
    rounds: BTreeMap<u64, Vec<PublicKeyBytes>>,
    keys: HashMap<PublicKeyBytes, KeyPair>,
}

impl Chain {
    fn block(&self, height: u32) -> &Arc<Block> {
        &self.blocks[height as usize - 1]
    }

    fn header(&self, height: u32) -> BlockHeader {
        self.block(height).header().clone()
    }

    fn id(&self, height: u32) -> BlockId {
        *self.block(height).id()
    }

    /// The delegates of the round of `height`.
    fn delegates(&self, height: u32) -> &[PublicKeyBytes] {
        &self.rounds[&calculate_round(height, &self.milestones).round]
    }

    /// The keys of the `n`th delegate of the round of `height` whose keys the seed derives.
    fn signer(&self, height: u32, n: usize) -> &KeyPair {
        let mut known = self
            .delegates(height)
            .iter()
            .filter_map(|k| self.keys.get(k));
        let count = known.clone().count();
        known.nth(n % count).expect("a derived round delegate")
    }

    /// Whether `key` is a delegate the chain registered.
    fn registered(&self, key: &PublicKeyBytes) -> bool {
        self.keys.contains_key(key) || self.rounds.values().flatten().any(|k| k == key)
    }
}

static C1: LazyLock<Chain> = LazyLock::new(|| {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(CORPUS);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).expect(name);
    let json = |line: &str| serde_json::from_str::<Value>(line).expect("a JSON line");
    let network = Network::load(&read("crypto/network.json")).expect("the network loads");
    let milestones =
        Milestones::load(&read("crypto/milestones.json"), &network).expect("the milestones load");
    let genesis = factory::from_json(&json(&read("crypto/genesisBlock.json")), &milestones)
        .expect("the genesis block loads");
    let mut blocks = vec![Arc::new(genesis)];
    for line in read("blocks.jsonl").lines() {
        let bytes = hex::decode(json(line)["hex"].as_str().expect("hex")).expect("block hex");
        let block = Block::decode_trusted(&bytes, &milestones).expect("a stored block decodes");
        blocks.push(Arc::new(block));
    }
    let heights = blocks
        .iter()
        .map(|block| (*block.id(), block.height()))
        .collect();
    let mut rounds: BTreeMap<u64, Vec<PublicKeyBytes>> = BTreeMap::new();
    for line in read("reference/rounds.jsonl").lines() {
        let row = json(line);
        let key = PublicKeyBytes::from_hex(row["publicKey"].as_str().expect("key")).expect("key");
        rounds
            .entry(row["round"].as_u64().expect("round"))
            .or_default()
            .push(key);
    }
    // The genesis delegates, and the wallets that registered as delegates on the chain.
    let roles = (1..=53)
        .map(|k| format!("delegate/{k}"))
        .chain((1..=20).map(|k| format!("wallet/{k}")));
    let keys: HashMap<PublicKeyBytes, KeyPair> = roles
        .map(|role| {
            let derived = role_key(
                "solar-devnet-v1/",
                &role,
                "solar-devnet",
                network.pub_key_hash,
            )
            .expect("a delegate key");
            (derived.keys.public_key().to_compressed(), derived.keys)
        })
        .collect();
    let known = rounds
        .values()
        .flatten()
        .filter(|&k| keys.contains_key(k))
        .count();
    assert!(
        known >= 53 * 5,
        "the derived keys sign most of the rounds' slots"
    );
    Chain {
        network,
        milestones,
        blocks,
        heights,
        rounds,
        keys,
    }
});

/// A key that no chain registered.
fn stranger() -> KeyPair {
    KeyPair::from_passphrase("verifier test: not a delegate").expect("a key")
}

/// A registered delegate that is in no round of the chain.
fn standby() -> KeyPair {
    KeyPair::from_passphrase("verifier test: a standby delegate").expect("a key")
}

/// `header` signed by `keys`, which become its generator.
fn signed_by(mut header: BlockHeader, keys: &KeyPair) -> BlockHeader {
    header.generator_public_key = keys.public_key().to_compressed();
    header.block_signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &signing_hash(&header),
        keys.secret_key(),
        Aux::random(),
    )
    .expect("a signature");
    header
}

/// The chain as the peer has it: ours up to `fork`, then its own blocks up to `top`, each signed
/// by a delegate of its round, eight seconds apart.
fn peer_chain(fork: u32, top: u32) -> Vec<BlockHeader> {
    let mut chain: Vec<BlockHeader> = (1..=fork.min(top)).map(|h| C1.header(h)).collect();
    for height in fork + 1..=top {
        let previous = chain.last().expect("a chain has a genesis block");
        let keys = C1.signer(height, height as usize);
        let draft = BlockHeader {
            timestamp: previous.timestamp + 8,
            height,
            previous_block: *block_id(previous).as_bytes(),
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            payload_length: 0,
            ..C1.header(2)
        };
        chain.push(signed_by(draft, keys));
    }
    chain
}

/// A header as a getBlocks reply lists it once checked.
fn listed(header: &BlockHeader) -> ListedBlock {
    let amount = |value: u64| Amount::new(i128::from(value));
    ListedBlock {
        header: ListedHeader {
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
            transactions: Some(Vec::new()),
            ..ListedHeader::default()
        },
        total_amount: amount(header.total_amount),
        total_fee: amount(header.total_fee),
        reward: amount(header.reward),
        transactions: Vec::new(),
    }
}

/// The state of a status whose last block is `header`, at `height`.
fn state_at(height: u32, header: &BlockHeader) -> PeerState {
    let listed = listed(header).header;
    PeerState {
        height,
        forging_allowed: false,
        current_slot: 0,
        header: StatusHeader {
            id: listed.id,
            version: listed.version,
            timestamp: listed.timestamp,
            previous_block: listed.previous_block,
            height: listed.height,
            number_of_transactions: listed.number_of_transactions,
            total_amount: listed.total_amount,
            total_fee: listed.total_fee,
            reward: listed.reward,
            payload_length: listed.payload_length,
            payload_hash: listed.payload_hash,
            generator_public_key: listed.generator_public_key,
            block_signature: listed.block_signature,
            ..StatusHeader::default()
        },
        total_amount: Amount::new(i128::from(header.total_amount)),
        total_fee: Amount::new(i128::from(header.total_fee)),
        reward: Amount::new(i128::from(header.reward)),
    }
}

/// The state the peer reports: the last block of its chain.
fn claimed(chain: &[BlockHeader]) -> PeerState {
    let top = chain.last().expect("a chain");
    state_at(top.height, top)
}

/// Our node: the committed chain up to `height`, with the round rows stored by then.
struct Ours {
    height: u32,
    standby: PublicKeyBytes,
}

impl ChainView for Ours {
    fn last_block(&self) -> Arc<Block> {
        C1.block(self.height).clone()
    }

    fn last_blocks(&self) -> Vec<Arc<Block>> {
        let from = self.height.saturating_sub(99).max(1);
        (from..=self.height).map(|h| C1.block(h).clone()).collect()
    }

    fn milestones(&self) -> &Milestones {
        &C1.milestones
    }

    fn network(&self) -> &Network {
        &C1.network
    }

    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
        (C1.registered(key) || *key == self.standby).then(|| DelegateInfo {
            username: "delegate".into(),
            rank: None,
        })
    }

    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        C1.keys.keys().copied().chain([self.standby]).collect()
    }

    fn is_synced(&self) -> bool {
        true
    }

    fn is_idle(&self) -> bool {
        true
    }

    fn block_ping(&self) -> Option<BlockPing> {
        None
    }
}

impl BlockLookup for Ours {
    fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockHeader>, LookupError> {
        heights
            .iter()
            .map(|&h| {
                (1..=self.height)
                    .contains(&h)
                    .then(|| C1.header(h))
                    .ok_or_else(|| LookupError(format!("no block at {h}")))
            })
            .collect()
    }

    fn round_delegate_keys(&self, round: RoundInfo) -> Result<Vec<PublicKeyBytes>, LookupError> {
        // The node stores a round's rows when the round before it ends.
        let stored = calculate_round(self.height + 1, &C1.milestones).round;
        (round.round <= stored)
            .then(|| C1.rounds.get(&round.round).cloned())
            .flatten()
            .ok_or_else(|| LookupError(format!("round {} is not stored", round.round)))
    }
}

fn verifier(height: u32) -> PeerVerifier {
    let ours = Arc::new(Ours {
        height,
        standby: standby().public_key().to_compressed(),
    });
    PeerVerifier::new(ours.clone(), ours, Arc::new(ManualClock::new(NOW_MS)))
}

fn a_peer() -> SharedPeer {
    SharedPeer::new(Peer::new(IpAddr::from([192, 0, 2, 7]), 4002))
}

fn violations(peer: &SharedPeer) -> usize {
    peer.snapshot().violations.len()
}

/// A request the verifier made.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    /// getCommonBlocks: the heights of our ids asked about, and the timeout.
    Common(Vec<u32>, Duration),
    /// getBlocks of headers after a height, at most so many.
    Headers(u32, u32),
}

/// A getCommonBlocks answer of the script.
#[derive(Debug, Clone)]
enum Reply {
    /// The answer of a peer with the served chain.
    Honest,
    /// This answer.
    Answer(Option<CommonBlock>),
}

/// The peer: it serves `served` (its chain from height 1), answers getCommonBlocks from the script
/// and then honestly, returns at most `per_reply` headers per getBlocks, and waits before the
/// request whose index `stall` names.
struct Link {
    served: Vec<ListedBlock>,
    script: Vec<Reply>,
    per_reply: u32,
    stall: Option<(usize, Duration)>,
    asked: Mutex<Vec<Asked>>,
}

impl Link {
    fn serving(chain: &[BlockHeader]) -> Link {
        Link {
            served: chain.iter().map(listed).collect(),
            script: Vec::new(),
            per_reply: u32::MAX,
            stall: None,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().expect("the log").clone()
    }

    /// Log `asked`, then wait if this request is the one to stall. Returns the number of
    /// getCommonBlocks asked before this one.
    async fn log(&self, asked: Asked) -> usize {
        let (index, probe) = {
            let mut log = self.asked.lock().expect("the log");
            let probes = log
                .iter()
                .filter(|a| matches!(a, Asked::Common(..)))
                .count();
            log.push(asked);
            (log.len() - 1, probes)
        };
        if let Some((at, wait)) = self.stall
            && at == index
        {
            tokio::time::sleep(wait).await;
        }
        probe
    }
}

impl VerifierLink for Link {
    async fn common_blocks(
        &self,
        _peer: &SharedPeer,
        ids: &[BlockId],
        timeout: Duration,
    ) -> Option<CommonBlock> {
        let heights = ids.iter().map(|id| C1.heights[id]).collect();
        let probe = self.log(Asked::Common(heights, timeout)).await;
        if let Some(Reply::Answer(answer)) = self.script.get(probe) {
            return answer.clone();
        }
        let hexes: Vec<String> = ids.iter().map(BlockId::to_hex).collect();
        self.served
            .iter()
            .filter(|block| hexes.contains(block.header.id.as_ref().expect("an id")))
            .map(|block| CommonBlock {
                height: block.header.height.expect("a height"),
                id: block.header.id.clone().expect("an id"),
            })
            .max_by_key(|common| common.height)
    }

    async fn headers(&self, _peer: &SharedPeer, from: u32, limit: u32) -> Vec<DownloadedBlock> {
        self.log(Asked::Headers(from, limit)).await;
        self.served
            .iter()
            .skip(from as usize)
            .take(limit.min(self.per_reply) as usize)
            .map(|header| DownloadedBlock {
                header: header.clone(),
                transactions: None,
                ip: IpAddr::from([192, 0, 2, 7]),
            })
            .collect()
    }
}

fn verified(my_height: u32, their_height: u32, common: u32) -> Outcome {
    Outcome::Verified(Verification {
        my_height,
        their_height,
        highest_common_height: common,
    })
}

fn far() -> Deadline {
    Deadline::after(Duration::from_secs(60))
}

/// Verify `chain`'s last block against our chain at `ours`, served by `link`.
async fn run(ours: u32, link: &Link, chain: &[BlockHeader], peer: &SharedPeer) -> Outcome {
    verifier(ours)
        .verify(link, peer, &claimed(chain), far())
        .await
}

#[tokio::test(start_paused = true)]
async fn the_same_chain_verifies_without_a_request() {
    let chain = peer_chain(200, 200);
    let link = Link::serving(&chain);
    let peer = a_peer();
    let outcome = run(200, &link, &chain, &peer).await;
    assert_eq!(outcome, verified(200, 200, 200));
    assert!(link.asked().is_empty());
    let checked = verifier(200)
        .check_state(&link, &peer, &claimed(&chain), far())
        .await;
    assert!(checked.expect("no deadline").is_some_and(|v| !v.forked()));
}

#[tokio::test(start_paused = true)]
async fn a_peer_behind_on_our_chain_verifies_at_its_height() {
    let chain = peer_chain(150, 150);
    let link = Link::serving(&chain);
    assert_eq!(
        run(200, &link, &chain, &a_peer()).await,
        verified(200, 150, 150)
    );
    assert!(link.asked().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_peer_ahead_on_our_chain_is_checked_to_the_end_of_our_round() {
    let chain = peer_chain(200, 200);
    let link = Link::serving(&chain);
    let peer = a_peer();
    let outcome = run(150, &link, &chain, &peer).await;
    assert_eq!(outcome, verified(150, 200, 150));
    let probe = vec![1, 20, 38, 57, 76, 94, 113, 131, 150];
    let timeout = Duration::from_secs(60);
    // Heights 151 to 159 end round 3.
    assert_eq!(
        link.asked(),
        vec![Asked::Common(probe, timeout), Asked::Headers(150, 9)]
    );
    assert_eq!(violations(&peer), 0);
}

#[tokio::test(start_paused = true)]
async fn a_peer_forked_below_our_height_verifies_with_the_fork_height() {
    let chain = peer_chain(180, 190);
    let link = Link::serving(&chain);
    let peer = a_peer();
    let outcome = run(200, &link, &chain, &peer).await;
    assert_eq!(outcome, verified(200, 190, 180));
    let Outcome::Verified(result) = outcome else {
        unreachable!()
    };
    assert!(result.forked());
    let heights: Vec<Vec<u32>> = link
        .asked()
        .into_iter()
        .filter_map(|asked| match asked {
            Asked::Common(heights, _) => Some(heights),
            Asked::Headers(..) => None,
        })
        .collect();
    assert_eq!(
        heights,
        vec![
            vec![1, 25, 48, 72, 96, 119, 143, 166, 190],
            vec![167, 170, 173, 175, 178, 181, 184, 186, 189],
            vec![179, 180],
        ]
    );
    assert_eq!(link.asked().last(), Some(&Asked::Headers(180, 10)));
    assert_eq!(violations(&peer), 0);
}

#[tokio::test(start_paused = true)]
async fn a_fork_across_a_round_end_is_checked_to_the_end_of_the_common_blocks_round() {
    // Forked in round 3 and running into round 4: only round 3's blocks are checked.
    let chain = peer_chain(150, 175);
    let link = Link::serving(&chain);
    assert_eq!(
        run(200, &link, &chain, &a_peer()).await,
        verified(200, 175, 150)
    );
    assert_eq!(link.asked().last(), Some(&Asked::Headers(150, 9)));
    // Forked at the last block of round 3: round 4's delegates sign the segment.
    let chain = peer_chain(159, 170);
    let link = Link::serving(&chain);
    assert_eq!(
        run(200, &link, &chain, &a_peer()).await,
        verified(200, 170, 159)
    );
    assert_eq!(link.asked().last(), Some(&Asked::Headers(159, 11)));
}

#[tokio::test(start_paused = true)]
async fn a_fork_at_our_height_needs_only_its_header_s_own_signature() {
    let chain = peer_chain(195, 200);
    let link = Link::serving(&chain);
    assert_eq!(
        run(200, &link, &chain, &a_peer()).await,
        verified(200, 200, 195)
    );
    // Above our height a header by a key that is no delegate passes the header check, as in the
    // reference: its segment is what is verified.
    let mut chain = peer_chain(200, 200);
    let top = chain.pop().expect("a block");
    chain.push(signed_by(top, &stranger()));
    let link = Link::serving(&chain);
    assert_eq!(
        run(150, &link, &chain, &a_peer()).await,
        verified(150, 200, 150)
    );
}

#[tokio::test(start_paused = true)]
async fn a_claimed_header_that_fails_ends_the_verification_at_once() {
    let chain = peer_chain(200, 200);
    let top = chain.last().expect("a block").clone();
    let peer = a_peer();
    let refused = Outcome::Refused(Refusal::Header);
    // A header height that is not the state's height.
    let link = Link::serving(&chain);
    let verifier = verifier(200);
    let state = state_at(201, &top);
    assert_eq!(verifier.verify(&link, &peer, &state, far()).await, refused);
    // A signature that fails, at our height.
    let mut forged = top.clone();
    forged.timestamp += 1;
    assert_eq!(
        verifier
            .verify(&link, &peer, &state_at(200, &forged), far())
            .await,
        refused
    );
    // Below our height: a registered delegate outside the round, and a key that is no delegate.
    for keys in [standby(), stranger()] {
        let header = signed_by(C1.header(150), &keys);
        assert_eq!(
            verifier
                .verify(&link, &peer, &state_at(150, &header), far())
                .await,
            refused
        );
    }
    assert!(link.asked().is_empty());
    assert_eq!(violations(&peer), 0);
}

#[tokio::test(start_paused = true)]
async fn a_common_block_reply_that_names_what_was_not_asked_is_a_violation() {
    let chain = peer_chain(180, 190);
    let foreign = CommonBlock {
        height: 25,
        id: "ab".repeat(32),
    };
    let elsewhere = CommonBlock {
        height: 26,
        id: C1.id(25).to_hex(),
    };
    let upper = CommonBlock {
        height: 25,
        id: C1.id(25).to_hex().to_uppercase(),
    };
    for bogus in [foreign, elsewhere, upper] {
        let mut link = Link::serving(&chain);
        link.script = vec![Reply::Answer(Some(bogus))];
        let peer = a_peer();
        assert_eq!(
            run(200, &link, &chain, &peer).await,
            Outcome::Refused(Refusal::NoCommonBlock)
        );
        assert_eq!(link.asked().len(), 1);
        assert_eq!(violations(&peer), 1);
    }
    // No common block at all is no violation.
    let mut link = Link::serving(&chain);
    link.script = vec![Reply::Answer(None)];
    let peer = a_peer();
    assert_eq!(
        run(200, &link, &chain, &peer).await,
        Outcome::Refused(Refusal::NoCommonBlock)
    );
    assert_eq!(violations(&peer), 0);
}

#[tokio::test(start_paused = true)]
async fn a_later_bogus_reply_keeps_the_common_block_found_before_it() {
    let chain = peer_chain(180, 190);
    let mut link = Link::serving(&chain);
    link.script = vec![
        Reply::Honest,
        Reply::Answer(Some(CommonBlock {
            height: 170,
            id: "cd".repeat(32),
        })),
    ];
    let peer = a_peer();
    // The first probe found 166, and the bogus reply ended the search there. The segment from 167
    // is our chain up to the peer's branch at 181: the reference verifies the peer as forked at
    // 166, Heartwood refuses it.
    assert_eq!(
        run(200, &link, &chain, &peer).await,
        Outcome::Refused(Refusal::OwnSegment)
    );
    assert_eq!(link.asked().last(), Some(&Asked::Headers(166, 24)));
    assert_eq!(violations(&peer), 1);
}

#[tokio::test(start_paused = true)]
async fn a_common_block_below_where_the_peer_leaves_our_chain_is_refused() {
    let chain = peer_chain(180, 190);
    let mut link = Link::serving(&chain);
    link.script = vec![Reply::Answer(Some(CommonBlock {
        height: 25,
        id: C1.id(25).to_hex(),
    }))];
    let peer = a_peer();
    // The search goes on in [26, 47], where the peer has everything; round 1 ends at 53. The
    // peer's block 48 is ours, so 47 is not its highest common block: the reference verifies it
    // as forked at 47, Heartwood refuses it. A probe of an honest peer that failed leads here
    // too, so it is no violation.
    assert_eq!(
        run(200, &link, &chain, &peer).await,
        Outcome::Refused(Refusal::OwnSegment)
    );
    assert_eq!(link.asked().last(), Some(&Asked::Headers(47, 6)));
    assert_eq!(violations(&peer), 0);
}

#[tokio::test(start_paused = true)]
async fn a_peer_on_our_chain_cannot_pass_as_forked_below_it() {
    // Our chain to 200 and a block 201 signed by a key that is no delegate's: the peer is not
    // forked, but claims so with a low first common block.
    let mut chain = peer_chain(200, 201);
    let top = chain.pop().expect("a block");
    chain.push(signed_by(top, &stranger()));
    let mut link = Link::serving(&chain);
    // The first probe asks about 1, 26, 51, ... 200.
    link.script = vec![Reply::Answer(Some(CommonBlock {
        height: 26,
        id: C1.id(26).to_hex(),
    }))];
    let peer = a_peer();
    assert_eq!(
        run(200, &link, &chain, &peer).await,
        Outcome::Refused(Refusal::OwnSegment)
    );
    assert_eq!(violations(&peer), 0);
    // Without the low reply it is ahead on our chain: its block 201 is what is verified, and a
    // key that is no delegate's stops the ping.
    let link = Link::serving(&chain);
    assert_eq!(
        run(200, &link, &chain, &peer).await,
        Outcome::Stopped(VerificationStopped::Header)
    );
}

/// A change to a listed header.
type Change = Box<dyn FnOnce(&mut ListedBlock)>;

/// The peer forked at 180 up to 190, with header 185 replaced by `change` of it.
async fn segment_with(change: impl FnOnce(&mut ListedBlock)) -> (Outcome, SharedPeer, Link) {
    let chain = peer_chain(180, 190);
    let mut link = Link::serving(&chain);
    change(&mut link.served[184]);
    let peer = a_peer();
    let outcome = run(200, &link, &chain, &peer).await;
    (outcome, peer, link)
}

fn resign(block: &mut ListedBlock, keys: &KeyPair, edit: impl FnOnce(&mut BlockHeader)) {
    let mut header = header_of(block);
    edit(&mut header);
    *block = listed(&signed_by(header, keys));
}

/// The header a listed block of these tests carries.
fn header_of(block: &ListedBlock) -> BlockHeader {
    let h = &block.header;
    let bytes = |text: &Option<String>| hex::decode(text.as_deref().expect("hex")).expect("hex");
    BlockHeader {
        version: h.version.expect("version"),
        timestamp: h.timestamp.expect("timestamp"),
        height: h.height.expect("height"),
        previous_block: bytes(&h.previous_block).try_into().expect("32 bytes"),
        number_of_transactions: h.number_of_transactions.expect("count"),
        total_amount: u64::try_from(block.total_amount.value()).expect("an amount"),
        total_fee: u64::try_from(block.total_fee.value()).expect("an amount"),
        reward: u64::try_from(block.reward.value()).expect("an amount"),
        payload_length: h.payload_length.expect("length"),
        payload_hash: bytes(&h.payload_hash).try_into().expect("32 bytes"),
        generator_public_key: PublicKeyBytes::from_hex(
            h.generator_public_key.as_deref().expect("key"),
        )
        .expect("key"),
        block_signature: heartwood_crypto::Signature::from_hex(
            h.block_signature.as_deref().expect("signature"),
        )
        .expect("signature"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_fork_segment_header_that_fails_its_checks_is_refused_and_a_violation() {
    let round_delegate = C1.signer(185, 0);
    // A signature that fails.
    let (outcome, peer, _) = segment_with(|block| {
        block.header.timestamp = block.header.timestamp.map(|t| t + 1);
    })
    .await;
    assert_eq!(outcome, Outcome::Refused(Refusal::Segment));
    assert_eq!(violations(&peer), 1);
    // Another height than the one asked.
    let (outcome, peer, _) =
        segment_with(|block| resign(block, round_delegate, |h| h.height = 186)).await;
    assert_eq!(outcome, Outcome::Refused(Refusal::Segment));
    assert_eq!(violations(&peer), 1);
    // A registered delegate that is not one of the round's.
    let (outcome, peer, _) = segment_with(|block| resign(block, &standby(), |_| {})).await;
    assert_eq!(outcome, Outcome::Refused(Refusal::Segment));
    assert_eq!(violations(&peer), 1);
}

#[tokio::test(start_paused = true)]
async fn a_fork_segment_header_the_reference_throws_on_stops_the_ping() {
    // A key that is no delegate, a version the reference names no generator for, a payload hash
    // that is not 32 bytes: the reference throws out of the ping.
    let changes: [Change; 3] = [
        Box::new(|block| resign(block, &stranger(), |_| {})),
        Box::new(|block| block.header.version = Some(1)),
        Box::new(|block| {
            let hash = block.header.payload_hash.as_mut().expect("a hash");
            hash.truncate(62);
        }),
    ];
    for change in changes {
        let (outcome, peer, _) = segment_with(change).await;
        assert_eq!(outcome, Outcome::Stopped(VerificationStopped::Header));
        assert_eq!(violations(&peer), 1);
    }
    // Through the seam, the ping fails and the peer keeps its last verification.
    let chain = peer_chain(180, 190);
    let mut link = Link::serving(&chain);
    resign(&mut link.served[184], &stranger(), |_| {});
    let checked = verifier(200)
        .check_state(&link, &a_peer(), &claimed(&chain), far())
        .await;
    assert_eq!(checked, Err(VerificationStopped::Header));
    // Node reads hex in pairs: an odd last digit is dropped, so 65 digits still make 32 bytes.
    let (outcome, _, _) = segment_with(|block| {
        block
            .header
            .payload_hash
            .as_mut()
            .expect("a hash")
            .push('7');
    })
    .await;
    assert_eq!(outcome, verified(200, 190, 180));
}

#[tokio::test(start_paused = true)]
async fn a_fork_segment_that_does_not_chain_is_refused() {
    let round_delegate = |height| C1.signer(height, 3);
    // Header 185 is signed by a delegate of the round, at its height, but names another parent.
    let (outcome, peer, _) =
        segment_with(|block| resign(block, round_delegate(185), |h| h.previous_block = [7; 32]))
            .await;
    assert_eq!(outcome, Outcome::Refused(Refusal::Unlinked));
    assert_eq!(violations(&peer), 1);
    // The first header must chain to our block at the common height.
    let chain = peer_chain(180, 190);
    let mut link = Link::serving(&chain);
    resign(&mut link.served[180], round_delegate(181), |h| {
        h.previous_block = *C1.id(179).as_bytes();
    });
    let peer = a_peer();
    assert_eq!(
        run(200, &link, &chain, &peer).await,
        Outcome::Refused(Refusal::Unlinked)
    );
    let checked = verifier(200)
        .check_state(&link, &peer, &claimed(&chain), far())
        .await;
    assert_eq!(checked, Ok(None));
}

#[tokio::test(start_paused = true)]
async fn short_replies_are_fetched_again_from_the_first_height_missing() {
    let chain = peer_chain(180, 190);
    let mut link = Link::serving(&chain);
    link.per_reply = 3;
    assert_eq!(
        run(200, &link, &chain, &a_peer()).await,
        verified(200, 190, 180)
    );
    let fetches: Vec<Asked> = link
        .asked()
        .into_iter()
        .filter(|asked| matches!(asked, Asked::Headers(..)))
        .collect();
    assert_eq!(
        fetches,
        vec![
            Asked::Headers(180, 10),
            Asked::Headers(183, 7),
            Asked::Headers(186, 4),
            Asked::Headers(189, 1)
        ]
    );
    // A peer that sends no header is refused, and that is no violation.
    let mut link = Link::serving(&chain);
    link.per_reply = 0;
    let peer = a_peer();
    assert_eq!(
        run(200, &link, &chain, &peer).await,
        Outcome::Refused(Refusal::NoHeaders)
    );
    assert_eq!(violations(&peer), 0);
}

#[tokio::test(start_paused = true)]
async fn the_deadline_is_checked_before_every_request() {
    let chain = peer_chain(180, 190);
    let claimed = claimed(&chain);
    let past = Deadline(Instant::now());
    // Past before the first probe: the reference throws.
    let link = Link::serving(&chain);
    assert_eq!(
        verifier(200).verify(&link, &a_peer(), &claimed, past).await,
        Outcome::Stopped(VerificationStopped::Deadline)
    );
    assert!(link.asked().is_empty());
    assert_eq!(
        verifier(200)
            .check_state(&link, &a_peer(), &claimed, past)
            .await,
        Err(VerificationStopped::Deadline)
    );
    // Each probe waits at most the time left.
    let link = Link::serving(&chain);
    let deadline = Deadline::after(Duration::from_secs(3));
    verifier(200)
        .verify(&link, &a_peer(), &claimed, deadline)
        .await;
    assert!(matches!(
        link.asked().first(),
        Some(Asked::Common(_, timeout)) if *timeout == Duration::from_secs(3)
    ));
    // Passed during the first probe: the second one is not sent.
    let mut link = Link::serving(&chain);
    link.stall = Some((0, Duration::from_secs(10)));
    let deadline = Deadline::after(Duration::from_secs(5));
    assert_eq!(
        verifier(200)
            .verify(&link, &a_peer(), &claimed, deadline)
            .await,
        Outcome::Stopped(VerificationStopped::Deadline)
    );
    assert_eq!(link.asked().len(), 1);
    // Passed during the last probe: the reference catches it before the fetch, and the chain
    // does not verify.
    let mut link = Link::serving(&chain);
    link.stall = Some((2, Duration::from_secs(10)));
    let deadline = Deadline::after(Duration::from_secs(5));
    assert_eq!(
        verifier(200)
            .check_state(&link, &a_peer(), &claimed, deadline)
            .await,
        Ok(None)
    );
    assert_eq!(link.asked().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn the_fast_verification_checks_the_header_only() {
    let peer = a_peer();
    let same = claimed(&peer_chain(200, 200));
    assert_eq!(
        verifier(200).check_state_fast(&peer, &same).await,
        Ok(Some(FastVerification {
            my_height: 200,
            their_height: 200,
            highest_common_height: Some(200),
        }))
    );
    let forked = verifier(200)
        .check_state_fast(&peer, &claimed(&peer_chain(180, 190)))
        .await
        .unwrap()
        .expect("the header verifies");
    assert_eq!(forked.highest_common_height, None);
    assert!(forked.forked());
    let bad = state_at(201, &C1.header(200));
    assert_eq!(verifier(200).check_state_fast(&peer, &bad).await, Ok(None));
}

/// The verifier through the communicator, over the connector, against a mock peer that serves a
/// fork of the committed chain.
#[cfg(feature = "difftest")]
mod through_the_communicator {
    use super::*;
    use heartwood_p2p::seeds::HttpProbe;
    use heartwood_p2p::testing::{MockPeer, ServedChain};
    use heartwood_p2p::utils::rules::Rules;
    use heartwood_p2p::view::{NoEvents, OnlineSink};
    use heartwood_p2p::{Communicator, CommunicatorParts, Connector, P2pOptions, PeerRepository};
    use heartwood_p2p_wire::status::{NetworkConfig, PeerConfig, TokenConfig};

    struct NoOnline;

    impl OnlineSink for NoOnline {
        fn set_version(&self, _key: PublicKeyBytes, _round: u64, _version: String) {}
    }

    /// A mock peer on `ip` that serves `chain`: our blocks up to 180, then its own.
    async fn serve(ip: [u8; 4], chain: &[BlockHeader]) -> MockPeer {
        let blocks = chain
            .iter()
            .map(|header| match header.height {
                h if h <= 180 => (**C1.block(h)).clone(),
                _ => factory::from_data(header.clone(), &[], &C1.milestones).expect("a block"),
            })
            .collect();
        let config = PeerConfig {
            version: "4.3.1".into(),
            network: NetworkConfig {
                version: C1.network.pub_key_hash,
                name: C1.network.name.clone(),
                nethash: C1.network.nethash.clone(),
                explorer: String::new(),
                token: TokenConfig {
                    name: "dROOT".into(),
                    symbol: "dRT".into(),
                },
            },
            plugins: Vec::new(),
        };
        let served = ServedChain {
            blocks,
            milestones: C1.milestones.clone(),
            config,
            signers: Vec::new(),
            peers: Vec::new(),
            empty_first_get_blocks: false,
        };
        MockPeer::serving(IpAddr::from(ip), served)
            .await
            .expect("the mock peer listens")
    }

    #[tokio::test]
    #[ignore = "integration"]
    async fn forks_served_by_mock_peers_verify_through_the_communicator() {
        let ours = Arc::new(Ours {
            height: 200,
            standby: standby().public_key().to_compressed(),
        });
        let clock = Arc::new(ManualClock::new(NOW_MS));
        let verifier = Arc::new(PeerVerifier::new(ours.clone(), ours.clone(), clock.clone()));
        // The node's version goes into every request's headers; a communicator needs one.
        let options = Arc::new(P2pOptions {
            version: "4.3.1-heartwood.0".into(),
            ..P2pOptions::default()
        });
        let communicator = Communicator::new(CommunicatorParts {
            transport: Arc::new(Connector::new()),
            verifier: verifier.clone(),
            rules: Arc::new(Rules::new(
                options.rate_limit,
                options.rate_limit_post_transactions,
            )),
            probe: Arc::new(HttpProbe {
                timeout: Duration::from_secs(1),
                ..HttpProbe::default()
            }),
            repository: Arc::new(PeerRepository::new()),
            chain: ours,
            online: Arc::new(NoOnline),
            events: Arc::new(NoEvents),
            options,
            clock,
            runtime: tokio::runtime::Handle::current(),
        })
        .unwrap();
        let honest = peer_chain(180, 190);
        let mut unlinked = honest.clone();
        unlinked[184] = signed_by(
            BlockHeader {
                previous_block: [7; 32],
                ..unlinked[184].clone()
            },
            C1.signer(185, 3),
        );
        for (ip, chain, expected) in [
            ([127, 0, 0, 41], honest, verified(200, 190, 180)),
            (
                [127, 0, 0, 42],
                unlinked,
                Outcome::Refused(Refusal::Unlinked),
            ),
        ] {
            let mock = serve(ip, &chain).await;
            let address = mock.address();
            let peer = SharedPeer::new(Peer::new(address.ip(), address.port()));
            let outcome = verifier
                .verify(&communicator, &peer, &claimed(&chain), far())
                .await;
            assert_eq!(outcome, expected);
            let unlinked = expected == Outcome::Refused(Refusal::Unlinked);
            assert_eq!(violations(&peer), usize::from(unlinked));
        }
    }
}
