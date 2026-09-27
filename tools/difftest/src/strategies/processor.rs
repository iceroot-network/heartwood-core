//! The cases of P13, the processor: on a prefix of a corpus chain, the next block built as the
//! synthetic cases build it (the slot's delegate, its reward, keys derived from the chain's public
//! seed) and then mutated (its timestamp, height, previous id, generator, reward, transactions,
//! version, payload length and signature, or a block of the chain itself), with a chain state
//! around it (the last downloaded block, the forked block, whether the blockchain has started,
//! blocks of the job not stored yet, a stored id, no delegate name when it was enqueued), for
//! `chain.process`: the result, the handler,
//! the unchained status, the chain state after it and the line after an accepted block must be
//! the reference's.
//!
//! The corpora are the committed one-node chain, and, when they are under the workspace's
//! `target/corpus`, `c2-faults` cut at 265 and `c4-restarts`. Each gives five prefixes: its tip,
//! the block before, the last round end below the tip and the block after it, and the middle of
//! the round before; each case carries its prefix whole (the chain form of `chain.process`), and
//! both sides keep at most sixteen chains applied.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use heartwood_crypto::blocks::{BlockHeader, serialiser};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::genesis::DerivedKey;
use heartwood_crypto::genesis::derive::role_key;
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::{TransferItem, Votes};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, Block, KeyPair, Milestones, PublicKeyBytes};
use heartwood_state::State;
use heartwood_transactions::Handlers;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Map, Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;
use crate::corpus::Corpus;
use crate::levels::l2::corpus_crypto;
use crate::synthetic::DERIVATION_TAG;

/// The aux of every signature of the cases.
const AUX: [u8; 32] = [0x13; 32];
/// A fee above every floor of the devnet.
const FEE: u64 = 20_000_000;
/// The corpora beside the committed one, under `target/corpus`, with their cuts.
const OTHER_CORPORA: [(&str, Option<u32>); 2] = [("c2-faults", Some(265)), ("c4-restarts", None)];

/// A prefix of a corpus chain, applied.
struct Prefix {
    chain: Value,
    state: State,
    milestones: Milestones,
    keys: Arc<Vec<DerivedKey>>,
    epoch_ms: i64,
    /// The chain's blocks up to the tip, the genesis block's successor first.
    blocks: Arc<Vec<Block>>,
    /// The corpus's block after the tip, when there is one.
    next: Option<Vec<u8>>,
}

impl Prefix {
    fn key_of(&self, public_key: &PublicKeyBytes) -> Option<&KeyPair> {
        self.keys
            .iter()
            .map(|k| &k.keys)
            .find(|k| &k.public_key().to_compressed() == public_key)
    }

    fn key(&self, role: &str) -> Option<&DerivedKey> {
        self.keys.iter().find(|k| k.role == role)
    }

    fn delegates(&self) -> impl Iterator<Item = &DerivedKey> {
        self.keys.iter().filter(|k| k.role.starts_with("delegate/"))
    }
}

static PREFIXES: LazyLock<Vec<Prefix>> = LazyLock::new(|| {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut corpora = vec![(root.join("corpus/txgen-one-node"), None)];
    for (name, cut) in OTHER_CORPORA {
        let dir = root.join("../../target/corpus").join(name);
        if dir.join("corpus.json").exists() {
            corpora.push((dir, cut));
        }
    }
    corpora
        .into_iter()
        .filter_map(|(dir, cut)| prefixes(&dir, cut))
        .flatten()
        .collect()
});

/// The five prefixes of the corpus in `dir`, cut at `cut`; `None` when it does not load.
fn prefixes(dir: &Path, cut: Option<u32>) -> Option<Vec<Prefix>> {
    let mut corpus = Corpus::load(dir).ok()?;
    let all: Vec<Vec<u8>> = corpus.blocks.iter().map(|b| b.bytes.clone()).collect();
    if let Some(to) = cut {
        corpus = corpus.cut(to).ok()?;
    }
    let crypto = corpus_crypto(&corpus).ok()?;
    let seed = corpus.meta.get("seed")?.as_str()?.to_owned();
    let network = corpus.network.pub_key_hash;
    let mut roles: Vec<String> = vec!["genesis-1".into(), "genesis-2".into()];
    roles.extend((1..=53).map(|k| format!("delegate/{k}")));
    roles.extend((1..=20).map(|k| format!("wallet/{k}")));
    let keys: Vec<DerivedKey> = roles
        .iter()
        .map(|role| role_key(DERIVATION_TAG, role, &seed, network))
        .collect::<Result<_, _>>()
        .ok()?;
    let keys = Arc::new(keys);
    let milestones = corpus.milestones.clone();
    let epoch_ms = slots::epoch_unix_ms(milestones.at(1).epoch())?;
    let tip = corpus.tip();
    let round = u32::try_from(milestones.at(1).active_delegates()).ok()?;
    let round_end = (tip - 1) / round * round;
    let mut heights = vec![
        tip,
        tip - 1,
        round_end,
        round_end + 1,
        round_end - round / 2,
    ];
    heights.retain(|&h| h >= 2 && h <= tip);
    heights.sort_unstable();
    heights.dedup();
    let mut state = State::from_genesis(&corpus.genesis, &milestones, &Handlers).ok()?;
    let mut blocks = Vec::new();
    let mut out = Vec::new();
    for block in &corpus.blocks {
        let decoded = Block::receive(&block.bytes, &milestones).ok()?;
        state.apply_block(&decoded, &milestones, &Handlers).ok()?;
        blocks.push(decoded);
        let height = block.height;
        if heights.contains(&height) {
            let hexes: Vec<Value> = all
                .iter()
                .take(usize::try_from(height - 1).ok()?)
                .map(|bytes| json!(hex::encode(bytes)))
                .collect();
            out.push(Prefix {
                chain: json!({"crypto": crypto.clone(), "blocks": hexes}),
                state: state.clone(),
                milestones: milestones.clone(),
                keys: Arc::clone(&keys),
                epoch_ms,
                blocks: Arc::new(blocks.clone()),
                next: all.get(usize::try_from(height - 1).ok()?).cloned(),
            });
        }
    }
    Some(out)
}

/// A transaction of a case, before the mutation.
#[derive(Debug, Clone, Copy)]
enum Tx {
    /// A transfer from a genesis wallet.
    Transfer(bool),
    /// A vote from a genesis wallet for a delegate.
    Vote(bool, u8),
    /// A transfer that skips a nonce.
    NonceGap,
    /// A second vote from the sender of the vote before it.
    SecondVote,
    /// A transfer of the sender's whole balance, which its fee takes over.
    Overspend,
    /// The first transaction of the latest block of the chain that has one.
    Stored,
    /// A transfer whose signature has one bit changed.
    BadSignature,
}

fn tx() -> impl Strategy<Value = Tx> {
    prop_oneof![
        4 => any::<bool>().prop_map(Tx::Transfer),
        3 => (any::<bool>(), 1u8..=53).prop_map(|(from, to)| Tx::Vote(from, to)),
        1 => Just(Tx::NonceGap),
        1 => Just(Tx::SecondVote),
        1 => Just(Tx::Overspend),
        1 => Just(Tx::Stored),
        1 => Just(Tx::BadSignature),
    ]
}

/// What the case changes of the next block.
#[derive(Debug, Clone, Copy)]
enum Mutation {
    None,
    SameSlot,
    EarlierThanTip(u32),
    Future(u32),
    AtTip,
    AtTipEarlier,
    AtTipOtherDelegate,
    HeightPlusTwo,
    HeightBelow(u32),
    ReplayTip,
    ReplayBelow(u32),
    OtherParent,
    OtherParentOtherDelegate,
    OtherDelegate(u8),
    NotDelegate(u8),
    Unranked,
    SecondaryReward,
    RewardPlusOne,
    NoReward,
    Version,
    PayloadLength,
    BadSignature,
    CorpusNext,
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        12 => Just(Mutation::None),
        3 => Just(Mutation::CorpusNext),
        2 => Just(Mutation::SameSlot),
        2 => (1u32..=3).prop_map(Mutation::EarlierThanTip),
        2 => (9u32..=40).prop_map(Mutation::Future),
        3 => Just(Mutation::AtTip),
        2 => Just(Mutation::AtTipEarlier),
        2 => Just(Mutation::AtTipOtherDelegate),
        2 => Just(Mutation::HeightPlusTwo),
        2 => (1u32..=5).prop_map(Mutation::HeightBelow),
        3 => Just(Mutation::ReplayTip),
        2 => (1u32..=5).prop_map(Mutation::ReplayBelow),
        2 => Just(Mutation::OtherParent),
        2 => Just(Mutation::OtherParentOtherDelegate),
        2 => any::<u8>().prop_map(Mutation::OtherDelegate),
        1 => (1u8..=20).prop_map(Mutation::NotDelegate),
        1 => Just(Mutation::Unranked),
        1 => Just(Mutation::SecondaryReward),
        1 => Just(Mutation::RewardPlusOne),
        1 => Just(Mutation::NoReward),
        1 => Just(Mutation::Version),
        1 => Just(Mutation::PayloadLength),
        1 => Just(Mutation::BadSignature),
    ]
}

/// The last downloaded block a case sets.
#[derive(Debug, Clone, Copy)]
enum Downloaded {
    Tip,
    None,
    Below,
    Ahead,
}

/// The forked block a case sets.
#[derive(Debug, Clone, Copy)]
enum Forked {
    None,
    ThisBlock,
    TheTip,
}

/// Everything a case is made of.
#[derive(Debug, Clone)]
struct Recipe {
    prefix: usize,
    gap: u32,
    transactions: Vec<Tx>,
    mutation: Mutation,
    downloaded: Downloaded,
    forked: Forked,
    started: bool,
    unstored: u32,
    forged: bool,
    unnamed: bool,
    late_ms: i64,
}

fn recipe(prefixes: usize) -> impl Strategy<Value = Recipe> {
    let chain_state = (
        prop_oneof![6 => Just(Downloaded::Tip), 1 => Just(Downloaded::None), 1 => Just(Downloaded::Below), 1 => Just(Downloaded::Ahead)],
        prop_oneof![6 => Just(Forked::None), 2 => Just(Forked::ThisBlock), 1 => Just(Forked::TheTip)],
        prop_oneof![4 => Just(true), 1 => Just(false)],
        prop_oneof![6 => Just(0u32), 1 => 1u32..=2],
        prop_oneof![8 => Just(false), 1 => Just(true)],
        prop_oneof![12 => Just(false), 1 => Just(true)],
    );
    (
        0..prefixes.max(1),
        prop_oneof![6 => Just(0u32), 2 => 1u32..=3],
        prop_oneof![3 => Just(Vec::new()), 2 => prop::collection::vec(tx(), 1..=3)],
        mutation(),
        chain_state,
        0i64..8_000,
    )
        .prop_map(
            |(
                prefix,
                gap,
                transactions,
                mutation,
                (downloaded, forked, started, unstored, forged, unnamed),
                late_ms,
            )| Recipe {
                prefix,
                gap,
                transactions,
                mutation,
                downloaded,
                forked,
                started,
                unstored,
                forged,
                unnamed,
                late_ms,
            },
        )
}

/// The cases of P13.
pub fn processor_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    recipe(PREFIXES.len())
        .prop_filter_map("the corpora have no prefix", |recipe| {
            PREFIXES
                .get(recipe.prefix)
                .map(|prefix| case(prefix, &recipe))
        })
        .boxed()
}

/// A block to assemble: the header's fields, the signer, and the transactions' bytes.
struct Draft<'k> {
    header: BlockHeader,
    signer: &'k KeyPair,
    transactions: Vec<(Vec<u8>, u64, u64)>,
    bad_signature: bool,
}

impl Draft<'_> {
    /// The block's bytes: the header signed (then broken when asked), the transaction lengths,
    /// the transactions. The totals and the payload hash are the transactions'.
    fn assemble(mut self) -> Vec<u8> {
        let ids: Vec<u8> = self
            .transactions
            .iter()
            .flat_map(|(bytes, _, _)| sha256(bytes).to_vec())
            .collect();
        self.header.number_of_transactions = u32::try_from(self.transactions.len()).unwrap_or(0);
        self.header.total_amount = self.transactions.iter().map(|t| t.1).sum();
        self.header.total_fee = self.transactions.iter().map(|t| t.2).sum();
        self.header.payload_hash = sha256(&ids);
        self.header.generator_public_key = self.signer.public_key().to_compressed();
        self.header.block_signature = sig::sign(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Block,
            &serialiser::signing_hash(&self.header),
            self.signer.secret_key(),
            Aux::fixed(AUX),
        )
        .unwrap_or(Signature::from_array([0; 64]));
        if self.bad_signature {
            let mut bytes = *self.header.block_signature.as_bytes();
            bytes[0] ^= 1;
            self.header.block_signature = Signature::from_array(bytes);
        }
        let mut out = serialiser::serialise_header(&self.header, true);
        for (bytes, _, _) in &self.transactions {
            let length = u32::try_from(bytes.len()).unwrap_or(0);
            out.extend_from_slice(&length.to_le_bytes());
        }
        for (bytes, _, _) in self.transactions {
            out.extend(bytes);
        }
        out
    }
}

/// The transactions of `kinds` on the prefix's tip, as bytes with their amounts and fees.
fn transactions(p: &Prefix, kinds: &[Tx]) -> Vec<(Vec<u8>, u64, u64)> {
    let params = p.milestones.at(p.state.height().saturating_add(1));
    let senders = [p.key("genesis-1"), p.key("genesis-2")];
    let wallet = |k: &DerivedKey| p.state.wallets().find_by_address(&k.address);
    let recipient = p.key("wallet/20").map(|k| k.address);
    let transfer = |amount: u64| {
        recipient
            .map(|recipient| TransactionBuilder::transfer(vec![TransferItem { amount, recipient }]))
    };
    let vote = |name: String| {
        let votes: Votes = [(name, 10_000)].into_iter().collect();
        Some(TransactionBuilder::vote(votes))
    };
    let mut used = [0u64; 2];
    let mut last_voter = 0usize;
    let mut out = Vec::new();
    for kind in kinds {
        let (s, builder, skip) = match *kind {
            Tx::Transfer(second) => (usize::from(second), transfer(1_000), 0),
            Tx::BadSignature => (0, transfer(1_000), 0),
            Tx::NonceGap => (0, transfer(1_000), 1),
            Tx::Overspend => {
                let balance = senders[0]
                    .and_then(wallet)
                    .map_or(1, heartwood_state::Wallet::balance);
                (0, transfer(balance), 0)
            }
            Tx::Vote(second, to) => {
                last_voter = usize::from(second);
                (last_voter, vote(format!("genesis_{to}")), 0)
            }
            Tx::SecondVote => (last_voter, vote("genesis_7".to_owned()), 0),
            Tx::Stored => {
                let stored = p.blocks.iter().rev().find_map(|b| b.transactions().first());
                if let Some(t) = stored {
                    out.push((t.bytes().to_vec(), t.amount(), t.fee()));
                }
                continue;
            }
        };
        let (Some(sender), Some(builder)) = (senders[s], builder) else {
            continue;
        };
        used[s] += 1 + skip;
        let nonce = wallet(sender).map_or(0, heartwood_state::Wallet::nonce) + used[s];
        let built = builder
            .network(params.network())
            .nonce(nonce)
            .fee(FEE)
            .build(&sender.keys, Aux::fixed(AUX), params);
        if let Ok(t) = built {
            let mut bytes = t.bytes().to_vec();
            if matches!(kind, Tx::BadSignature)
                && let Some(byte) = bytes.last_mut()
            {
                *byte ^= 1;
            }
            out.push((bytes, t.amount(), t.fee()));
        }
    }
    out
}

/// The case of `recipe` on the prefix `p`.
fn case<'p>(p: &'p Prefix, recipe: &Recipe) -> OpCase {
    let ms = &p.milestones;
    let tip = p.state.last_block();
    let last = tip.header();
    let block_time = u32::try_from(ms.at(1).block_time()).unwrap_or(8);
    let n = u32::try_from(ms.at(1).active_delegates())
        .unwrap_or(53)
        .max(1);
    let last_slot = last.timestamp / block_time;
    // The first slot from the gap on whose delegate the seed derives.
    let found = (last_slot + 1 + recipe.gap..last_slot + 1 + recipe.gap + 200).find_map(|slot| {
        let forger = p
            .state
            .forging_delegates()
            .get(usize::try_from(slot % n).ok()?)?;
        p.key_of(&forger.public_key)
            .map(|k| (slot, forger.public_key, k))
    });
    let Some((slot, scheduled, signer)) = found else {
        return plain_case(p, recipe, tip.encode());
    };
    let height = last.height + 1;
    let reward = p
        .state
        .reward_for_block_in_round(height, &scheduled, ms)
        .map_or(0, |r| r.reward);
    let mut draft = Draft {
        header: BlockHeader {
            version: ms.at(height).block_version(),
            timestamp: slot * block_time,
            height,
            previous_block: *tip.id().as_bytes(),
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward,
            payload_length: 0,
            payload_hash: [0; 32],
            generator_public_key: scheduled,
            block_signature: Signature::from_array([0; 64]),
        },
        signer,
        transactions: transactions(p, &recipe.transactions),
        bad_signature: false,
    };
    let other = |pick: u8| {
        p.delegates()
            .map(|k| &k.keys)
            .filter(|k| k.public_key().to_compressed() != scheduled)
            .nth(usize::from(pick) % 52)
    };
    let at_tip = |draft: &mut Draft<'p>| {
        draft.header.height = last.height;
        draft.header.previous_block = last.previous_block;
        draft.header.timestamp = last.timestamp + 1;
        draft.header.reward = last.reward;
        if let Some(k) = p.key_of(&last.generator_public_key) {
            draft.signer = k;
        }
    };
    let mut now_shift = 0i64;
    match recipe.mutation {
        Mutation::None => {}
        Mutation::CorpusNext => {
            if let Some(next) = &p.next {
                return plain_case(p, recipe, next.clone());
            }
        }
        Mutation::SameSlot => draft.header.timestamp = last.timestamp + 1,
        Mutation::EarlierThanTip(k) => {
            draft.header.timestamp = last.timestamp.saturating_sub(k * block_time);
        }
        Mutation::Future(seconds) => now_shift = -i64::from(seconds) * 1_000,
        Mutation::AtTip => at_tip(&mut draft),
        Mutation::AtTipEarlier => {
            at_tip(&mut draft);
            draft.header.timestamp = last.timestamp.saturating_sub(block_time);
        }
        Mutation::AtTipOtherDelegate => {
            at_tip(&mut draft);
            if let Some(k) = other(7) {
                draft.signer = k;
            }
        }
        Mutation::HeightPlusTwo => draft.header.height = last.height + 2,
        Mutation::HeightBelow(k) => draft.header.height = last.height.saturating_sub(k).max(2),
        Mutation::ReplayTip => return plain_case(p, recipe, tip.encode()),
        Mutation::ReplayBelow(k) => {
            let below = p.blocks.len().saturating_sub(1 + k as usize);
            if let Some(block) = p.blocks.get(below) {
                return plain_case(p, recipe, block.encode());
            }
        }
        Mutation::OtherParent => draft.header.previous_block = sha256(tip.id().as_bytes()),
        Mutation::OtherParentOtherDelegate => {
            draft.header.previous_block = sha256(tip.id().as_bytes());
            if let Some(k) = other(3) {
                draft.signer = k;
            }
        }
        Mutation::OtherDelegate(pick) => {
            if let Some(k) = other(pick) {
                draft.signer = k;
            }
        }
        Mutation::NotDelegate(k) => {
            if let Some(k) = p.key(&format!("wallet/{k}")) {
                draft.signer = &k.keys;
            }
        }
        Mutation::Unranked => {
            let unranked = p.state.wallets().delegates().find_map(|w| {
                let delegate = w.delegate()?;
                let key = w.public_key()?.to_compressed();
                if delegate.rank.is_none() {
                    p.key_of(&key)
                } else {
                    None
                }
            });
            if let Some(k) = unranked {
                draft.signer = k;
            }
        }
        Mutation::SecondaryReward => {
            if let Some(table) = ms.at(height).dynamic_reward() {
                draft.header.reward = table.secondary_reward();
            }
        }
        Mutation::RewardPlusOne => draft.header.reward = draft.header.reward.saturating_add(1),
        Mutation::NoReward => draft.header.reward = 0,
        Mutation::Version => draft.header.version = 1,
        Mutation::PayloadLength => {
            draft.header.payload_length = 32;
        }
        Mutation::BadSignature => draft.bad_signature = true,
    }
    let count = u32::try_from(draft.transactions.len()).unwrap_or(0);
    if !matches!(recipe.mutation, Mutation::PayloadLength) {
        draft.header.payload_length = count * 32;
    } else {
        draft.header.payload_length = count * 32 + 32;
    }
    let timestamp = draft.header.timestamp;
    let bytes = draft.assemble();
    let now = p.epoch_ms + i64::from(timestamp) * 1_000 + recipe.late_ms + now_shift;
    finish(p, recipe, bytes, now)
}

/// A case of `bytes` that are not built here (a block of the chain), at its slot's time.
fn plain_case(p: &Prefix, recipe: &Recipe, bytes: Vec<u8>) -> OpCase {
    let timestamp = bytes
        .get(4..8)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map_or(0, u32::from_le_bytes);
    let now = p.epoch_ms + i64::from(timestamp) * 1_000 + recipe.late_ms;
    finish(p, recipe, bytes, now)
}

/// The operation's input: the prefix's chain, the block, the time and the chain state.
fn finish(p: &Prefix, recipe: &Recipe, bytes: Vec<u8>, now: i64) -> OpCase {
    let tip = p.state.last_block();
    let height = bytes
        .get(8..12)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map_or(0, u32::from_le_bytes);
    let mut state = Map::new();
    let link = |block: &Block| {
        let header = block.header();
        json!({
            "height": header.height,
            "id": block.id().to_hex(),
            "previousBlock": hex::encode(&header.previous_block),
            "timestamp": header.timestamp,
        })
    };
    match recipe.downloaded {
        Downloaded::Tip => {}
        Downloaded::None => {
            state.insert("lastDownloaded".into(), Value::Null);
        }
        Downloaded::Below => {
            if let Some(below) = p.blocks.iter().rev().nth(3) {
                state.insert("lastDownloaded".into(), link(below));
            }
        }
        Downloaded::Ahead => {
            let mut ahead = link(tip);
            ahead["height"] = json!(tip.height() + 10);
            state.insert("lastDownloaded".into(), ahead);
        }
    }
    match recipe.forked {
        Forked::None => {}
        Forked::ThisBlock => {
            state.insert("forked".into(), json!(hex::encode(&bytes)));
        }
        Forked::TheTip => {
            state.insert("forked".into(), json!(hex::encode(&tip.encode())));
        }
    }
    if !recipe.started {
        state.insert("started".into(), json!(false));
    }
    let unstored = recipe
        .unstored
        .min(u32::try_from(p.blocks.len()).unwrap_or(0));
    if unstored > 0 {
        state.insert("unstored".into(), json!(unstored));
    }
    if recipe.forged {
        let first = Block::receive(&bytes, &p.milestones)
            .ok()
            .and_then(|block| block.transaction_ids().next().map(|id| id.to_hex()));
        if let Some(id) = first {
            state.insert("forged".into(), json!([id]));
        }
    }
    if recipe.unnamed {
        state.insert("unnamed".into(), json!(true));
    }
    let mut input = json!({"chain": p.chain.clone(), "hex": hex::encode(&bytes), "now": now});
    if !state.is_empty() {
        input["state"] = Value::Object(state);
    }
    ("chain.process", u64::from(height), input)
}
