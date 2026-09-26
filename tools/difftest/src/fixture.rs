//! Wallet fixtures: the input of the state operations of L1 (`state.applyTransaction`,
//! `state.digest`) and of the `state_apply` fuzz target.
//!
//! A fixture is a list of wallets in the shape of the digest's `#W` records, without the values
//! both sides derive (`vb`, `vc` and `rk`: vote balances and voter counts come from the voters,
//! and a fixture has no ranks), plus `resignedAt`, the height of a resigned delegate's latest
//! resignation. The oracle builds the reference's wallets and history from the same JSON.
//!
//! The JSON is an array of wallet objects, each with the keys `a` (address), `pk` (public key,
//! optional), `bal` and `n` (decimal strings), `spk` (second public key, optional), and for a
//! delegate `u` (username), `rs` (0 temporary, 1 permanent, optional), `resignedAt` (optional),
//! `pb` (an integer), `ff`, `fr`, `bf`, `dn` (decimal strings) and `lb` (a block id, optional);
//! `v` (optional) is `[[username, basisPoints], ...]`. A voter needs a public key (the reference
//! builds vote balances from the wallets it knows by key), and it may vote only for the fixture's
//! delegates.

use heartwood_crypto::BlockId;
use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::types::Votes;
use heartwood_state::wallets::vote_balances::{increase_vote_balances, update_vote_shares};
use heartwood_state::wallets::{Resignation, WalletRepository};
use serde_json::{Map, Value, json};

/// A wallet fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    /// The wallets, in the order they are created.
    pub wallets: Vec<FixtureWallet>,
}

/// One wallet of a fixture (`a`, `pk`, `bal`, `n`, `spk`, and the delegate and vote keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureWallet {
    /// `a`.
    pub address: Address,
    /// `pk`.
    pub public_key: Option<PublicKey>,
    /// `bal`.
    pub balance: u64,
    /// `n`.
    pub nonce: u64,
    /// `spk`.
    pub second_public_key: Option<PublicKeyBytes>,
    /// The delegate keys, when `u` is present.
    pub delegate: Option<FixtureDelegate>,
    /// `v`, in the order they are kept.
    pub votes: Votes,
}

/// The delegate record of a fixture wallet (`u`, `rs`, `pb`, `ff`, `fr`, `bf`, `dn`, `lb`, and
/// `resignedAt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureDelegate {
    /// `u`.
    pub username: String,
    /// `rs`.
    pub resigned: Option<Resignation>,
    /// `resignedAt`.
    pub resigned_at: Option<u32>,
    /// `pb`.
    pub produced_blocks: u64,
    /// `ff`.
    pub forged_fees: u64,
    /// `fr`.
    pub forged_rewards: u64,
    /// `bf`.
    pub burned_fees: u64,
    /// `dn`.
    pub donations: u64,
    /// `lb`.
    pub last_block: Option<BlockId>,
}

fn amount(object: &Map<String, Value>, key: &str, at: usize) -> Result<u64, String> {
    match object.get(key) {
        None => Ok(0),
        Some(Value::String(text))
            if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) =>
        {
            text.parse()
                .map_err(|_| format!("wallet {at}: {key} is not a u64"))
        }
        Some(_) => Err(format!("wallet {at}: {key} is not a decimal string")),
    }
}

fn integer(object: &Map<String, Value>, key: &str, at: usize) -> Result<Option<u64>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("wallet {at}: {key} is not an integer")),
    }
}

fn text<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    at: usize,
) -> Result<Option<&'a str>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("wallet {at}: {key} is not a string")),
    }
}

const KEYS: [&str; 15] = [
    "a",
    "pk",
    "bal",
    "n",
    "spk",
    "u",
    "rs",
    "resignedAt",
    "pb",
    "ff",
    "fr",
    "bf",
    "dn",
    "lb",
    "v",
];

impl Fixture {
    /// The fixture in `value`, the JSON the oracle reads; a message when it is malformed.
    pub fn from_json(value: &Value) -> Result<Fixture, String> {
        let list = value.as_array().ok_or("a fixture is an array of wallets")?;
        let mut wallets = Vec::with_capacity(list.len());
        for (at, item) in list.iter().enumerate() {
            let object = item
                .as_object()
                .ok_or_else(|| format!("wallet {at} is not an object"))?;
            if let Some(key) = object.keys().find(|k| !KEYS.contains(&k.as_str())) {
                return Err(format!("wallet {at}: unknown key {key}"));
            }
            let address = text(object, "a", at)?
                .ok_or_else(|| format!("wallet {at} has no address"))
                .and_then(|a| {
                    Address::from_base58(a).map_err(|e| format!("wallet {at}: address {a}: {e}"))
                })?;
            let public_key = text(object, "pk", at)?
                .map(|pk| PublicKey::from_hex(pk).map_err(|e| format!("wallet {at}: pk: {e}")))
                .transpose()?;
            let second_public_key = text(object, "spk", at)?
                .map(|k| PublicKeyBytes::from_hex(k).map_err(|e| format!("wallet {at}: spk: {e}")))
                .transpose()?;
            let delegate = match text(object, "u", at)? {
                None => {
                    if let Some(key) = ["rs", "resignedAt", "pb", "ff", "fr", "bf", "dn", "lb"]
                        .into_iter()
                        .find(|k| object.contains_key(*k))
                    {
                        return Err(format!("wallet {at}: {key} without u"));
                    }
                    None
                }
                Some(username) => Some(FixtureDelegate {
                    username: username.to_owned(),
                    resigned: match integer(object, "rs", at)? {
                        None => None,
                        Some(0) => Some(Resignation::Temporary),
                        Some(1) => Some(Resignation::Permanent),
                        Some(other) => return Err(format!("wallet {at}: rs {other}")),
                    },
                    resigned_at: integer(object, "resignedAt", at)?
                        .map(|h| u32::try_from(h).map_err(|_| format!("wallet {at}: resignedAt")))
                        .transpose()?,
                    produced_blocks: integer(object, "pb", at)?.unwrap_or(0),
                    forged_fees: amount(object, "ff", at)?,
                    forged_rewards: amount(object, "fr", at)?,
                    burned_fees: amount(object, "bf", at)?,
                    donations: amount(object, "dn", at)?,
                    last_block: text(object, "lb", at)?
                        .map(|id| {
                            BlockId::from_hex(id).map_err(|e| format!("wallet {at}: lb: {e}"))
                        })
                        .transpose()?,
                }),
            };
            let mut votes = Votes::new();
            if let Some(v) = object.get("v") {
                for entry in v
                    .as_array()
                    .ok_or_else(|| format!("wallet {at}: v is not an array"))?
                {
                    let (Some(name), Some(bps)) = (
                        entry.get(0).and_then(Value::as_str),
                        entry
                            .get(1)
                            .and_then(Value::as_u64)
                            .and_then(|b| u16::try_from(b).ok()),
                    ) else {
                        return Err(format!(
                            "wallet {at}: a vote is not [username, basisPoints]"
                        ));
                    };
                    if votes.insert(name.to_owned(), bps).is_some() {
                        return Err(format!("wallet {at}: two votes for {name}"));
                    }
                }
            }
            wallets.push(FixtureWallet {
                address,
                public_key,
                balance: amount(object, "bal", at)?,
                nonce: amount(object, "n", at)?,
                second_public_key,
                delegate,
                votes,
            });
        }
        Ok(Fixture { wallets })
    }

    /// The fixture as the JSON the oracle reads.
    pub fn to_json(&self) -> Value {
        Value::Array(
            self.wallets
                .iter()
                .map(|w| {
                    let mut o = Map::new();
                    o.insert("a".into(), json!(w.address.to_base58()));
                    if let Some(pk) = &w.public_key {
                        o.insert("pk".into(), json!(pk.to_hex()));
                    }
                    o.insert("bal".into(), json!(w.balance.to_string()));
                    o.insert("n".into(), json!(w.nonce.to_string()));
                    if let Some(spk) = &w.second_public_key {
                        o.insert("spk".into(), json!(spk.to_hex()));
                    }
                    if let Some(d) = &w.delegate {
                        o.insert("u".into(), json!(d.username));
                        if let Some(r) = d.resigned {
                            o.insert("rs".into(), json!(r.number()));
                        }
                        if let Some(h) = d.resigned_at {
                            o.insert("resignedAt".into(), json!(h));
                        }
                        o.insert("pb".into(), json!(d.produced_blocks));
                        o.insert("ff".into(), json!(d.forged_fees.to_string()));
                        o.insert("fr".into(), json!(d.forged_rewards.to_string()));
                        o.insert("bf".into(), json!(d.burned_fees.to_string()));
                        o.insert("dn".into(), json!(d.donations.to_string()));
                        if let Some(lb) = &d.last_block {
                            o.insert("lb".into(), json!(lb.to_hex()));
                        }
                    }
                    if !w.votes.is_empty() {
                        let v: Vec<Value> = w.votes.iter().map(|(n, b)| json!([n, b])).collect();
                        o.insert("v".into(), Value::Array(v));
                    }
                    Value::Object(o)
                })
                .collect(),
        )
    }

    /// The sum of the fixture's balances, or `None` when it exceeds `u64`.
    pub fn total_balance(&self) -> Option<u64> {
        self.wallets
            .iter()
            .try_fold(0u64, |sum, w| sum.checked_add(w.balance))
    }

    /// The fixture as a wallet repository of the network with address byte `network`: the
    /// wallets, then the vote balances and voter counts built from the voters. A message when the
    /// fixture cannot be built (for example, a vote for a name no fixture delegate has).
    pub fn to_wallets(&self, network: u8) -> Result<WalletRepository, String> {
        let mut wallets = WalletRepository::new(network);
        for (at, w) in self.wallets.iter().enumerate() {
            if wallets.has_by_address(&w.address) {
                return Err(format!("wallet {at}: {} is listed twice", w.address));
            }
            if let Some(pk) = &w.public_key {
                if Address::from_public_key(pk, network) != w.address {
                    return Err(format!("wallet {at}: pk is not the key of {}", w.address));
                }
                if wallets.has_by_public_key(&pk.to_compressed()) {
                    return Err(format!("wallet {at}: another wallet has its pk"));
                }
            }
            if !w.votes.is_empty() && w.public_key.is_none() {
                return Err(format!("wallet {at}: a voter needs a public key"));
            }
            let wallet = wallets.get_or_create(&w.address);
            wallet
                .credit(w.balance)
                .map_err(|e| format!("wallet {at}: {e}"))?;
            wallet.set_nonce(w.nonce);
            wallet.set_second_public_key(w.second_public_key);
            wallet.set_votes(&w.votes);
            if let Some(pk) = &w.public_key {
                wallets.bind_public_key(&w.address, pk.clone());
            }
            if let Some(d) = &w.delegate {
                let record = wallets
                    .register_delegate(&w.address, d.username.clone())
                    .map_err(|e| format!("wallet {at}: {e}"))?;
                record.resigned = d.resigned;
                record.resigned_at = d.resigned_at;
                record.produced_blocks = d.produced_blocks;
                record.forged_fees = d.forged_fees;
                record.forged_rewards = d.forged_rewards;
                record.burned_fees = d.burned_fees;
                record.donations = d.donations;
                record.last_block = d.last_block;
            }
        }
        for (at, w) in self.wallets.iter().enumerate() {
            if let Some(name) = w.votes.keys().find(|n| !wallets.has_by_username(n)) {
                return Err(format!(
                    "wallet {at}: a vote for {name}, which is no fixture delegate"
                ));
            }
            if !w.votes.is_empty() && w.votes.values().map(|b| u32::from(*b)).sum::<u32>() != 10_000
            {
                return Err(format!(
                    "wallet {at}: the votes do not add up to 10,000 basis points"
                ));
            }
        }
        for (at, w) in self.wallets.iter().enumerate() {
            if w.votes.is_empty() {
                continue;
            }
            update_vote_shares(&mut wallets, &w.address)
                .map_err(|e| format!("wallet {at}: {e}"))?;
            increase_vote_balances(&mut wallets, &w.address, true)
                .map_err(|e| format!("wallet {at}: {e}"))?;
        }
        Ok(wallets)
    }
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::identities::KeyPair;

    use super::*;

    fn key(passphrase: &str) -> PublicKey {
        KeyPair::from_passphrase(passphrase)
            .unwrap()
            .public_key()
            .clone()
    }

    fn sample() -> Value {
        let (a, b) = (key("fixture a"), key("fixture b"));
        json!([
            {"a": Address::from_public_key(&a, 90).to_base58(), "pk": a.to_hex(), "bal": "1000", "n": "3",
             "u": "alpha", "rs": 0, "resignedAt": 7, "pb": 2, "ff": "5", "fr": "6", "bf": "4", "dn": "1",
             "lb": "11".repeat(32), "v": [["alpha", 10000]]},
            {"a": Address::from_public_key(&b, 90).to_base58(), "pk": b.to_hex(), "bal": "20", "n": "1",
             "spk": key("second").to_hex(), "v": [["alpha", 10000]]},
        ])
    }

    #[test]
    fn json_round_trips() {
        let value = sample();
        let fixture = Fixture::from_json(&value).unwrap();
        assert_eq!(fixture.to_json(), value);
        let d = fixture.wallets[0].delegate.as_ref().unwrap();
        assert_eq!(
            (d.resigned, d.resigned_at),
            (Some(Resignation::Temporary), Some(7))
        );
        assert_eq!(fixture.total_balance(), Some(1020));
    }

    #[test]
    fn refuses_malformed_fixtures() {
        let mut value = sample();
        value[1]["bal"] = json!(20);
        assert!(Fixture::from_json(&value).unwrap_err().contains("bal"));
        let mut value = sample();
        value[1]["rs"] = json!(1);
        assert!(
            Fixture::from_json(&value)
                .unwrap_err()
                .contains("without u")
        );
        let mut value = sample();
        value[0]["extra"] = json!(1);
        assert!(
            Fixture::from_json(&value)
                .unwrap_err()
                .contains("unknown key")
        );
        assert!(Fixture::from_json(&json!({})).is_err());
    }

    #[test]
    fn to_wallets_refuses_votes_for_strangers() {
        let mut value = sample();
        value[1]["v"] = json!([["nobody", 10000]]);
        let fixture = Fixture::from_json(&value).unwrap();
        assert!(fixture.to_wallets(90).unwrap_err().contains("nobody"));
        let mut value = sample();
        value[1].as_object_mut().unwrap().remove("pk");
        let fixture = Fixture::from_json(&value).unwrap();
        assert!(fixture.to_wallets(90).unwrap_err().contains("public key"));
    }
}
