//! The wallet part of `GET /blockchain/search/{id}`: the reference's `getWalletsLike`. An input of
//! up to 20 characters is a username prefix, a longer one an address or a public key prefix; the
//! first 100 matches in the address index's order, sorted by the reference's comparator with
//! [`super::locale_compare()`] and Node 18's sort ([`super::v8_sort()`]).

use heartwood_state::StateView;
use serde_json::Value;

/// The wallets of `state` that the lower-cased input `criteria` starts, as the reference lists
/// them.
pub fn wallets_like(state: &StateView, criteria: &str) -> Vec<Value> {
    let _ = (state, criteria);
    todo!("the wallet search")
}
