//! The canonical state digest of a wallet fixture, or of the state after a genesis block.

use heartwood_crypto::blocks::factory;
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::utils::hex;
use heartwood_state::State;
use heartwood_state::digest::{Sections, records};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use super::{Env, Outcome, failed, unclassified};
use crate::fixture::Fixture;
use crate::subject::{Case, Verdict};

/// `state.digest`, on one of two inputs:
///
/// - `input.fixture`: the wallets of the fixture, built on the devnet's network. A fixture has no
///   block, no round and no missed slot, so `#B`, `#R` and `#M` are their tag lines, and `#S`
///   issues 0. A fixture whose wallets cannot be built (a vote for a name that no delegate has)
///   is a refusal whose class is not compared.
/// - `input.crypto`: the state after the genesis block of a chain definition (its `network`,
///   `milestones` and `genesisBlock` files, as JSON), as the node builds it at its first start.
///   A genesis block the node refuses to start on is a refusal whose class is not compared.
///
/// The output has the five sections, their hashes and the height digest.
pub(super) fn state_digest(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let sections = match case.input.get("crypto") {
        Some(crypto) => match genesis_sections(crypto)? {
            Ok(sections) => sections,
            Err(verdict) => return Ok(verdict),
        },
        None => {
            let network = env.devnet()?.network.pub_key_hash;
            let fixture = Fixture::from_json(&case.input["fixture"])
                .map_err(|message| failed(format!("input.fixture: {message}")))?;
            match fixture.to_wallets(network) {
                Ok(wallets) => Sections::of_wallets(&wallets),
                Err(message) => return Ok(unclassified(message)),
            }
        }
    };
    let hashes = sections.hashes();
    Ok(Verdict::Output(json!({
        "sections": {
            "B": sections.block,
            "S": sections.supply,
            "R": sections.round,
            "M": sections.missed,
            "W": sections.wallets,
        },
        "hashes": {
            "B": hex::encode(&hashes.block),
            "S": hex::encode(&hashes.supply),
            "R": hex::encode(&hashes.round),
            "M": hex::encode(&hashes.missed),
            "W": hex::encode(&hashes.wallets),
        },
        "sd": hex::encode(&hashes.state_digest()),
    })))
}

/// The sections of the state after the genesis block of the chain definition `crypto`, or the
/// refusal of a definition that does not load or a genesis block the node does not start on.
fn genesis_sections(crypto: &Value) -> Outcome<Result<Sections, Verdict>> {
    let file = |name: &str| match crypto.get(name) {
        Some(value) => Ok(value.to_string()),
        None => Err(failed(format!("input.crypto has no {name}"))),
    };
    let network = match Network::load(&file("network")?) {
        Ok(network) => network,
        Err(error) => return Ok(Err(unclassified(error))),
    };
    let milestones = match Milestones::load(&file("milestones")?, &network) {
        Ok(milestones) => milestones,
        Err(error) => return Ok(Err(unclassified(error))),
    };
    let genesis = match factory::from_json(&crypto["genesisBlock"], &milestones) {
        Ok(genesis) => genesis,
        Err(error) => return Ok(Err(unclassified(error))),
    };
    Ok(
        match State::from_genesis(&genesis, &milestones, &Handlers) {
            Ok(state) => Ok(records(&state, &milestones, &[])),
            Err(error) => Err(unclassified(error)),
        },
    )
}
