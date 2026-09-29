//! The wallet part of `GET /blockchain/search/{id}`: the reference's `getWalletsLike`. An input of
//! up to 20 characters is a username prefix, a longer one an address or a public key prefix; the
//! first 100 matches in the address index's order, sorted by the reference's comparator with
//! [`super::locale_compare()`] and Node 18's sort ([`super::v8_sort()`]).

use std::cmp::Ordering;

use heartwood_state::{StateView, Wallet};
use serde_json::Value;

use super::{locale_compare, v8_sort};
use crate::resources::wallet_search::wallet_search;

/// A match with what the comparator reads.
struct Entry {
    value: Value,
    address: String,
    /// `None` for a wallet that is no delegate; else whether it has resigned, and its username.
    delegate: Option<(bool, String)>,
}

/// The wallets of `state` that the lower-cased input `criteria` starts, as the reference lists
/// them.
pub fn wallets_like(state: &StateView, criteria: &str) -> Vec<Value> {
    let criteria = criteria.to_lowercase();
    let username_search = criteria.encode_utf16().count() <= 20;
    let matches = |wallet: &&Wallet| {
        if username_search {
            wallet
                .delegate()
                .is_some_and(|d| !d.username().is_empty() && d.username().starts_with(&criteria))
        } else {
            let base58 = wallet.address().to_base58().to_lowercase();
            let key = wallet.public_key().map(|key| key.to_compressed().to_hex());
            base58.starts_with(&criteria) || key.is_some_and(|key| key.starts_with(&criteria))
        }
    };
    let mut entries: Vec<Entry> = (state.wallets.by_address_index().filter(matches).take(100))
        .map(|wallet| Entry {
            value: wallet_search(wallet),
            address: wallet.address().to_base58(),
            delegate: (wallet.delegate()).map(|d| (d.resigned.is_some(), d.username().to_owned())),
        })
        .collect();
    v8_sort(&mut entries, compare);
    entries.into_iter().map(|entry| entry.value).collect()
}

/// The reference's comparator: a delegate that has not resigned before a wallet that is no
/// delegate, two such delegates by name, every other pair by address. It is not a total order.
fn compare(a: &Entry, b: &Entry) -> Ordering {
    match (&a.delegate, &b.delegate) {
        (Some((false, _)), None) => Ordering::Less,
        (None, Some((false, _))) => Ordering::Greater,
        (Some((false, x)), Some((false, y))) => locale_compare(x, y),
        _ => locale_compare(&a.address, &b.address),
    }
}
