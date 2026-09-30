//! The `proptest` strategies of L1: the inputs of each property, in the domains the reference
//! and Heartwood both accept.
//!
//! Amounts stay inside the domain every implementation can represent: the balances of a fixture
//! add up to at most `2^63 − 1`, as a genesis block can issue at most that much.
//!
//! - this module: heights, wallet fixtures, and the shared keys and names;
//! - [`transactions`]: signed transactions of the six kept types, for the fee floor and the
//!   handlers, and their mutations;
//! - [`kernel`]: rounds, slots, block chaining, JSON values, `.env` text and IP lists;
//! - [`state`]: shuffles, rankings and vote maps;
//! - [`pool`]: pool sessions: fixtures, options and steps;
//! - [`text`]: amounts, counts, symbols and transactions for the message formats;
//! - [`config`]: mutations of the devnet's crypto files;
//! - [`verifier`], [`exchange`] and [`monitor`]: the peer network's verifier, requests and
//!   replies, and monitor decisions;
//! - [`processor`]: the next block of a corpus prefix, mutated, for the block processor;
//! - [`forging`] and [`forger_decide`]: forging from a pool session, and the forger's decisions;
//! - [`quorum`]: the network state a relay gives its forger.
//! - [`served`]: requests of the public routes over a chain, a pool and a peer repository, for the
//!   served answers.
//! - [`api_state`] and [`api_page`]: the listings over the state and their pagination (P21);
//!   [`api_js`]: the wallet search (P22); [`api_query`]: the API's query grammar (P23).

pub mod api_js;
pub mod api_page;
pub mod api_query;
pub mod api_state;
pub mod config;
pub mod exchange;
pub mod forger_decide;
pub mod forging;
pub mod kernel;
pub mod monitor;
pub mod pool;
pub mod processor;
pub mod quorum;
pub mod served;
pub mod state;
pub mod text;
pub mod transactions;
pub mod verifier;

use heartwood_crypto::BlockId;
use heartwood_crypto::identities::{Address, KeyPair, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::types::Votes;
use heartwood_state::wallets::Resignation;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use crate::fixture::{Fixture, FixtureDelegate, FixtureWallet};

/// The address byte of the devnet.
pub const NETWORK: u8 = 90;
/// The most a fixture's balances add up to.
pub const MAX_SUPPLY: u64 = i64::MAX as u64;

/// The key of the passphrase `text`, one of the harness's fixed passphrases.
pub fn keys(text: &str) -> KeyPair {
    KeyPair::from_passphrase(text).expect("the harness's passphrases have keys")
}

/// The key of fixture wallet `index` (a fixed passphrase per index, so shrinking keeps keys).
pub fn wallet_keys(index: usize) -> KeyPair {
    keys(&format!("l1 wallet {index}"))
}

/// The key of the second signature of fixture wallet `index`.
pub fn second_keys(index: usize) -> KeyPair {
    keys(&format!("l1 second {index}"))
}

/// The devnet address of `key`.
pub fn address_of(key: &PublicKey) -> Address {
    Address::from_public_key(key, NETWORK)
}

/// The username of fixture delegate `index`.
pub fn delegate_name(index: usize) -> String {
    format!("l1d{index}")
}

/// Heights from 1 to `2^32 − 1`, the edges of a round of `active_delegates` more often.
pub fn heights(active_delegates: u64) -> BoxedStrategy<u32> {
    let n = active_delegates.clamp(1, u64::from(u32::MAX));
    let near_round =
        (0u64..(u64::from(u32::MAX) / n), -2i64..=2).prop_map(move |(round, offset)| {
            let base = i64::try_from(round.saturating_mul(n)).unwrap_or(0);
            u32::try_from(base.saturating_add(offset).clamp(1, i64::from(u32::MAX))).unwrap_or(1)
        });
    prop_oneof![
        3 => 1u32..=u32::MAX,
        3 => near_round,
        2 => 1u32..=10_000,
        1 => (u32::MAX - 200)..=u32::MAX,
    ]
    .boxed()
}

/// What a fixture wallet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Plain,
    Keyed,
    Delegate,
    Resigned(Resignation),
    Revoked,
}

fn role() -> impl Strategy<Value = Role> {
    prop_oneof![
        2 => Just(Role::Plain),
        4 => Just(Role::Keyed),
        4 => Just(Role::Delegate),
        1 => Just(Role::Resigned(Resignation::Temporary)),
        1 => Just(Role::Resigned(Resignation::Permanent)),
        1 => Just(Role::Revoked),
    ]
}

/// A balance: small, round, near a limit, or anything.
fn balance() -> impl Strategy<Value = u64> {
    prop_oneof![
        2 => 0u64..=1_000,
        3 => 0u64..=100_000_000_000_000,
        1 => Just(0u64),
        1 => (MAX_SUPPLY - 1_000_000)..=MAX_SUPPLY,
        2 => any::<u64>().prop_map(|b| b % (MAX_SUPPLY / 16)),
    ]
}

/// Basis points that add up to 10,000 over `n` entries (n from 1).
pub fn split(n: usize) -> BoxedStrategy<Vec<u16>> {
    if n <= 1 {
        return Just(vec![10_000]).boxed();
    }
    prop::collection::vec(1u32..=1_000, n)
        .prop_map(move |weights| {
            let total: u32 = weights.iter().sum();
            let mut out: Vec<u16> = weights
                .iter()
                .map(|w| u16::try_from((w * 10_000 / total).max(1)).unwrap_or(1))
                .collect();
            let sum: i32 = out.iter().map(|b| i32::from(*b)).sum();
            let mut diff = 10_000 - sum;
            let mut i = 0;
            while diff != 0 && i < out.len() * 20_000 {
                let slot = i % out.len();
                if diff > 0 {
                    out[slot] += 1;
                    diff -= 1;
                } else if out[slot] > 1 {
                    out[slot] -= 1;
                    diff += 1;
                }
                i += 1;
            }
            out
        })
        .boxed()
}

/// Basis points split as evenly as possible over `n` entries (n from 1), the first entries taking
/// the remainder.
pub fn split_now(n: usize) -> Vec<u16> {
    let n = n.max(1);
    let count = u16::try_from(n).unwrap_or(u16::MAX);
    let base = 10_000 / count;
    let extra = usize::from(10_000 - base * count);
    (0..n).map(|i| base + u16::from(i < extra)).collect()
}

#[derive(Debug, Clone)]
struct WalletDraft {
    role: Role,
    balance: u64,
    nonce: u64,
    second: bool,
    produced: u64,
    counters: [u64; 4],
    last_block: Option<[u8; 32]>,
    resigned_back: u32,
    votes: Vec<(usize, u16)>,
}

fn wallet_draft() -> impl Strategy<Value = WalletDraft> {
    (
        role(),
        balance(),
        prop_oneof![3 => 0u64..=10, 1 => 0u64..=1_000_000, 1 => Just(u64::MAX - 1)],
        prop::bool::weighted(0.2),
        0u64..=1_000,
        [const { 0u64..=1_000_000_000_000 }; 4],
        prop::option::weighted(0.5, any::<[u8; 32]>()),
        1u32..=300,
    )
        .prop_map(
            |(role, balance, nonce, second, produced, counters, last_block, resigned_back)| {
                WalletDraft {
                    role,
                    balance,
                    nonce,
                    second,
                    produced,
                    counters,
                    last_block,
                    resigned_back,
                    votes: Vec::new(),
                }
            },
        )
}

/// The fixture of `drafts`, with votes for its delegates, resignations `resigned_back` blocks
/// before `last_block_height`, and balances scaled down to add up to at most `2^63 − 1`.
fn assemble(
    mut drafts: Vec<WalletDraft>,
    vote_plan: Vec<(bool, Vec<u16>)>,
    last_block_height: u32,
) -> Fixture {
    let delegates: Vec<usize> = drafts
        .iter()
        .enumerate()
        .filter(|(_, d)| matches!(d.role, Role::Delegate | Role::Resigned(_) | Role::Revoked))
        .map(|(i, _)| i)
        .collect();
    let total: u128 = drafts.iter().map(|d| u128::from(d.balance)).sum();
    if total > u128::from(MAX_SUPPLY) {
        let scale = total / u128::from(MAX_SUPPLY) + 1;
        for d in &mut drafts {
            d.balance = u64::try_from(u128::from(d.balance) / scale).unwrap_or(0);
        }
    }
    for (i, (votes, shares)) in vote_plan.into_iter().enumerate() {
        let Some(d) = drafts.get_mut(i) else { break };
        if !votes || delegates.is_empty() || matches!(d.role, Role::Plain) {
            continue;
        }
        let n = shares.len().min(delegates.len());
        let mut shares = shares;
        shares.truncate(n);
        // Keep the sum at 10,000 after truncation: the last entry takes the rest.
        let sum: u32 = shares.iter().map(|s| u32::from(*s)).sum();
        if let Some(last) = shares.last_mut() {
            *last = u16::try_from(u32::from(*last) + 10_000 - sum.min(10_000)).unwrap_or(*last);
        }
        let start = i % delegates.len();
        d.votes = (0..n)
            .map(|k| (delegates[(start + k) % delegates.len()], shares[k]))
            .collect();
    }
    let wallets = drafts
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let keys = wallet_keys(i);
            let keyed = !matches!(d.role, Role::Plain);
            let delegate = match d.role {
                Role::Delegate | Role::Resigned(_) | Role::Revoked => Some(FixtureDelegate {
                    username: delegate_name(i),
                    resigned: match d.role {
                        Role::Resigned(r) => Some(r),
                        _ => None,
                    },
                    resigned_at: match d.role {
                        Role::Resigned(_) | Role::Revoked => {
                            Some(last_block_height.saturating_sub(d.resigned_back).max(1))
                        }
                        _ => None,
                    },
                    produced_blocks: d.produced,
                    forged_fees: d.counters[0],
                    forged_rewards: d.counters[1],
                    burned_fees: d.counters[2],
                    donations: d.counters[3],
                    last_block: d.last_block.map(BlockId::from_array),
                }),
                _ => None,
            };
            let mut votes = Votes::new();
            for (target, bps) in &d.votes {
                votes.insert(delegate_name(*target), *bps);
            }
            FixtureWallet {
                address: address_of(keys.public_key()),
                public_key: keyed.then(|| keys.public_key().clone()),
                balance: d.balance,
                nonce: if keyed { d.nonce.max(1) } else { d.nonce },
                second_public_key: (keyed && d.second)
                    .then(|| second_keys(i).public_key().to_compressed()),
                delegate,
                votes: if keyed { votes } else { Votes::new() },
                second_key_first: false,
            }
        })
        .collect();
    Fixture { wallets }
}

/// Wallet fixtures of 3 to 12 wallets whose resignations lie before `last_block_height`:
/// delegates, resigned delegates with their resignation height, voters and second keys, with
/// balances that add up to at most `2^63 − 1`.
pub fn fixtures_at(last_block_height: u32) -> BoxedStrategy<Fixture> {
    prop::collection::vec(wallet_draft(), 3..=12)
        .prop_flat_map(move |drafts| {
            let n = drafts.len();
            let plan = prop::collection::vec(
                (prop::bool::weighted(0.4), (1usize..=4).prop_flat_map(split)),
                n,
            );
            (Just(drafts), plan)
        })
        .prop_map(move |(drafts, plan)| assemble(drafts, plan, last_block_height))
        .boxed()
}

/// Wallet fixtures of 3 to 12 wallets: delegates, resigned delegates with their resignation
/// height, voters and second keys, with balances that add up to at most `2^63 − 1`.
pub fn fixtures() -> BoxedStrategy<Fixture> {
    fixtures_at(1_000_000)
}

/// A second-key value as a fixture holds it.
pub fn second_key_bytes(index: usize) -> PublicKeyBytes {
    second_keys(index).public_key().to_compressed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    use super::*;

    #[test]
    fn heights_stay_in_range_and_reach_round_edges() {
        let mut runner = TestRunner::deterministic();
        let strategy = heights(53);
        let mut edges = 0;
        for _ in 0..2_000 {
            let h = strategy.new_tree(&mut runner).unwrap().current();
            assert!(h >= 1);
            if h % 53 <= 1 || h % 53 >= 51 {
                edges += 1;
            }
        }
        assert!(edges > 200, "{edges}");
    }

    #[test]
    fn splits_add_up_to_ten_thousand() {
        let mut runner = TestRunner::deterministic();
        for n in 1..=53 {
            let v = split(n).new_tree(&mut runner).unwrap().current();
            assert_eq!(v.len(), n);
            assert_eq!(v.iter().map(|b| u32::from(*b)).sum::<u32>(), 10_000);
            assert!(v.iter().all(|b| *b >= 1));
        }
    }

    #[test]
    fn fixtures_are_in_the_shared_domain() {
        let mut runner = TestRunner::deterministic();
        let strategy = fixtures_at(5_000);
        for _ in 0..300 {
            let f = strategy.new_tree(&mut runner).unwrap().current();
            assert!((3..=12).contains(&f.wallets.len()));
            assert!(f.total_balance().is_some_and(|t| t <= MAX_SUPPLY));
            let json = f.to_json();
            assert_eq!(Fixture::from_json(&json).unwrap(), f);
            for w in &f.wallets {
                if !w.votes.is_empty() {
                    assert!(w.public_key.is_some());
                    assert_eq!(w.votes.values().map(|b| u32::from(*b)).sum::<u32>(), 10_000);
                    for name in w.votes.keys() {
                        assert!(
                            f.wallets
                                .iter()
                                .any(|d| d.delegate.as_ref().is_some_and(|d| &d.username == name))
                        );
                    }
                }
                if let Some(d) = &w.delegate {
                    assert!(d.resigned_at.is_none_or(|h| h <= 5_000));
                    assert!(w.public_key.is_some());
                }
            }
        }
    }
}
