//! The chain a scenario runs on and the wallets it names: the genesis profile of its network,
//! built from its genesis seed, the four crypto files the reference and the lockstep producer are
//! given, and the wallets by name. A name is an actor of the scenario (a key derived from the seed
//! and the actor's own seed), `genesis` (the first funded allocation), or a label of the genesis
//! (a delegate's username such as `genesis_5`, or a key's role such as `wallet/3`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use heartwood_crypto::genesis::derive::role_key;
use serde_json::{Map, Value};

use super::format::Scenario;
use crate::errors::{Error, Result};
use crate::genesis::{Built, normalise_epoch};

/// The chain's epoch, as the lockstep producer's chains have it.
const EPOCH: &str = "2026-01-01T00:00:00Z";

/// The label the `genesis` wallet has.
const GENESIS_LABEL: &str = "genesis-1";

/// A wallet a scenario can sign for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The passphrase the reference's builders sign with.
    pub passphrase: String,
    /// The address.
    pub address: String,
}

/// The chain of a scenario.
#[derive(Debug)]
pub struct Chain {
    /// The crypto files (`network`, `milestones`, `genesisBlock`, `exceptions`) as JSON.
    pub crypto: Value,
    /// The profile's name, as the lockstep producer resolves it.
    pub profile: String,
    /// The blocks of a round.
    pub round_length: u32,
    /// The seconds between two slots.
    pub block_time: u64,
    /// The supply the genesis issues, in satoshi.
    pub supply: String,
    /// The delegates' usernames, in genesis order.
    pub delegates: Vec<String>,
    people: HashMap<String, Person>,
    labels: HashMap<String, String>,
}

/// The profile file the network `network` names: a profile of the CLI by that name, or without
/// its `-N` node count (`devnet-5` is the `devnet` profile).
fn profile_of(network: &str) -> Result<(String, PathBuf)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/cli/src/profiles");
    let bare = network
        .rsplit_once('-')
        .filter(|(_, count)| !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit()))
        .map_or(network, |(name, _)| name);
    [network, bare]
        .into_iter()
        .map(|name| (name.to_owned(), dir.join(format!("{name}.json"))))
        .find(|(_, path)| path.is_file())
        .ok_or_else(|| Error::Usage(format!("no genesis profile for the network {network}")))
}

impl Chain {
    /// The chain of `scenario`.
    pub fn new(scenario: &Scenario) -> Result<Chain> {
        let (profile, path) = profile_of(&scenario.network)?;
        let seed = scenario.genesis_seed.as_str();
        let built = Built::new(&path, seed, &normalise_epoch(EPOCH))?;
        let definition = &built.definition;
        let mut crypto = Map::new();
        for (name, text) in [
            ("network", &definition.network_json),
            ("milestones", &definition.milestones_json),
            ("genesisBlock", &definition.genesis_block_json),
            ("exceptions", &definition.exceptions_json),
        ] {
            let value = serde_json::from_str(text)
                .map_err(|e| Error::Fixture(format!("the {name} file: {e}")))?;
            crypto.insert(name.to_owned(), value);
        }
        let mut people = HashMap::new();
        for key in std::iter::once(&definition.generator)
            .chain(&definition.allocations)
            .chain(&definition.donations)
            .chain(&definition.wallets)
            .chain(&definition.delegates)
        {
            people.insert(key.role.clone(), person(&key.passphrase, &key.address));
        }
        let mut delegates = Vec::new();
        for (delegate, key) in built.input.delegates.iter().zip(&definition.delegates) {
            people.insert(
                delegate.username.clone(),
                person(&key.passphrase, &key.address),
            );
            delegates.push(delegate.username.clone());
        }
        let mut labels = HashMap::new();
        for (name, actor) in &scenario.actors {
            let label = format!("{name}-{}", actor.seed);
            let key = role_key(
                &built.profile.derivation_tag,
                &format!("recipe/{label}"),
                seed,
                built.profile.network.pub_key_hash,
            )?;
            people.insert(name.clone(), person(&key.passphrase, &key.address));
            labels.insert(name.clone(), label);
        }
        if let Some(genesis) = people.get(GENESIS_LABEL).cloned() {
            people.entry("genesis".to_owned()).or_insert(genesis);
        }
        let number = |key: &str| {
            built
                .profile
                .rules
                .get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::Fixture(format!("the profile's rules have no {key}")))
        };
        let round_length = u32::try_from(number("activeDelegates")?)
            .map_err(|e| Error::Fixture(format!("activeDelegates: {e}")))?;
        Ok(Chain {
            crypto: Value::Object(crypto),
            profile,
            round_length,
            block_time: number("blockTime")?,
            supply: built.profile.supply.clone(),
            delegates,
            people,
            labels,
        })
    }

    /// The wallet `name` names.
    pub fn person(&self, name: &str) -> Result<&Person> {
        self.people.get(name).ok_or_else(|| {
            Error::Usage(format!(
                "no wallet is named {name}: an actor, genesis, a delegate or a role"
            ))
        })
    }

    /// The label the lockstep producer knows the wallet `name` by.
    pub fn label(&self, name: &str) -> String {
        match self.labels.get(name) {
            Some(label) => label.clone(),
            None if name == "genesis" => GENESIS_LABEL.to_owned(),
            None => name.to_owned(),
        }
    }

    /// The labels of the wallets the lockstep producer derives for the scenario's actors.
    pub fn actor_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.labels.values().cloned().collect();
        labels.sort();
        labels
    }
}

fn person(passphrase: &str, address: &impl ToString) -> Person {
    Person {
        passphrase: passphrase.to_owned(),
        address: address.to_string(),
    }
}
