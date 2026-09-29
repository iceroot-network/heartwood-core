//! The labels of a recipe and the keys behind them: a genesis delegate by its name
//! (`genesis_1`), a key of the devnet tooling by its role (`genesis-1`, `wallet/3`), and a
//! wallet of the recipe by its label, whose key derives from the seed and `recipe/LABEL`.

use std::collections::HashMap;
use std::sync::Arc;

use heartwood_crypto::KeyPair;
use heartwood_crypto::genesis::DerivedKey;
use heartwood_crypto::genesis::derive::role_key;
use heartwood_crypto::identities::{Address, PublicKeyBytes};

use super::LockstepError;
use super::recipe::Recipe;
use crate::genesis::Built;

/// A wallet the producer can sign for.
#[derive(Debug, Clone)]
pub struct Person {
    /// The passphrase the reference's builders sign with.
    pub passphrase: String,
    /// The address.
    pub address: Address,
}

/// The labels of a chain and their keys.
pub struct Labels {
    people: HashMap<String, Person>,
    forgers: HashMap<PublicKeyBytes, Arc<KeyPair>>,
    delegates: HashMap<String, PublicKeyBytes>,
}

fn person(key: &DerivedKey) -> Person {
    Person {
        passphrase: key.passphrase.to_string(),
        address: key.address,
    }
}

impl Labels {
    /// The labels of the chain `built` derived from `seed`, and of the wallets of `recipe`.
    pub fn new(
        built: &Built,
        seed: &str,
        network: u8,
        recipe: &Recipe,
    ) -> Result<Labels, LockstepError> {
        let definition = &built.definition;
        let mut labels = Labels {
            people: HashMap::new(),
            forgers: HashMap::new(),
            delegates: HashMap::new(),
        };
        let mut add = |label: &str, key: &DerivedKey| -> Result<(), LockstepError> {
            match labels.people.insert(label.to_owned(), person(key)) {
                None => Ok(()),
                Some(_) => Err(LockstepError::Recipe(format!(
                    "the label {label} names two wallets"
                ))),
            }
        };
        for key in std::iter::once(&definition.generator)
            .chain(&definition.allocations)
            .chain(&definition.donations)
            .chain(&definition.wallets)
            .chain(&definition.delegates)
        {
            add(&key.role, key)?;
        }
        for (delegate, key) in built.input.delegates.iter().zip(&definition.delegates) {
            add(&delegate.username, key)?;
            labels.delegates.insert(
                delegate.username.clone(),
                key.keys.public_key().to_compressed(),
            );
            let pair = role_key(&built.profile.derivation_tag, &delegate.role, seed, network)
                .map_err(|e| LockstepError::Heartwood(e.to_string()))?;
            labels
                .forgers
                .insert(pair.keys.public_key().to_compressed(), Arc::new(pair.keys));
        }
        for label in &recipe.wallets {
            let key = role_key(
                &built.profile.derivation_tag,
                &format!("recipe/{label}"),
                seed,
                network,
            )
            .map_err(|e| LockstepError::Heartwood(e.to_string()))?;
            add(label, &key)?;
        }
        Ok(labels)
    }

    /// The wallet a label names.
    pub fn person(&self, label: &str) -> Result<&Person, LockstepError> {
        self.people
            .get(label)
            .ok_or_else(|| LockstepError::Recipe(format!("no wallet is labelled {label}")))
    }

    /// The public key of a genesis delegate by its name.
    pub fn delegate(&self, name: &str) -> Option<PublicKeyBytes> {
        self.delegates.get(name).copied()
    }

    /// The key that forges for the delegate with `public_key`, when the seed derives it.
    pub fn forger(&self, public_key: &PublicKeyBytes) -> Option<Arc<KeyPair>> {
        self.forgers.get(public_key).cloned()
    }
}
