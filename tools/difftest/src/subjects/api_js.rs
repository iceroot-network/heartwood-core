//! The wallet search: `api.walletsLike` (the sorted list of a search) and `js.localeCompare` (the
//! numeric collation of two strings).
//!
//! `api.walletsLike` builds the wallets of `input.fixture` (with the ranks of `input.ranks`, a
//! fixture has none) into the state view of the committed one-node chain, in place of its
//! wallets, and searches it with `input.input`.

use std::cmp::Ordering;
use std::path::Path;
use std::sync::OnceLock;

use heartwood_api::search::{locale_compare as collate, wallets_like as search};
use heartwood_crypto::identities::Address;
use heartwood_state::{State, StateView};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, text};
use crate::corpus::Corpus;
use crate::fixture::Fixture;
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("api.walletsLike", wallets_like),
    ("js.localeCompare", locale_compare),
];

/// The view of the committed chain's genesis state, which a search reads only the wallets of.
fn base_view() -> Outcome<&'static StateView> {
    static VIEW: OnceLock<Result<StateView, String>> = OnceLock::new();
    let view = VIEW.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/txgen-one-node");
        let corpus = Corpus::load(&dir).map_err(|e| e.to_string())?;
        let state = State::from_genesis(&corpus.genesis, &corpus.milestones, &Handlers)
            .map_err(|e| e.to_string())?;
        Ok(state.view())
    });
    view.as_ref()
        .map_err(|e| failed(format!("the base chain: {e}")))
}

fn wallets_like(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let network = env.milestones(case)?.at(1).network();
    let fixture = Fixture::from_json(&case.input["fixture"]).map_err(|e| failed(e.to_string()))?;
    let mut wallets = fixture
        .to_wallets(network)
        .map_err(|e| failed(e.to_string()))?;
    for entry in case.input["ranks"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
    {
        let address = entry[0].as_str().and_then(|a| Address::from_base58(a).ok());
        let rank = entry[1].as_u64().and_then(|r| u32::try_from(r).ok());
        let delegate = address
            .and_then(|a| wallets.wallet_mut(&a))
            .and_then(|wallet| wallet.delegate_mut());
        match (delegate, rank) {
            (Some(delegate), Some(rank)) => delegate.rank = Some(rank),
            _ => return Err(failed("input.ranks names a wallet that is no delegate")),
        }
    }
    let mut view = base_view()?.clone();
    view.wallets = wallets;
    let list = search(&view, text(case.input, "input")?);
    Ok(Verdict::Output(json!({ "wallets": Value::Array(list) })))
}

fn locale_compare(_: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let order = match collate(text(case.input, "a")?, text(case.input, "b")?) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    Ok(Verdict::Output(json!({ "order": order })))
}
