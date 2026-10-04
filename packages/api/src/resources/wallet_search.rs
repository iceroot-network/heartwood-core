//! The wallet entries of the chain search (`GET /blockchain/search/{id}`): the object the
//! reference's `getWalletsLike` builds for each match, keys in its order and a key it leaves
//! `undefined` left out.

use heartwood_state::{Resignation, Wallet};
use serde_json::{Map, Value, json};

use super::defined;

/// The entry of `wallet`: `address`, `delegate` (`rank`, `resigned`, `username`, `voters` and
/// `votes`, the vote balance), `publicKey`, `balance` and `votes` (per delegate voted for).
pub fn wallet_search(wallet: &Wallet) -> Value {
    let delegate = wallet.delegate().map(|delegate| {
        let resigned = delegate.resigned.map(|resigned| match resigned {
            Resignation::Permanent => "permanent",
            Resignation::Temporary => "temporary",
        });
        defined(json!({
            "rank": delegate.rank, "resigned": resigned, "username": delegate.username(),
            "voters": delegate.voters, "votes": delegate.vote_balance.to_string(),
        }))
    });
    let shares = wallet.vote_shares();
    let votes = wallet.votes().iter().map(|(name, points)| {
        let percent = match points % 100 {
            0 => json!(points / 100),
            _ => json!(f64::from(*points) / 100.0),
        };
        let votes = shares.get(name).map(u64::to_string);
        (
            name.clone(),
            defined(json!({ "percent": percent, "votes": votes })),
        )
    });
    defined(json!({
        "address": wallet.address().to_base58(), "delegate": delegate,
        "publicKey": wallet.public_key().map(|key| key.to_compressed().to_hex()),
        "balance": wallet.balance().to_string(), "votes": Value::Object(js_order(votes)),
    }))
}

/// The members of a JavaScript object made from `entries`: the keys that are array indices come
/// first, in ascending order, then the others as they were added.
fn js_order(entries: impl Iterator<Item = (String, Value)>) -> Map<String, Value> {
    let index = |key: &str| {
        let canonical =
            key == "0" || (!key.starts_with('0') && key.bytes().all(|b| b.is_ascii_digit()));
        key.parse::<u32>()
            .ok()
            .filter(|&n| canonical && n != u32::MAX)
    };
    let (mut indexed, others): (Vec<_>, Vec<_>) =
        entries.partition(|(key, _)| index(key).is_some());
    indexed.sort_by_key(|(key, _)| index(key));
    indexed.into_iter().chain(others).collect()
}
