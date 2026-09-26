//! Wallets, ranking and rounds: the forging shuffle, the delegate ranking and a voter's shares.

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::transactions::types::Votes;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Address, PublicKey};
use heartwood_kernel::RoundInfo;
use heartwood_state::dpos::{build_delegate_ranking, set_delegates_round};
use heartwood_state::errors::RoundError;
use heartwood_state::round_state::shuffle_delegates;
use heartwood_state::wallets::vote_balances::calculate_vote_amounts;
use heartwood_state::{Resignation, WalletRepository};
use serde_json::{Value, json};

use super::{Env, Outcome, amount, failed, integer, text, unclassified};
use crate::subject::{Case, Verdict};

/// The largest delegate list and round range the oracle's operations take.
const MAX_DELEGATES: u64 = 100_000;
const MAX_ROUNDS: u64 = 100_000;
/// The largest integer a JavaScript number holds exactly.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// A count of the input, refused as the oracle refuses it when it is over `max`.
fn count(input: &Value, key: &str, max: u64) -> Outcome<Result<u64, Verdict>> {
    let value: u64 = integer(input, key)?;
    Ok(if value > max {
        Err(unclassified(format!("{key} is over {max}")))
    } else {
        Ok(value)
    })
}

/// The input indices of `count` delegates in the forging order of `round`.
fn order(round: u64, count: u64) -> Outcome<Vec<u64>> {
    let indices: Vec<u64> = (0..count).collect();
    Ok(shuffle_delegates(round, &indices))
}

/// `state.shuffle`: the forging order of a round, or the SHA-256 of the orders of a range of
/// rounds as text (one line per round, the indices separated by commas).
pub(super) fn shuffle(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let n = match count(input, "count", MAX_DELEGATES)? {
        Ok(n) => n,
        Err(refused) => return Ok(refused),
    };
    if input.get("round").is_some() {
        let round = match count(input, "round", MAX_SAFE_INTEGER)? {
            Ok(round) => round,
            Err(refused) => return Ok(refused),
        };
        return Ok(Verdict::Output(json!({ "order": order(round, n)? })));
    }
    let (from, to) = match (
        count(input, "from", MAX_SAFE_INTEGER)?,
        count(input, "to", MAX_SAFE_INTEGER)?,
    ) {
        (Ok(from), Ok(to)) if from <= to && to - from < MAX_ROUNDS => (from, to),
        _ => return Ok(unclassified("from and to are not a range the oracle takes")),
    };
    let mut text = String::new();
    for round in from..=to {
        let line: Vec<String> = order(round, n)?.iter().map(u64::to_string).collect();
        text.push_str(&line.join(","));
        text.push('\n');
    }
    Ok(Verdict::Output(json!({
        "rounds": to - from + 1,
        "sha256": hex::encode(&sha256(text.as_bytes())),
    })))
}

/// The reference's error class for a refused ranking or round.
fn round_class(error: &RoundError) -> Option<&'static str> {
    match error {
        RoundError::IdenticalPublicKeys { .. } | RoundError::NotEnoughDelegates { .. } => {
            Some("Error")
        }
        RoundError::NoPublicKey { .. } => Some("AssertionException"),
        _ => None,
    }
}

/// `state.rank`: the ranks and the round's delegates of a set of delegates.
pub(super) fn rank(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let delegates = input["delegates"]
        .as_array()
        .ok_or_else(|| failed("input.delegates is not a list"))?;
    let max_delegates = match input.get("maxDelegates") {
        None | Some(Value::Null) => env.milestones(case)?.at(1).active_delegates(),
        Some(_) => match count(input, "maxDelegates", MAX_DELEGATES)? {
            Ok(max) => max,
            Err(refused) => return Ok(refused),
        },
    };
    if u64::try_from(delegates.len()).map_or(true, |n| n > MAX_DELEGATES) {
        return Ok(unclassified("too many delegates"));
    }

    // Each delegate gets its own wallet, at an address made from its position, so that two
    // delegates can hold the same key.
    let mut wallets = WalletRepository::new(0);
    let mut names = Vec::with_capacity(delegates.len());
    for (index, delegate) in delegates.iter().enumerate() {
        let mut bytes = [0_u8; 21];
        bytes[13..].copy_from_slice(&u64::try_from(index).unwrap_or(u64::MAX).to_be_bytes());
        let address = Address::from_bytes(bytes);
        let username = text(delegate, "username")?.to_owned();
        let public_key = PublicKey::from_hex(text(delegate, "publicKey")?)
            .map_err(|e| failed(format!("delegates[{index}].publicKey: {e}")))?;
        let vote_balance = amount(delegate, "voteBalance")?;
        let resigned = match delegate.get("resigned").and_then(Value::as_u64) {
            None => None,
            Some(0) => Some(Resignation::Temporary),
            Some(1) => Some(Resignation::Permanent),
            Some(other) => return Err(failed(format!("delegates[{index}].resigned is {other}"))),
        };
        wallets.bind_public_key(&address, public_key);
        let registered = wallets
            .register_delegate(&address, username.clone())
            .map_err(|e| failed(format!("delegates[{index}]: {e}")))?;
        registered.vote_balance = vote_balance;
        registered.resigned = resigned;
        names.push((username, address));
    }

    let ranked = match build_delegate_ranking(&mut wallets) {
        Ok(ranked) => ranked,
        Err(error) => {
            return Ok(Verdict::Rejected {
                class: round_class(&error),
                detail: error.to_string(),
            });
        }
    };
    let ranks: Vec<Value> = names
        .iter()
        .map(|(username, address)| {
            let rank = wallets
                .find_by_address(address)
                .and_then(|wallet| wallet.delegate())
                .and_then(|delegate| delegate.rank);
            json!([username, rank])
        })
        .collect();
    let info = RoundInfo {
        round: 1,
        round_height: 1,
        next_round: 1,
        max_delegates,
    };
    let round = match set_delegates_round(&wallets, &ranked, &info) {
        Ok(round) => json!(round.iter().map(|d| &d.username).collect::<Vec<_>>()),
        Err(error) => match round_class(&error) {
            Some(class) => json!({"error": {"class": class, "message": error.to_string()}}),
            None => {
                return Ok(Verdict::Rejected {
                    class: None,
                    detail: error.to_string(),
                });
            }
        },
    };
    Ok(Verdict::Output(json!({"ranks": ranks, "round": round})))
}

/// `wallet.voteAmounts`: each delegate's share of a voter's balance.
pub(super) fn vote_amounts(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let balance = amount(input, "balance")?;
    let entries = input["votes"]
        .as_array()
        .ok_or_else(|| failed("input.votes is not a list"))?;
    let mut votes = Votes::new();
    for entry in entries {
        let (Some(name), Some(basis_points)) = (entry[0].as_str(), entry[1].as_u64()) else {
            return Err(failed("a vote is not [name, basis points]"));
        };
        let Some(basis_points) = u16::try_from(basis_points).ok().filter(|&b| b <= 10_000) else {
            return Ok(unclassified("basis points over 10,000"));
        };
        votes.insert(name.to_owned(), basis_points);
    }
    let amounts: Vec<Value> = calculate_vote_amounts(balance, &votes)
        .into_iter()
        .map(|(name, share)| json!([name, share.to_string()]))
        .collect();
    Ok(Verdict::Output(json!({ "amounts": amounts })))
}
