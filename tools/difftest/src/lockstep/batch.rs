//! The lockstep batch (`difftest lockstep batch`): many lockstep chains, each with a recipe drawn
//! from the batch's seed, made at once and written into one corpus directory beside a batch file
//! that names them.
//!
//! A drawn recipe uses every kept type, missed-slot patterns, revert plans across round ends,
//! restart points, a milestone that activates inside the chain and the online steps; a long
//! chain's recipe also stops attesting one delegate after round 1, so that its online entry has
//! expired at round 7's start and a vote for it in round 7 is refused `ERR_OFFLINE` by both pools.
//! Each chain is made by [`super::make`] on the lockstep [`Recipe`], so it is an ordinary corpus
//! that `corpus judge`, `l2`, `l2-processor`, `l2-restart` and `forge-replay` read as they are.
//!
//! The batch file `<out>/batch-<seed>.json` lists the chains (`batch-<seed>-<index>`, so that
//! every chain's name is its own in the whole corpus directory), their seeds, rounds and recipes,
//! and what each made; `difftest run l2-batch` reads it.
//!
//! The draw is a pure function of the batch's seed, the chain's index, its rounds and whether it
//! is long, so a batch can be made again chain by chain. Every chain carries, whatever its draw:
//! the funding of its wallets, the six kept transaction types, the removed type's expected
//! refusal, a vote for a delegate that is not attested (refused `ERR_OFFLINE`), an online step
//! for every round, a missed-slot pattern, a restart point at a round start and at a round end,
//! and, from two rounds on, a revert across a round end. Whether it also forks, whether a
//! delegate it registers wins votes and forges, whether a milestone activates inside it and
//! whether it resigns and revokes are drawn.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Attested, BuildSpec, Step};
use super::{LONG_ROUNDS, LockstepError, MadeChain, Recipe, SHORT_ROUNDS};
use crate::cli::LockstepBatchArgs;
use crate::errors::{Error, Result};
use crate::store::ScratchDir;

/// The batch file's format.
pub const BATCH_FORMAT: &str = "heartwood-lockstep-batch/1";

/// The blocks of a round on the devnet profile, which the draw plans for.
const ROUND: u32 = 53;

/// The round at whose start a long chain's unrefreshed entry has expired: an entry of round 1 is
/// forgotten once a round start passes 5 rounds beyond it, which is round 7's.
pub const EXPIRY_ROUND: u32 = 7;

/// The blocks of one download of a syncing node.
pub const DOWNLOAD_BLOCKS: u32 = 400;

/// The blocks a delegate the recipe registers misses its slots after its registration: more than
/// the 100 blocks of a job of the reference's download cut.
pub const SILENT_BLOCKS: u32 = 115;

/// The delegates no recipe attests: a vote for one is refused `ERR_OFFLINE` by both pools.
const UNATTESTED: [&str; 3] = ["genesis_51", "genesis_52", "genesis_53"];

/// The delegate a long chain attests in round 1 only.
const EXPIRING: &str = "genesis_50";

/// The delegates the draw votes for: always attested.
const VOTED: u32 = 49;

/// A removed type's hash, which the IPFS builder accepts.
const IPFS_HASH: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";

/// The funds of each wallet at height 2.
const FUNDING: &str = "1000000000000";

/// One chain of a batch, as drawn from the batch's seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawnChain {
    /// The chain's place in the batch, from 1.
    pub index: u32,
    /// The chain's corpus name, `batch-<seed>-<index>`.
    pub name: String,
    /// The chain's own seed, drawn from the batch's.
    pub seed: String,
    /// The chain's rounds.
    pub rounds: u32,
    /// Whether the chain is a long one (more than 5 rounds, the online entry's expiry shown).
    pub long: bool,
    /// The drawn recipe.
    pub recipe: Recipe,
}

/// What one chain of a batch made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchChain {
    /// The chain as drawn.
    pub drawn: DrawnChain,
    /// The last height, when the chain was made.
    pub tip: Option<u32>,
    /// The transactions the chain holds.
    pub transactions: u64,
    /// The slots missed.
    pub missed: u64,
    /// Why the producer stopped, when it did.
    pub stopped: Option<String>,
}

/// A batch: its file's contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Batch {
    /// [`BATCH_FORMAT`].
    pub format: String,
    /// The batch's seed.
    pub seed: String,
    /// The genesis profile of every chain.
    pub profile: String,
    /// Every chain, in index order.
    pub chains: Vec<BatchChain>,
}

impl BatchChain {
    /// The chain as the producer made it, `out` being the corpus directory.
    pub fn made(&self, out: &Path) -> Option<MadeChain> {
        Some(MadeChain {
            dir: out.join(&self.drawn.name),
            tip: self.tip?,
            transactions: self.transactions,
            missed: self.missed,
        })
    }
}

/// The batch file of the batch `seed` in the corpus directory `out`.
pub fn batch_file(out: &Path, seed: &str) -> PathBuf {
    out.join(format!("batch-{seed}.json"))
}

// ---- the draw -----------------------------------------------------------------------------------

/// A SplitMix64 stream: the draw's only source of choices.
struct Rng(u64);

impl Rng {
    /// The stream of `parts`, hashed together.
    fn of(parts: &[&str]) -> Rng {
        let mut hash = Sha256::new();
        hash.update(BATCH_FORMAT.as_bytes());
        for part in parts {
            hash.update([0]);
            hash.update(part.as_bytes());
        }
        let digest = hash.finalize();
        let mut first = [0u8; 8];
        first.copy_from_slice(&digest[..8]);
        Rng(u64::from_le_bytes(first))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n` (0 when `n` is 0).
    fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        u32::try_from(self.next() % u64::from(n)).unwrap_or(0)
    }

    /// A number from `low` to `high`, both included (`low` when `high` is below it).
    fn between(&mut self, low: u32, high: u32) -> u32 {
        if high <= low {
            return low;
        }
        low + self.below(high - low + 1)
    }

    /// Whether a draw of `of` comes up below `num`.
    fn chance(&mut self, num: u32, of: u32) -> bool {
        self.below(of) < num
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let at = usize::try_from(self.next() % items.len().max(1) as u64).unwrap_or(0);
        &items[at]
    }
}

/// The height of a round's first block.
const fn round_first(round: u32) -> u32 {
    (round - 1) * ROUND + 1
}

/// The round a height is in.
const fn round_of(height: u32) -> u32 {
    (height - 1) / ROUND + 1
}

/// Whether a batch's seed can name a corpus directory.
fn valid_seed(seed: &str) -> bool {
    !seed.is_empty()
        && seed.len() <= 64
        && seed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        && !seed.starts_with('.')
}

/// The height at which the donations' milestone of chain `seed` of `rounds` rounds activates
/// when the chain has a milestone inside it: three in four chains do, at a height after the
/// second block and before the tip.
fn milestone_of(seed: &str, rounds: u32) -> Option<u32> {
    let mut rng = Rng::of(&["milestone", seed, &rounds.to_string()]);
    let tip = rounds * ROUND;
    rng.chance(3, 4).then(|| rng.between(3, tip - 1))
}

impl DrawnChain {
    /// The height the donations' milestone activates at in this chain's genesis profile (the
    /// profile's own is height 2), when the draw puts one inside the chain.
    pub fn donations_from(&self) -> Option<u32> {
        milestone_of(&self.seed, self.rounds)
    }
}

/// A delegate the recipe registers.
struct Registered {
    username: String,
    /// The round of its registration.
    round: u32,
}

/// The recipe being drawn.
struct Plan {
    rng: Rng,
    index: u32,
    rounds: u32,
    tip: u32,
    long: bool,
    /// The wallets that send transactions.
    senders: Vec<String>,
    /// The transactions by height, in submission order.
    transactions: BTreeMap<u32, Vec<BuildSpec>>,
    steps: Vec<Step>,
    registered: Vec<Registered>,
    /// The heights of the registrations of `registered`.
    registrations: Vec<u32>,
    /// The heights after which a revert or a fork runs.
    after: BTreeSet<u32>,
    restarts: BTreeSet<u32>,
}

fn spec(kind: &str, sender: &str, asset: Value) -> BuildSpec {
    BuildSpec {
        kind: kind.to_owned(),
        sender: sender.to_owned(),
        asset,
        fee: None,
        memo: None,
        refused: None,
    }
}

fn transfer(sender: &str, to: &[(String, u64)]) -> BuildSpec {
    let recipients: Vec<Value> = to
        .iter()
        .map(|(label, amount)| json!([label, amount.to_string()]))
        .collect();
    spec("transfer", sender, json!({ "recipients": recipients }))
}

fn vote(sender: &str, votes: &[(String, u32)]) -> BuildSpec {
    let votes: Vec<Value> = votes
        .iter()
        .map(|(name, percent)| json!([name, percent.to_string()]))
        .collect();
    spec("vote", sender, json!({ "votes": votes }))
}

fn resignation(sender: &str, kind: u32) -> BuildSpec {
    spec(
        "delegateResignation",
        sender,
        json!({ "resignationType": kind }),
    )
}

impl Plan {
    fn new(seed: &str, index: u32, rounds: u32, long: bool) -> Plan {
        let mut rng = Rng::of(&["recipe", seed, &index.to_string(), &rounds.to_string()]);
        let wallets = rng.between(6, 10);
        Plan {
            rng,
            index,
            rounds,
            tip: rounds * ROUND,
            long,
            senders: (1..=wallets).map(|i| format!("w{i}")).collect(),
            transactions: BTreeMap::new(),
            steps: Vec::new(),
            registered: Vec::new(),
            registrations: Vec::new(),
            after: BTreeSet::new(),
            restarts: BTreeSet::new(),
        }
    }

    fn add(&mut self, height: u32, spec: BuildSpec) {
        self.transactions.entry(height).or_default().push(spec);
    }

    /// A height from the third block to the tip.
    fn any_height(&mut self) -> u32 {
        let tip = self.tip;
        self.rng.between(3, tip)
    }

    /// A height inside `round`, at least `from` blocks into the chain.
    fn in_round(&mut self, round: u32, from: u32) -> u32 {
        let first = round_first(round).max(from);
        let last = round * ROUND;
        self.rng.between(first, last)
    }

    /// A height in the middle of `round`, clear of the blocks a revert across its ends takes back.
    fn mid_round(&mut self, round: u32) -> u32 {
        let first = round_first(round);
        (first + self.rng.between(12, 44)).min(self.tip - 2)
    }

    fn sender(&mut self) -> String {
        self.rng.pick(&self.senders.clone()).clone()
    }

    fn voted(&mut self) -> String {
        format!("genesis_{}", self.rng.between(1, VOTED))
    }

    fn amount(&mut self) -> u64 {
        u64::from(self.rng.between(1_000, 900_000)) * 1_000
    }

    /// The funding of every wallet at height 2.
    fn funding(&mut self, extra: &[&str]) {
        let to: Vec<(String, u64)> = self
            .senders
            .iter()
            .map(String::as_str)
            .chain(extra.iter().copied())
            .map(|label| (label.to_owned(), FUNDING.parse().unwrap_or(0)))
            .collect();
        self.add(2, transfer("genesis-1", &to));
    }

    /// Transfers of every shape, votes, burns, and a burst of senders at one height.
    fn mix(&mut self) {
        let events = 6 + 2 * self.rounds.min(10) + self.rounds / 5;
        for _ in 0..events {
            let height = self.any_height();
            let sender = self.sender();
            let item = match self.rng.below(20) {
                0..=6 => {
                    let (to, amount) = (self.sender(), self.amount());
                    transfer(&sender, &[(to, amount)])
                }
                7..=9 => {
                    let count = self.rng.between(2, 6);
                    let to: Vec<(String, u64)> =
                        (0..count).map(|_| (self.sender(), self.amount())).collect();
                    transfer(&sender, &to)
                }
                10..=12 => {
                    let (to, amount) = (self.sender(), self.amount());
                    let mut item = transfer(&sender, &[(to, amount)]);
                    item.memo = Some(format!("batch {} {}", self.index, self.rng.below(1000)));
                    if self.rng.chance(1, 2) {
                        item.fee = Some(format!("{}", self.rng.between(10, 90) * 1_000_000));
                    }
                    item
                }
                13..=15 => {
                    let name = self.voted();
                    if self.rng.chance(1, 3) {
                        let other = self.voted();
                        if other != name {
                            self.add(height, vote(&sender, &[(name, 60), (other, 40)]));
                            continue;
                        }
                    }
                    vote(&sender, &[(name, 100)])
                }
                16..=17 => spec(
                    "burn",
                    &sender,
                    json!({ "amount": (self.amount() / 1_000 + 100_000_000).to_string() }),
                ),
                _ => {
                    // A burst: several wallets send one or more transfers in one batch.
                    let senders = self.senders.clone();
                    for who in senders.iter().take(self.rng.between(3, 8) as usize) {
                        for _ in 0..self.rng.between(1, 3) {
                            let (to, amount) = (self.sender(), self.amount());
                            self.add(height, transfer(who, &[(to, amount)]));
                        }
                    }
                    continue;
                }
            };
            self.add(height, item);
        }
        // Whatever the draw, a vote and a burn.
        let (height, sender, name) = (self.any_height(), self.sender(), self.voted());
        self.add(height, vote(&sender, &[(name, 100)]));
        let (height, sender) = (self.any_height(), self.sender());
        self.add(
            height,
            spec("burn", &sender, json!({ "amount": "100000000" })),
        );
        // A wide multi-payment to many recipients, from one wallet.
        let height = self.any_height();
        let sender = self.sender();
        let count = self.rng.between(20, 60);
        let to: Vec<(String, u64)> = (0..count)
            .map(|i| {
                (
                    self.senders[i as usize % self.senders.len()].clone(),
                    self.amount(),
                )
            })
            .collect();
        self.add(height, transfer(&sender, &to));
    }

    /// The expected refusals: a vote for a delegate no recipe attests, and the removed type.
    fn refusals(&mut self) {
        for _ in 0..self.rng.between(1, 3) {
            let height = self.any_height();
            let sender = self.sender();
            let name = (*self.rng.pick(&UNATTESTED)).to_owned();
            let mut item = vote(&sender, &[(name, 100)]);
            item.refused = Some("ERR_OFFLINE".into());
            self.add(height, item);
        }
        let height = self.any_height();
        let sender = self.sender();
        let mut item = spec("ipfs", &sender, json!({ "hash": IPFS_HASH }));
        item.refused = Some("ERR_OTHER".into());
        self.add(height, item);
    }

    /// The second signature, which its wallet never sends from again.
    fn second_signature(&mut self) {
        let height = self.any_height();
        let phrase = format!("batch second passphrase {}", self.index);
        self.add(
            height,
            spec(
                "secondSignature",
                "sig",
                json!({ "secondPassphrase": phrase }),
            ),
        );
    }

    /// The delegates the recipe registers: the first always, which wins a vote in the drawn chains
    /// that have rounds for it; the second, which resigns for a time and revokes it, where the
    /// chain is long enough for the revoke.
    ///
    /// A registered delegate takes a place among the active ones from the next round, with votes
    /// or without (every delegate has the same weight and the keys decide), so each is silent for
    /// [`SILENT_BLOCKS`] blocks from its registration: its first block is then more than a job of
    /// the reference's download cut after the registration, and no job holds both. The judge's
    /// one-block jobs and the cut's jobs give the same answers on such a chain. (A delegate whose
    /// registration and first block fall in one job is a case of its own, which the processor
    /// replay shows as a mismatch; [`registered_forgers`] names every chain whose delegate forges
    /// within one download.)
    fn delegates(&mut self) {
        let wins = self.rounds >= 3 && !self.index.is_multiple_of(3);
        let revokes = wins && self.rounds >= 6 && self.index.is_multiple_of(2);
        let last_round = self.rounds.saturating_sub(2).clamp(1, 8);
        let round = self.rng.between(1, last_round);
        let height = self.mid_round(round);
        let name = format!("bd{}x1", self.index);
        self.add(
            height,
            spec("delegateRegistration", "d1", json!({ "username": name })),
        );
        self.register(&name, height);
        let mut resigned = height + self.rng.between(1, 20);
        if wins {
            let voting = self
                .rng
                .between(round + 1, (round + 3).min(self.rounds - 1));
            let at = self.in_round(voting, 3);
            let voter = self.sender();
            self.add(at, vote(&voter, &[(name, 100)]));
            // It resigns after it forged: in a later round, or at the tip.
            let later = self.rng.between(voting + 1, self.rounds);
            resigned = self.in_round(later, at + 1);
        }
        // For good or for a time, as it is drawn: 1 is a permanent resignation, 0 a temporary one.
        let kind = u32::from(self.rng.chance(1, 2));
        self.add(
            resigned.max(height + 1).min(self.tip),
            resignation("d1", kind),
        );
        if revokes {
            let name = format!("bd{}x2", self.index);
            let round = self.rng.between(1, 2);
            let at = self.mid_round(round);
            self.add(
                at,
                spec("delegateRegistration", "d2", json!({ "username": name })),
            );
            self.register(&name, at);
            let voting = round + 1;
            let vote_at = self.in_round(voting, 3);
            let voter = self.sender();
            self.add(vote_at, vote(&voter, &[(name, 100)]));
            // Its resignation is temporary (type 0): the revoke (type 2) is valid 106 blocks on.
            let rests = self.in_round(voting + 1, vote_at + 1);
            let back = rests + 110;
            if back <= self.tip {
                self.add(rests, resignation("d2", 0));
                self.add(back, resignation("d2", 2));
            } else {
                self.add(rests, resignation("d2", 1));
            }
        }
    }

    /// Note the delegate `name`, registered at `height`: a restart point there, its name given
    /// to the online steps of later rounds, and its forger stopped for [`SILENT_BLOCKS`].
    fn register(&mut self, name: &str, height: u32) {
        self.restarts.insert(height);
        self.registrations.push(height);
        self.registered.push(Registered {
            username: name.to_owned(),
            round: round_of(height),
        });
        self.steps.push(Step::Missed {
            delegate: name.to_owned(),
            from_height: height,
            to_height: (height + SILENT_BLOCKS).min(self.tip),
        });
    }

    /// Missed-slot patterns: one delegate over a long stretch, a few over short ones that cross a
    /// round start, one over a whole round, and several at once.
    fn missed(&mut self) {
        let tip = self.tip;
        let rounds = self.rounds;
        let delegate = |rng: &mut Rng| format!("genesis_{}", rng.between(1, 49));
        let from = self.rng.between(2, tip.saturating_sub(20).max(2));
        let to = (from + self.rng.between(20, 80)).min(tip);
        let name = delegate(&mut self.rng);
        self.steps.push(Step::Missed {
            delegate: name,
            from_height: from,
            to_height: to,
        });
        if rounds >= 2 {
            for _ in 0..self.rng.between(1, 3) {
                let round = self.rng.between(2, rounds);
                let from = round_first(round) - self.rng.between(1, 10);
                let to = (from + self.rng.between(10, 30)).min(tip);
                let name = delegate(&mut self.rng);
                self.steps.push(Step::Missed {
                    delegate: name,
                    from_height: from.max(2),
                    to_height: to,
                });
            }
        }
        if self.rng.chance(1, 2) {
            let round = self.rng.between(1, rounds);
            let name = delegate(&mut self.rng);
            self.steps.push(Step::Missed {
                delegate: name,
                from_height: round_first(round).max(2),
                to_height: round * ROUND,
            });
        }
        if self.rng.chance(1, 2) {
            let round = self.rng.between(1, rounds);
            let first = round_first(round).max(2);
            let to = (first + self.rng.between(5, 25)).min(tip);
            let want = self.rng.between(3, 6) as usize;
            let mut down = BTreeSet::new();
            while down.len() < want {
                down.insert(self.rng.between(1, 49));
            }
            for number in down {
                self.steps.push(Step::Missed {
                    delegate: format!("genesis_{number}"),
                    from_height: first,
                    to_height: to,
                });
            }
        }
    }

    /// Whether a revert or fork after `height` stays clear of the long chain's expiry window,
    /// whose blocks the recipe's proof reads.
    fn clear_of_proof(&self, height: u32) -> bool {
        !(self.long
            && (round_first(EXPIRY_ROUND - 1)..=round_first(EXPIRY_ROUND) + 25).contains(&height))
    }

    /// Whether a revert or fork of `blocks` blocks after `height` leaves alone the registrations
    /// of delegates that a round start has attested since: the reference keeps a delegate's
    /// online entry in its wallet, which the revert of its registration takes away, and a vote for
    /// the delegate is then refused there until the next round's attestation, where Heartwood's
    /// record outlives the revert and admits it. A registration reverted within its own round, with
    /// no attestation yet, is drawn.
    fn clear_of_attested_registrations(&self, height: u32, blocks: u32) -> bool {
        let first = height - blocks + 1;
        !self
            .registrations
            .iter()
            .any(|&at| (first..=height).contains(&at) && round_of(height) > round_of(at))
    }

    /// Reverts across round ends, within a round, and forks.
    fn reverts(&mut self) {
        let rounds = self.rounds;
        let tip = self.tip;
        // Across a round end: back into the round before it, never to its last block. One is
        // always drawn, and up to two more; a draw the long chain's proof window or an earlier
        // revert takes is drawn again.
        if rounds >= 2 {
            let wanted = self.rng.between(1, 3);
            let mut made = 0;
            for _ in 0..32 {
                if made == wanted {
                    break;
                }
                let end = self.rng.between(1, rounds - 1) * ROUND;
                let past = self.rng.between(2, 10);
                let back = self.rng.between(1, 8);
                let (height, blocks) = (end + past, past + back);
                if height > tip
                    || !self.clear_of_proof(height)
                    || !self.clear_of_attested_registrations(height, blocks)
                    || !self.after.insert(height)
                {
                    continue;
                }
                self.steps.push(Step::Revert {
                    after_height: height,
                    blocks,
                });
                made += 1;
            }
        }
        // Inside a round.
        for _ in 0..self.rng.between(1, 3) {
            let round = self.rng.between(1, rounds);
            let height = self.in_round(round, 12);
            let position = height - round_first(round) + 1;
            let blocks = self.rng.between(1, position.saturating_sub(2).clamp(1, 20));
            if position <= blocks + 1
                || !self.clear_of_proof(height)
                || !self.clear_of_attested_registrations(height, blocks)
                || !self.after.insert(height)
            {
                continue;
            }
            self.steps.push(Step::Revert {
                after_height: height,
                blocks,
            });
        }
        // Forks stay inside one round.
        let forks = if self.rng.chance(2, 3) {
            self.rng.between(1, 2)
        } else {
            0
        };
        for _ in 0..forks {
            let round = self.rng.between(1, rounds);
            let height = self.in_round(round, 12);
            let position = height - round_first(round) + 1;
            let blocks = self.rng.between(1, 4);
            if position <= blocks + 2
                || !self.clear_of_proof(height)
                || !self.clear_of_attested_registrations(height, blocks)
                || !self.after.insert(height)
            {
                continue;
            }
            self.steps.push(Step::Fork {
                after_height: height,
                blocks,
            });
        }
    }

    /// Restart points: a round start, a round end, the heights after a revert or a fork, and a
    /// few anywhere.
    fn restarts(&mut self, milestone: Option<u32>) {
        let first = self.rng.between(self.rounds.min(2), self.rounds);
        self.restarts.insert(round_first(first).max(2));
        let last = self.rng.between(1, self.rounds);
        self.restarts.insert(last * ROUND);
        let after: Vec<u32> = self.after.iter().copied().collect();
        for height in after.into_iter().take(2) {
            self.restarts.insert(height);
        }
        if let Some(height) = milestone {
            self.restarts.insert(height);
        }
        for _ in 0..self.rng.between(1, 3) {
            let height = self.rng.between(2, self.tip);
            self.restarts.insert(height);
        }
    }

    /// The online steps: every round attests every delegate but the three no one attests; once a
    /// delegate is registered, the names are given, so that a vote for it is admitted; a long
    /// chain stops attesting its expiring delegate after round 1.
    fn online(&mut self) {
        for round in 1..=self.rounds {
            let expired = self.long && self.rounds >= EXPIRY_ROUND && round > 1;
            let mut but: Vec<String> = UNATTESTED.iter().map(|n| (*n).to_owned()).collect();
            if expired {
                but.push(EXPIRING.to_owned());
            }
            let known: Vec<&Registered> =
                self.registered.iter().filter(|r| r.round < round).collect();
            let delegates = if known.is_empty() {
                Attested::AllBut(but)
            } else {
                let mut names: Vec<String> = (1..=53)
                    .map(|i| format!("genesis_{i}"))
                    .filter(|n| !but.contains(n))
                    .collect();
                names.extend(known.iter().map(|r| r.username.clone()));
                Attested::Only(names)
            };
            self.steps.push(Step::Online {
                round: u64::from(round),
                delegates,
            });
        }
    }

    /// A long chain's proof of the online record's expiry: a vote for the expiring delegate in
    /// round 6, while its entry stands, which both pools admit; and one at round 7's first block,
    /// which both refuse as `ERR_OFFLINE`.
    fn expiry(&mut self) {
        let before = round_first(EXPIRY_ROUND - 1) + self.rng.between(5, 40);
        self.add(before, vote("w1", &[(EXPIRING.to_owned(), 100)]));
        let mut refused = vote("w2", &[(EXPIRING.to_owned(), 100)]);
        refused.refused = Some("ERR_OFFLINE".into());
        self.add(round_first(EXPIRY_ROUND), refused);
    }

    /// The pool refuses a second vote of a sender while its first is pending, before it looks at
    /// the delegate, so no sender has two votes in one height's batch: the later vote stays, the
    /// earlier moves to the next height (and is dropped past the tip).
    fn one_vote_per_sender(&mut self) {
        let mut carried: Vec<BuildSpec> = Vec::new();
        for height in 2..=self.tip {
            let mut list = std::mem::take(&mut carried);
            list.extend(self.transactions.remove(&height).unwrap_or_default());
            let mut senders = BTreeSet::new();
            let mut kept = Vec::new();
            for item in list.into_iter().rev() {
                if item.kind == "vote" && !senders.insert(item.sender.clone()) {
                    carried.push(item);
                } else {
                    kept.push(item);
                }
            }
            kept.reverse();
            carried.reverse();
            if !kept.is_empty() {
                self.transactions.insert(height, kept);
            }
        }
    }

    /// A vote that would repeat the wallet's last vote, in height order, votes for the next
    /// delegate instead.
    fn vary_votes(&mut self) {
        let mut last: BTreeMap<String, Value> = BTreeMap::new();
        for build in self.transactions.values_mut() {
            for item in build
                .iter_mut()
                .filter(|b| b.kind == "vote" && b.refused.is_none())
            {
                if last.get(&item.sender) == Some(&item.asset)
                    && let Some(vote) = item.asset["votes"][0][0].as_str()
                    && let Some(number) = vote
                        .strip_prefix("genesis_")
                        .and_then(|n| n.parse::<u32>().ok())
                {
                    item.asset["votes"] =
                        json!([[format!("genesis_{}", number % VOTED + 1), "100"]]);
                }
                last.insert(item.sender.clone(), item.asset.clone());
            }
        }
    }

    /// The steps, in height order.
    fn finish(mut self) -> Recipe {
        self.one_vote_per_sender();
        self.vary_votes();
        for (height, build) in std::mem::take(&mut self.transactions) {
            self.steps.push(Step::Transactions { height, build });
        }
        for height in std::mem::take(&mut self.restarts) {
            self.steps.push(Step::Restart { height });
        }
        let at = |step: &Step| -> (u32, u8) {
            match step {
                Step::Online { round, .. } => (round_first(u32::try_from(*round).unwrap_or(1)), 0),
                Step::Missed { from_height, .. } => (*from_height, 1),
                Step::Transactions { height, .. } => (*height, 2),
                Step::Revert { after_height, .. } | Step::Fork { after_height, .. } => {
                    (*after_height, 3)
                }
                Step::Restart { height } => (*height, 4),
            }
        };
        self.steps.sort_by_key(at);
        let mut wallets = self.senders;
        wallets.extend(["sig".to_owned(), "d1".to_owned(), "d2".to_owned()]);
        Recipe {
            wallets,
            steps: self.steps,
        }
    }
}

/// Draw the recipe of the chain `index` (from 1) of the batch `seed`, of `rounds` rounds (a long
/// chain when `long`). The same arguments always draw the same chain.
///
/// A long chain has at least [`EXPIRY_ROUND`] rounds, the fewest that show its expiry step.
pub fn draw(seed: &str, index: u32, rounds: u32, long: bool) -> Result<DrawnChain> {
    if !valid_seed(seed) {
        return Err(Error::Usage(
            "a batch seed is 1 to 64 letters, digits, dots, dashes and underscores, and does not \
             start with a dot"
                .into(),
        ));
    }
    if index == 0 {
        return Err(Error::Usage("a batch's chains are numbered from 1".into()));
    }
    if rounds == 0 || rounds > LONG_ROUNDS {
        return Err(Error::Usage(format!(
            "a chain has 1 to {LONG_ROUNDS} rounds"
        )));
    }
    if rounds > SHORT_ROUNDS && !long {
        return Err(Error::Usage(format!(
            "a chain of more than {SHORT_ROUNDS} rounds is a long chain"
        )));
    }
    if long && rounds < EXPIRY_ROUND {
        return Err(Error::Usage(format!(
            "a long chain has at least {EXPIRY_ROUND} rounds, the fewest that show its online \
             entry's expiry"
        )));
    }
    let name = format!("batch-{seed}-{index}");
    let chain_seed = format!(
        "{:016x}",
        Rng::of(&["seed", seed, &index.to_string()]).next()
    );
    let milestone = milestone_of(&chain_seed, rounds);
    let mut plan = Plan::new(seed, index, rounds, long);
    plan.funding(&["sig", "d1", "d2"]);
    plan.mix();
    plan.refusals();
    plan.second_signature();
    plan.delegates();
    if long {
        plan.expiry();
    }
    plan.missed();
    plan.reverts();
    plan.restarts(milestone);
    if let Some(height) = milestone {
        // A transfer in the block that activates the milestone.
        let (sender, to, amount) = (plan.sender(), plan.sender(), plan.amount());
        plan.add(height, transfer(&sender, &[(to, amount)]));
    }
    plan.online();
    let recipe = plan.finish();
    Ok(DrawnChain {
        index,
        name,
        seed: chain_seed,
        rounds,
        long,
        recipe,
    })
}

// ---- the batch ----------------------------------------------------------------------------------

/// Makes one drawn chain from its profile into its directory.
pub type Maker<'a> =
    dyn Fn(&DrawnChain, &str, &Path) -> std::result::Result<MadeChain, LockstepError> + Sync + 'a;

/// The genesis profile text `profile` names, with the donations' milestone at `height`, written
/// as `name` into `dir`.
fn profile_with_milestone(profile: &str, height: u32, dir: &Path, name: &str) -> Result<PathBuf> {
    const KEY: &str = "\"donationsFromHeight\":";
    let path = super::profile_path(profile);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let start = text
        .find(KEY)
        .ok_or_else(|| Error::Usage(format!("{}: no donationsFromHeight", path.display())))?
        + KEY.len();
    let digits = text[start..]
        .trim_start()
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    let skipped = text[start..].len() - text[start..].trim_start().len();
    let end = start + skipped + digits;
    let changed = format!("{} {height}{}", &text[..start], &text[end..]);
    let out = dir.join(format!("{name}.json"));
    std::fs::write(&out, changed).map_err(|e| Error::io(&out, e))?;
    Ok(out)
}

/// The chains made at once when the arguments do not say: the machine's parallelism, at most 8.
fn default_jobs(count: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .clamp(1, 8)
        .min(count.max(1))
}

/// `difftest lockstep batch`: draw and make the chains of `args` at once, write the batch file,
/// and return it. A chain the producer stops is kept in the batch with its reason, and fails it.
pub fn run(args: &LockstepBatchArgs) -> Result<Batch> {
    run_with(args, &|chain, profile, out| {
        super::make(
            &chain.seed,
            profile,
            chain.rounds,
            &chain.recipe,
            out,
            &args.oracle,
        )
    })
}

/// As [`run`], with `maker` making each chain.
pub fn run_with(args: &LockstepBatchArgs, maker: &Maker<'_>) -> Result<Batch> {
    if args.count == 0 {
        return Err(Error::Usage("a batch has at least one chain".into()));
    }
    let drawn: Vec<DrawnChain> = (1..=args.count)
        .map(|index| draw(&args.seed, index, args.rounds, args.long))
        .collect::<Result<_>>()?;
    std::fs::create_dir_all(&args.out).map_err(|e| Error::io(&args.out, e))?;
    for chain in &drawn {
        let dir = args.out.join(&chain.name);
        if dir.exists() {
            return Err(Error::Usage(format!(
                "{} exists: remove it, or draw the batch under another seed",
                dir.display()
            )));
        }
    }
    let scratch = ScratchDir::new("lockstep-batch")?;
    let profiles: Vec<String> = drawn
        .iter()
        .map(|chain| match chain.donations_from() {
            Some(height) => profile_with_milestone(
                &args.profile,
                height,
                scratch.path(),
                &format!("profile-{}", chain.index),
            )
            .map(|path| path.to_string_lossy().into_owned()),
            None => Ok(args.profile.clone()),
        })
        .collect::<Result<_>>()?;
    let jobs = args
        .jobs
        .unwrap_or_else(|| default_jobs(drawn.len()))
        .max(1);
    let next = AtomicUsize::new(0);
    let made: Mutex<BTreeMap<usize, BatchChain>> = Mutex::new(BTreeMap::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(drawn.len()) {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let (Some(chain), Some(profile)) = (drawn.get(at), profiles.get(at)) else {
                        break;
                    };
                    let outcome = maker(chain, profile, &args.out.join(&chain.name));
                    let entry = match outcome {
                        Ok(made) => {
                            eprintln!(
                                "lockstep batch: {} made to height {}, {} transactions",
                                chain.name, made.tip, made.transactions
                            );
                            BatchChain {
                                drawn: chain.clone(),
                                tip: Some(made.tip),
                                transactions: made.transactions,
                                missed: made.missed,
                                stopped: None,
                            }
                        }
                        Err(stop) => {
                            eprintln!("lockstep batch: {} stopped: {stop}", chain.name);
                            BatchChain {
                                drawn: chain.clone(),
                                tip: None,
                                transactions: 0,
                                missed: 0,
                                stopped: Some(stop.to_string()),
                            }
                        }
                    };
                    if let Ok(mut done) = made.lock() {
                        done.insert(at, entry);
                    }
                }
            });
        }
    });
    let chains: Vec<BatchChain> = made
        .into_inner()
        .map_err(|_| Error::Usage("a batch worker stopped".into()))?
        .into_values()
        .collect();
    let batch = Batch {
        format: BATCH_FORMAT.to_owned(),
        seed: args.seed.clone(),
        profile: args.profile.clone(),
        chains,
    };
    write_batch(&args.out, &batch)?;
    Ok(batch)
}

/// Write `batch` as its batch file, through a rename.
fn write_batch(out: &Path, batch: &Batch) -> Result<()> {
    let path = batch_file(out, &batch.seed);
    let temp = out.join(format!(".batch-{}.json.tmp", batch.seed));
    let text = serde_json::to_string_pretty(batch)
        .map_err(|e| Error::Usage(format!("the batch file: {e}")))?;
    std::fs::write(&temp, format!("{text}\n")).map_err(|e| Error::io(&temp, e))?;
    std::fs::rename(&temp, &path).map_err(|e| Error::io(&path, e))
}

/// Read the batch file `path`.
pub fn read_batch(path: &Path) -> Result<Batch> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let batch: Batch = serde_json::from_str(&text).map_err(|e| Error::Corpus {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    if batch.format != BATCH_FORMAT {
        return Err(Error::Corpus {
            path: path.to_path_buf(),
            message: format!("not a {BATCH_FORMAT} file"),
        });
    }
    Ok(batch)
}

// ---- delegates the recipe registers that then forge ---------------------------------------------

/// A delegate a chain registers that forges within one download of its registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredForger {
    /// The delegate's name.
    pub username: String,
    /// The height of its registration step: the block that holds it is this one.
    pub registered_at: u32,
    /// The first block it forged after that.
    pub first_block: u32,
    /// The blocks it forged within [`DOWNLOAD_BLOCKS`] of the registration.
    pub blocks: u32,
}

/// The delegates `chain` registers that forge within [`DOWNLOAD_BLOCKS`] blocks of their
/// registration, read from the blocks' generators in the corpus under `out`. A syncing node
/// downloads a registration and the delegate's first block together then, which the judge's
/// one-block jobs do not.
pub fn registered_forgers(chain: &BatchChain, out: &Path) -> Result<Vec<RegisteredForger>> {
    let Some(made) = chain.made(out) else {
        return Ok(Vec::new());
    };
    let mut registrations: Vec<(String, u32)> = Vec::new();
    for step in &chain.drawn.recipe.steps {
        let Step::Transactions { height, build } = step else {
            continue;
        };
        for item in build
            .iter()
            .filter(|b| b.kind == "delegateRegistration" && b.refused.is_none())
        {
            if let Some(name) = item.asset["username"].as_str() {
                registrations.push((name.to_owned(), *height));
            }
        }
    }
    if registrations.is_empty() {
        return Ok(Vec::new());
    }
    let path = made.dir.join("reference").join("meta.jsonl");
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let mut generators: Vec<(u32, String)> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let row: Value = serde_json::from_str(line).map_err(|e| Error::Parse {
            path: path.clone(),
            line: i + 1,
            message: e.to_string(),
        })?;
        if let (Some(height), Some(name)) = (row["h"].as_u64(), row["username"].as_str()) {
            generators.push((u32::try_from(height).unwrap_or(u32::MAX), name.to_owned()));
        }
    }
    let mut out = Vec::new();
    for (username, registered_at) in registrations {
        let forged: Vec<u32> = generators
            .iter()
            .filter(|(h, name)| {
                *name == username && *h > registered_at && *h - registered_at < DOWNLOAD_BLOCKS
            })
            .map(|(h, _)| *h)
            .collect();
        if let Some(first) = forged.first() {
            out.push(RegisteredForger {
                username,
                registered_at,
                first_block: *first,
                blocks: u32::try_from(forged.len()).unwrap_or(u32::MAX),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(recipe: &Recipe) -> BTreeSet<String> {
        recipe
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Transactions { build, .. } => Some(build),
                _ => None,
            })
            .flatten()
            .map(|b| b.kind.clone())
            .collect()
    }

    #[test]
    fn the_same_arguments_draw_the_same_chain_and_others_another() {
        let a = draw("seed", 3, 5, false).unwrap();
        assert_eq!(a, draw("seed", 3, 5, false).unwrap());
        assert_eq!(a.name, "batch-seed-3");
        assert_ne!(a.recipe, draw("seed", 4, 5, false).unwrap().recipe);
        assert_ne!(a.seed, draw("seed", 4, 5, false).unwrap().seed);
        assert_ne!(a.recipe, draw("other", 3, 5, false).unwrap().recipe);
        assert_ne!(a.recipe, draw("seed", 3, 4, false).unwrap().recipe);
    }

    #[test]
    fn the_draw_refuses_what_a_chain_cannot_be() {
        assert!(draw("seed", 0, 5, false).is_err());
        assert!(draw("seed", 1, 0, false).is_err());
        assert!(draw("seed", 1, 6, false).is_err());
        assert!(draw("seed", 1, 51, true).is_err());
        assert!(
            draw("seed", 1, 6, true).is_err(),
            "too short to show the expiry"
        );
        assert!(draw("../x", 1, 5, false).is_err());
        assert!(draw("", 1, 5, false).is_err());
        assert!(draw("seed", 1, 7, true).is_ok());
    }

    #[test]
    fn every_chain_carries_every_kept_type_the_refusals_and_a_step_of_each_kind() {
        for rounds in 1..=5 {
            for index in 1..=8 {
                let chain = draw("coverage", index, rounds, false).unwrap();
                let kinds = kinds(&chain.recipe);
                for kind in [
                    "transfer",
                    "vote",
                    "delegateRegistration",
                    "delegateResignation",
                    "burn",
                    "secondSignature",
                    "ipfs",
                ] {
                    assert!(kinds.contains(kind), "{kind} in {}", chain.name);
                }
                let steps = &chain.recipe.steps;
                let has = |f: &dyn Fn(&Step) -> bool| steps.iter().any(f);
                assert!(has(&|s| matches!(s, Step::Missed { .. })), "{}", chain.name);
                assert!(has(&|s| matches!(s, Step::Revert { .. })), "{}", chain.name);
                assert!(
                    has(&|s| matches!(s, Step::Restart { .. })),
                    "{}",
                    chain.name
                );
                let online: Vec<u64> = steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Online { round, .. } => Some(*round),
                        _ => None,
                    })
                    .collect();
                assert_eq!(online, (1..=u64::from(rounds)).collect::<Vec<_>>());
                let refused: BTreeSet<&str> = steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Transactions { build, .. } => Some(build),
                        _ => None,
                    })
                    .flatten()
                    .filter_map(|b| b.refused.as_deref())
                    .collect();
                assert_eq!(refused, BTreeSet::from(["ERR_OFFLINE", "ERR_OTHER"]));
            }
        }
    }

    #[test]
    fn from_two_rounds_on_every_chain_reverts_across_a_round_end_and_restarts_at_both_ends() {
        for rounds in [2, 3, 5, 7, 9, 50] {
            for index in 1..=20 {
                let chain = draw("crossing", index, rounds, rounds > 5).unwrap();
                let steps = &chain.recipe.steps;
                assert!(
                    steps
                        .iter()
                        .any(|s| matches!(s, Step::Revert { after_height, blocks }
                        if round_of(*after_height) != round_of(after_height - blocks))),
                    "{} of {rounds} rounds",
                    chain.name
                );
                let restarts: Vec<u32> = steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Restart { height } => Some(*height),
                        _ => None,
                    })
                    .collect();
                assert!(
                    restarts.iter().any(|h| (h - 1) % ROUND == 0 && *h > 2),
                    "a round start: {restarts:?}"
                );
                assert!(
                    restarts.iter().any(|h| h % ROUND == 0),
                    "a round end: {restarts:?}"
                );
            }
        }
    }

    #[test]
    fn a_ten_chain_batch_covers_forks_milestones_won_votes_and_the_long_chains_revokes() {
        let chains: Vec<DrawnChain> = (1..=10)
            .map(|i| draw("2026", i, 5, false).unwrap())
            .collect();
        let any = |f: &dyn Fn(&DrawnChain) -> bool| chains.iter().any(f);
        assert!(any(&|c| c
            .recipe
            .steps
            .iter()
            .any(|s| matches!(s, Step::Fork { .. }))));
        assert!(any(&|c| c.donations_from().is_some()));
        assert!(any(&|c| c.donations_from().is_none()));
        let crosses = |c: &DrawnChain| {
            c.recipe.steps.iter().any(|s| match s {
                Step::Revert {
                    after_height,
                    blocks,
                } => round_of(*after_height) != round_of(after_height - blocks),
                _ => false,
            })
        };
        assert!(chains.iter().filter(|c| crosses(c)).count() >= 5);
        // A registered delegate wins a vote in the chains that have rounds for it, and every
        // registered delegate is attested from the round after its registration.
        let transactions = |c: &DrawnChain| -> Vec<BuildSpec> {
            c.recipe
                .steps
                .iter()
                .filter_map(|s| match s {
                    Step::Transactions { build, .. } => Some(build.clone()),
                    _ => None,
                })
                .flatten()
                .collect()
        };
        let wins = |c: &DrawnChain| {
            transactions(c).iter().any(|b| {
                b.kind == "vote"
                    && b.refused.is_none()
                    && b.asset["votes"][0][0]
                        .as_str()
                        .is_some_and(|name| name.starts_with("bd"))
            })
        };
        assert!(chains.iter().filter(|c| wins(c)).count() >= 4);
        let attested = |c: &DrawnChain| {
            c.recipe.steps.iter().any(|s| match s {
                Step::Online {
                    delegates: Attested::Only(names),
                    ..
                } => names.iter().any(|n| n.starts_with("bd")),
                _ => false,
            })
        };
        assert!(chains.iter().filter(|c| attested(c)).count() >= 4);
        // A temporary resignation and its revoke (types 0 and 2) need 6 rounds or more: the long
        // chains draw them.
        let revokes = |c: &DrawnChain| {
            let types: BTreeSet<u64> = transactions(c)
                .iter()
                .filter(|b| b.kind == "delegateResignation")
                .filter_map(|b| b.asset["resignationType"].as_u64())
                .collect();
            types.contains(&0) && types.contains(&2)
        };
        assert!(!chains.iter().any(|c| revokes(c)));
        let long: Vec<DrawnChain> = (1..=10)
            .map(|i| draw("2026", i, 7, true).unwrap())
            .collect();
        assert!(long.iter().filter(|c| revokes(c)).count() >= 2);
        // Every milestone activates inside its chain, after the genesis profile's own.
        for c in &chains {
            if let Some(height) = c.donations_from() {
                assert!((3..c.rounds * ROUND).contains(&height));
            }
        }
    }

    #[test]
    fn a_registered_delegate_is_silent_for_over_a_job_after_its_registration() {
        let mut registrations = 0;
        for rounds in [1, 2, 4, 5, 7, 20] {
            for index in 1..=12 {
                let chain = draw("silent", index, rounds, rounds > 5).unwrap();
                let tip = rounds * ROUND;
                let silent: Vec<(&str, u32, u32)> = chain
                    .recipe
                    .steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Missed {
                            delegate,
                            from_height,
                            to_height,
                        } if delegate.starts_with("bd") => {
                            Some((delegate.as_str(), *from_height, *to_height))
                        }
                        _ => None,
                    })
                    .collect();
                for step in &chain.recipe.steps {
                    let Step::Transactions { height, build } = step else {
                        continue;
                    };
                    for item in build.iter().filter(|b| b.kind == "delegateRegistration") {
                        registrations += 1;
                        let name = item.asset["username"].as_str().unwrap();
                        assert!(
                            silent.iter().any(|(n, from, to)| {
                                *n == name
                                    && from <= height
                                    && *to >= (height + SILENT_BLOCKS).min(tip)
                            }),
                            "{name} of {} registered at {height} is not silent",
                            chain.name
                        );
                    }
                }
            }
        }
        assert!(registrations >= 60, "{registrations}");
    }

    #[test]
    fn no_revert_or_fork_takes_back_a_registration_after_a_round_start_attested_it() {
        let mut inside = 0;
        for rounds in [2, 3, 5, 7, 12] {
            for index in 1..=25 {
                let chain = draw("attested", index, rounds, rounds > 5).unwrap();
                let registrations: Vec<u32> = chain
                    .recipe
                    .steps
                    .iter()
                    .filter_map(|s| match s {
                        Step::Transactions { height, build } => Some((height, build)),
                        _ => None,
                    })
                    .flat_map(|(h, build)| {
                        build
                            .iter()
                            .filter(|b| b.kind == "delegateRegistration")
                            .map(move |_| *h)
                    })
                    .collect();
                for step in &chain.recipe.steps {
                    let (Step::Revert {
                        after_height,
                        blocks,
                    }
                    | Step::Fork {
                        after_height,
                        blocks,
                    }) = step
                    else {
                        continue;
                    };
                    for at in &registrations {
                        if (after_height - blocks + 1..=*after_height).contains(at) {
                            inside += 1;
                            assert_eq!(round_of(*after_height), round_of(*at), "{}", chain.name);
                        }
                    }
                }
            }
        }
        // A registration reverted in its own round is still drawn.
        assert!(inside >= 1);
    }

    #[test]
    fn no_sender_has_two_votes_in_one_height() {
        // The chain that once drew two votes of one wallet in one height, and 40 chains of each
        // of five lengths.
        let mut chains = vec![draw("explore3-r3", 8, 3, false).unwrap()];
        for rounds in [1, 2, 3, 5, 7] {
            for index in 1..=40 {
                chains.push(draw("votes", index, rounds, rounds > 5).unwrap());
            }
        }
        for chain in chains {
            for step in &chain.recipe.steps {
                let Step::Transactions { height, build } = step else {
                    continue;
                };
                let mut voters = BTreeSet::new();
                for item in build.iter().filter(|b| b.kind == "vote") {
                    assert!(
                        voters.insert(item.sender.clone()),
                        "{} votes twice at {height} in {}",
                        item.sender,
                        chain.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_long_chain_stops_attesting_one_delegate_after_round_one_and_proves_the_expiry() {
        let chain = draw("long", 1, 7, true).unwrap();
        let online: BTreeMap<u64, &Attested> = chain
            .recipe
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Online { round, delegates } => Some((*round, delegates)),
                _ => None,
            })
            .collect();
        let names = |a: &Attested| -> (bool, Vec<String>) {
            match a {
                Attested::AllBut(names) => (true, names.clone()),
                Attested::Only(names) => (false, names.clone()),
                Attested::All => (true, Vec::new()),
            }
        };
        let (_, round_one) = names(online[&1]);
        assert!(
            !round_one.contains(&EXPIRING.to_owned()),
            "attested in round 1"
        );
        for round in 2..=7 {
            let (all_but, list) = names(online[&round]);
            assert_eq!(
                all_but,
                list.contains(&EXPIRING.to_owned()),
                "round {round} leaves {EXPIRING} out"
            );
        }
        // The vote in round 6 is admitted; the one at round 7's first block is refused.
        let votes: Vec<(u32, bool)> = chain
            .recipe
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Transactions { height, build } => Some((height, build)),
                _ => None,
            })
            .flat_map(|(h, build)| {
                build
                    .iter()
                    .filter(|b| b.kind == "vote" && b.asset.to_string().contains(EXPIRING))
                    .map(move |b| (*h, b.refused.as_deref() == Some("ERR_OFFLINE")))
            })
            .collect();
        assert_eq!(votes.len(), 2, "{votes:?}");
        let (admitted, refused) = (votes[0], votes[1]);
        assert!((round_first(6)..=round_first(7) - 1).contains(&admitted.0));
        assert!(!admitted.1);
        assert_eq!(refused, (round_first(7), true));
        // No revert or fork touches the window of the proof.
        for step in &chain.recipe.steps {
            if let Step::Revert { after_height, .. } | Step::Fork { after_height, .. } = step {
                assert!(!(round_first(6)..=round_first(7) + 25).contains(after_height));
            }
        }
        // The default chain has no such step: its attested delegates stay attested.
        let plain = draw("long", 1, 5, false).unwrap();
        assert!(!plain.recipe.steps.iter().any(|s| matches!(s,
            Step::Online { delegates: Attested::AllBut(names), .. } if names.contains(&EXPIRING.to_owned()))));
    }

    #[test]
    fn no_wallet_sends_after_its_second_signature_and_every_label_is_a_wallet() {
        for index in 1..=12 {
            let chain = draw("labels", index, 5, false).unwrap();
            let mut labels: BTreeSet<&str> =
                chain.recipe.wallets.iter().map(String::as_str).collect();
            labels.insert("genesis-1");
            for step in &chain.recipe.steps {
                if let Step::Transactions { build, .. } = step {
                    for item in build {
                        assert!(labels.contains(item.sender.as_str()), "{}", item.sender);
                        if item.sender == "sig" {
                            assert_eq!(
                                item.kind, "secondSignature",
                                "the signing wallet only receives"
                            );
                        }
                        if let Some(list) = item.asset["recipients"].as_array() {
                            for pair in list {
                                assert!(labels.contains(pair[0].as_str().unwrap()));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_drawn_recipe_is_one_the_producer_accepts() {
        for rounds in [1, 2, 3, 5, 7, 12, 50] {
            for index in 1..=6 {
                let long = rounds > 5;
                let chain = draw("accepted", index, rounds, long).unwrap();
                let tip = rounds * ROUND;
                super::super::producer::check(&chain.recipe, rounds, tip)
                    .unwrap_or_else(|e| panic!("{} of {rounds} rounds: {e}", chain.name));
                // The recipe round-trips as the batch file writes it.
                let text = serde_json::to_string(&chain).unwrap();
                assert_eq!(serde_json::from_str::<DrawnChain>(&text).unwrap(), chain);
            }
        }
    }

    fn args(out: &Path, count: u32, jobs: usize) -> LockstepBatchArgs {
        LockstepBatchArgs {
            oracle: crate::cli::OracleArgs::default(),
            count,
            rounds: 2,
            seed: "unit".into(),
            long: false,
            profile: "devnet".into(),
            out: out.to_path_buf(),
            jobs: Some(jobs),
        }
    }

    #[test]
    fn a_batch_makes_its_chains_at_once_and_keeps_a_stopped_one_with_its_reason() {
        let scratch = ScratchDir::new("batch-unit").unwrap();
        let out = scratch.path().join("corpus");
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let maker: &Maker<'_> = &|chain, profile, dir| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(60));
            running.fetch_sub(1, Ordering::SeqCst);
            // The milestone's profile is a file with the drawn height, else the profile as given.
            match chain.donations_from() {
                Some(height) => {
                    let text = std::fs::read_to_string(profile).unwrap();
                    assert!(text.contains(&format!("\"donationsFromHeight\": {height},")));
                }
                None => assert_eq!(profile, "devnet"),
            }
            if chain.index == 3 {
                return Err(LockstepError::Recipe("no wallet is labelled x".into()));
            }
            Ok(MadeChain {
                dir: dir.to_path_buf(),
                tip: chain.rounds * ROUND,
                transactions: u64::from(chain.index),
                missed: 1,
            })
        };
        let batch = run_with(&args(&out, 5, 3), maker).unwrap();
        assert!(most.load(Ordering::SeqCst) >= 2, "chains were made at once");
        assert!(most.load(Ordering::SeqCst) <= 3, "no more than the jobs");
        let indexes: Vec<u32> = batch.chains.iter().map(|c| c.drawn.index).collect();
        assert_eq!(indexes, vec![1, 2, 3, 4, 5]);
        assert_eq!(batch.chains[2].tip, None);
        assert!(
            batch.chains[2]
                .stopped
                .as_deref()
                .unwrap()
                .contains("labelled x")
        );
        assert_eq!(batch.chains[3].tip, Some(106));
        assert_eq!(batch.chains[3].transactions, 4);
        assert!(batch.chains[0].made(&out).is_some());
        assert!(batch.chains[2].made(&out).is_none());
        // The batch file is the batch.
        assert_eq!(read_batch(&batch_file(&out, "unit")).unwrap(), batch);
        // A chain directory in the way stops the batch before anything is made.
        std::fs::create_dir_all(out.join("batch-other-2")).unwrap();
        let mut other = args(&out, 3, 1);
        other.seed = "other".into();
        let refused = run_with(&other, &|_, _, _| panic!("nothing is made")).unwrap_err();
        assert!(matches!(refused, Error::Usage(m) if m.contains("batch-other-2")));
    }

    #[test]
    fn the_milestone_profile_changes_the_height_and_nothing_else() {
        let scratch = ScratchDir::new("batch-profile").unwrap();
        let path = profile_with_milestone("devnet", 120, scratch.path(), "late").unwrap();
        let original = std::fs::read_to_string(super::super::profile_path("devnet")).unwrap();
        let changed = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            changed,
            original.replace(
                "\"donationsFromHeight\": 2,",
                "\"donationsFromHeight\": 120,"
            )
        );
    }

    #[test]
    fn registered_forgers_are_the_registered_delegates_that_forge_within_a_download() {
        let scratch = ScratchDir::new("batch-forgers").unwrap();
        let mut drawn = draw("forgers", 1, 5, false).unwrap();
        drawn.recipe.steps = vec![Step::Transactions {
            height: 10,
            build: vec![
                spec(
                    "delegateRegistration",
                    "d1",
                    json!({"username": "late_one"}),
                ),
                spec("delegateRegistration", "d2", json!({"username": "never"})),
            ],
        }];
        let chain = BatchChain {
            tip: Some(265),
            transactions: 2,
            missed: 0,
            stopped: None,
            drawn: drawn.clone(),
        };
        let dir = scratch.path().join(&drawn.name).join("reference");
        std::fs::create_dir_all(&dir).unwrap();
        let rows = [
            (9, "late_one"),
            (60, "late_one"),
            (200, "late_one"),
            (420, "late_one"),
            (20, "genesis_1"),
        ];
        let text: String = rows
            .iter()
            .map(|(h, n)| format!("{}\n", json!({"h": h, "username": n})))
            .collect();
        std::fs::write(dir.join("meta.jsonl"), text).unwrap();
        let found = registered_forgers(&chain, scratch.path()).unwrap();
        assert_eq!(
            found,
            vec![RegisteredForger {
                username: "late_one".into(),
                registered_at: 10,
                first_block: 60,
                blocks: 2,
            }]
        );
        let stopped = BatchChain {
            tip: None,
            stopped: Some("stopped".into()),
            ..chain
        };
        assert!(
            registered_forgers(&stopped, scratch.path())
                .unwrap()
                .is_empty()
        );
    }
}
