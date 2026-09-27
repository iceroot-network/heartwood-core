//! The cases of P14, the verifier: `verifier.checkState` on a chain of 70 headers signed by the
//! delegates of their rounds (rounds of 53, from a rotating list of 56 registered delegates), with
//! a random height of ours, a peer on our chain or forked from a random height with a branch of its
//! own, and optionally a header of the branch that fails a check or does not chain, scripted
//! common-block replies (bogus ones among them), short replies, a deadline and a claimed header
//! that fails. The result and every request must be the reference's; the unlinked segments are
//! the documented difference.

use std::sync::{Arc, LazyLock};

use heartwood_crypto::blocks::serialiser::{block_id, serialise_header, signing_hash};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockHeader, KeyPair, PublicKeyBytes};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::keys;
use super::transactions::TxContext;

/// The delegates of a round.
const ROUND: usize = 53;
/// The registered delegates: the rounds rotate through the first 55, the last is in none.
const REGISTERED: usize = 56;
/// Our chain's length.
const TOP: u32 = 70;
/// The aux of every signature of the cases.
const AUX: [u8; 32] = [0x42; 32];

/// The keys and the chain the cases share.
struct Chains {
    delegates: Vec<KeyPair>,
    stranger: KeyPair,
    base: Vec<BlockHeader>,
}

static CHAINS: LazyLock<Chains> = LazyLock::new(|| {
    let delegates = (0..REGISTERED)
        .map(|i| keys(&format!("verifier delegate {i}")))
        .collect();
    let mut chains = Chains {
        delegates,
        stranger: keys("verifier stranger"),
        base: Vec::new(),
    };
    chains.base = chains.extend([0; 32], 1, TOP, 0, 0);
    chains
});

impl Chains {
    /// The delegates of the round of `height`: 53 consecutive ones from the round's number - 1.
    fn round(&self, height: u32) -> &[KeyPair] {
        let round = (height.saturating_sub(1) as usize) / ROUND;
        &self.delegates[round..round + ROUND]
    }

    fn sign(&self, mut header: BlockHeader, keys: &KeyPair) -> BlockHeader {
        header.generator_public_key = keys.public_key().to_compressed();
        header.block_signature = sig::sign(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Block,
            &signing_hash(&header),
            keys.secret_key(),
            Aux::fixed(AUX),
        )
        .unwrap_or(Signature::from_array([0; 64]));
        header
    }

    /// Headers `from` to `to` on top of `previous`, each signed by a delegate of its round.
    fn extend(
        &self,
        previous: [u8; 32],
        from: u32,
        to: u32,
        offset: u32,
        shift: usize,
    ) -> Vec<BlockHeader> {
        let mut out: Vec<BlockHeader> = Vec::new();
        let mut last = previous;
        for height in from..=to {
            let round = self.round(height);
            let signer = &round[(height as usize + shift) % round.len()];
            let draft = BlockHeader {
                version: 0,
                timestamp: (height - 1) * 8 + offset,
                height,
                previous_block: last,
                number_of_transactions: 0,
                total_amount: 0,
                total_fee: 0,
                reward: 180_000_000,
                payload_length: 0,
                payload_hash: sha256(&[]),
                generator_public_key: PublicKeyBytes::from_array([0; 33]),
                block_signature: Signature::from_array([0; 64]),
            };
            let header = self.sign(draft, signer);
            last = *block_id(&header).as_bytes();
            out.push(header);
        }
        out
    }

    fn id(&self, height: u32) -> String {
        block_id(&self.base[height as usize - 1]).to_hex()
    }
}

fn hex_of(header: &BlockHeader) -> Value {
    json!(hex::encode(&serialise_header(header, true)))
}

/// A change to one header of the peer's branch.
#[derive(Debug, Clone, Copy)]
enum Change {
    Signature,
    Height,
    OtherRound,
    Standby,
    Stranger,
    Version,
    ShortHash,
    LongHash,
    OddHash,
    NoHash,
    Unlinked,
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        1 => Just(Change::Signature),
        1 => Just(Change::Height),
        1 => Just(Change::OtherRound),
        1 => Just(Change::Standby),
        1 => Just(Change::Stranger),
        1 => Just(Change::Version),
        1 => Just(Change::ShortHash),
        1 => Just(Change::LongHash),
        1 => Just(Change::OddHash),
        1 => Just(Change::NoHash),
        3 => Just(Change::Unlinked),
    ]
}

/// The header `header` changed by `change`, as the input writes it.
fn changed(c: &Chains, header: &BlockHeader, change: Change) -> Value {
    let hash = hex::encode(&header.payload_hash);
    let set = |set: Value| json!({"hex": hex_of(header), "set": set});
    let resign = |edit: &dyn Fn(&mut BlockHeader), keys: Option<&KeyPair>| {
        let mut edited = header.clone();
        edit(&mut edited);
        let keys = keys.unwrap_or_else(|| {
            c.delegates
                .iter()
                .find(|k| k.public_key().to_compressed() == header.generator_public_key)
                .unwrap_or(&c.stranger)
        });
        hex_of(&c.sign(edited, keys))
    };
    // A registered delegate outside the header's round: the next round's newest, or the first.
    let outsider = if header.height as usize <= ROUND {
        &c.delegates[ROUND]
    } else {
        &c.delegates[0]
    };
    match change {
        Change::Signature => set(json!({"timestamp": header.timestamp ^ 1})),
        Change::Height => resign(&|h| h.height += 1, None),
        Change::OtherRound => resign(&|_| {}, Some(outsider)),
        Change::Standby => resign(&|_| {}, Some(&c.delegates[REGISTERED - 1])),
        Change::Stranger => resign(&|_| {}, Some(&c.stranger)),
        Change::Version => resign(&|h| h.version = 1, None),
        Change::ShortHash => set(json!({"payloadHash": &hash[..62]})),
        Change::LongHash => set(json!({"payloadHash": format!("{hash}77")})),
        Change::OddHash => set(json!({"payloadHash": format!("{hash}7")})),
        Change::NoHash => set(json!({"payloadHash": null})),
        Change::Unlinked => resign(&|h| h.previous_block = [7; 32], None),
    }
}

/// A scripted getCommonBlocks answer: honest, none, or about our block at a height.
#[derive(Debug, Clone, Copy)]
enum Reply {
    Honest,
    Nothing,
    Ours(u32),
    OtherHeight(u32),
    Uppercase(u32),
    Foreign(u32),
}

fn reply() -> impl Strategy<Value = Reply> {
    prop_oneof![
        3 => Just(Reply::Honest),
        1 => Just(Reply::Nothing),
        1 => (1..=TOP).prop_map(Reply::Ours),
        1 => (1..=TOP).prop_map(Reply::OtherHeight),
        1 => (1..=TOP).prop_map(Reply::Uppercase),
        1 => (1..=TOP).prop_map(Reply::Foreign),
    ]
}

fn reply_json(c: &Chains, reply: Reply, ours: u32) -> Value {
    let at = |h: u32| h.clamp(1, ours);
    match reply {
        Reply::Honest => json!("honest"),
        Reply::Nothing => Value::Null,
        Reply::Ours(h) => json!({"height": at(h), "id": c.id(at(h))}),
        Reply::OtherHeight(h) => json!({"height": at(h) + 1, "id": c.id(at(h))}),
        Reply::Uppercase(h) => json!({"height": at(h), "id": c.id(at(h)).to_uppercase()}),
        Reply::Foreign(h) => json!({"height": at(h), "id": "ab".repeat(32)}),
    }
}

/// What the peer claims besides its last block.
#[derive(Debug, Clone, Copy)]
enum Claim {
    Tip,
    OtherHeight,
    Signature,
    Stranger,
}

/// One case, as drawn.
#[derive(Debug, Clone)]
struct Draw {
    ours: u32,
    fork: u32,
    top: u32,
    offset: u32,
    shift: usize,
    change: Option<(usize, Change)>,
    replies: Vec<Reply>,
    per_reply: Option<u32>,
    deadline: Option<u32>,
    fast: bool,
    claim: Claim,
}

fn draw() -> impl Strategy<Value = Draw> {
    let chains = (1..=TOP).prop_flat_map(|ours| {
        prop_oneof![
            // On our chain, at or below our height or ahead of it.
            1 => (Just(ours), 1..=TOP, Just(0u32)),
            // Forked from a height of ours, with up to 20 blocks of its own.
            3 => (Just(ours), 1..=ours, 1..=20u32),
        ]
    });
    let extras = (
        prop::option::weighted(0.4, (0..20usize, change())),
        prop::collection::vec(reply(), 0..3),
        prop::option::weighted(0.25, 0..=4u32),
        prop::option::weighted(0.2, 0..=6u32),
        prop::bool::weighted(0.1),
        prop_oneof![
            8 => Just(Claim::Tip),
            1 => Just(Claim::OtherHeight),
            1 => Just(Claim::Signature),
            1 => Just(Claim::Stranger),
        ],
    );
    (chains, 1..8u32, 0..ROUND, extras).prop_map(
        |(
            (ours, at, length),
            offset,
            shift,
            (change, replies, per_reply, deadline, fast, claim),
        )| {
            let (fork, top) = if length == 0 {
                (at, at)
            } else {
                (at, at + length)
            };
            Draw {
                ours,
                fork,
                top,
                offset,
                shift,
                change,
                replies,
                per_reply,
                deadline,
                fast,
                claim,
            }
        },
    )
}

/// The input of a drawn case, in the operation's shape.
fn input(d: &Draw) -> Value {
    let c = &*CHAINS;
    let own = d.top > d.fork;
    let shared = d.fork.min(d.ours);
    // The peer's blocks after our chain's part: ours above our height, then its own.
    let mut branch: Vec<BlockHeader> = c.base[shared as usize..d.fork as usize].to_vec();
    if own {
        let previous = *block_id(&c.base[d.fork as usize - 1]).as_bytes();
        branch.extend(c.extend(previous, d.fork + 1, d.top, d.offset, d.shift));
    }
    let mut written: Vec<Value> = branch.iter().map(hex_of).collect();
    if let Some((index, change)) = d.change
        && !branch.is_empty()
    {
        let index = index % branch.len();
        written[index] = changed(c, &branch[index], change);
    }
    let tip = branch.last().unwrap_or(&c.base[d.fork as usize - 1]);
    let claimed = match d.claim {
        Claim::Tip => json!({"height": tip.height, "header": hex_of(tip)}),
        Claim::OtherHeight => json!({"height": tip.height + 1, "header": hex_of(tip)}),
        Claim::Signature => json!({"height": tip.height, "header": {
            "hex": hex_of(tip), "set": {"timestamp": tip.timestamp ^ 1}}}),
        Claim::Stranger => {
            json!({"height": tip.height, "header": hex_of(&c.sign(tip.clone(), &c.stranger))})
        }
    };
    let last_round = (d.ours as usize) / ROUND + 1;
    let rounds: Vec<Value> = (1..=last_round)
        .map(|round| {
            let keys: Vec<String> = c.delegates[round - 1..round - 1 + ROUND]
                .iter()
                .map(|k| k.public_key().to_compressed().to_hex())
                .collect();
            json!([round, keys])
        })
        .collect();
    let delegates: Vec<Value> = c
        .delegates
        .iter()
        .enumerate()
        .map(|(i, k)| {
            json!([
                k.public_key().to_compressed().to_hex(),
                format!("delegate_{i}")
            ])
        })
        .collect();
    let mut peer = json!({"fork": shared, "branch": written});
    if let Some(per_reply) = d.per_reply {
        peer["perReply"] = json!(per_reply);
    }
    if !d.replies.is_empty() {
        peer["common"] = d
            .replies
            .iter()
            .map(|&r| reply_json(c, r, d.ours))
            .collect();
    }
    let mut out = json!({
        "chain": c.base[..d.ours as usize].iter().map(hex_of).collect::<Vec<_>>(),
        "delegates": delegates,
        "rounds": rounds,
        "peer": peer,
        "claimed": claimed,
        "fast": d.fast,
    });
    if let Some(deadline) = d.deadline {
        out["deadline"] = json!(deadline);
    }
    out
}

/// The cases of P14.
pub fn verifier_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    draw()
        .prop_map(|d| ("verifier.checkState", 2, input(&d)))
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    #[test]
    fn the_base_chain_chains_and_is_signed_by_its_rounds() {
        let c = &*CHAINS;
        assert_eq!(c.base.len(), TOP as usize);
        for pair in c.base.windows(2) {
            assert_eq!(pair[1].previous_block, *block_id(&pair[0]).as_bytes());
        }
        let first = c.round(1)[0].public_key().to_compressed();
        assert_eq!(c.round(53)[0].public_key().to_compressed(), first);
        assert_ne!(c.round(54)[0].public_key().to_compressed(), first);
    }

    #[test]
    fn the_cases_are_well_formed() {
        let mut runner = TestRunner::deterministic();
        let strategy = draw();
        for _ in 0..64 {
            let d = strategy.new_tree(&mut runner).expect("a case").current();
            let input = input(&d);
            let chain = input["chain"].as_array().expect("a chain");
            assert_eq!(chain.len(), d.ours as usize);
            let fork = input["peer"]["fork"].as_u64().expect("a fork");
            assert!(fork <= u64::from(d.ours));
            assert!(input["claimed"]["height"].as_u64().expect("a height") >= 1);
        }
    }
}
