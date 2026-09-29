//! The wallet part of the chain search (`GET /blockchain/search/{id}`) against the reference's
//! `getWalletsLike`: the records of the vector class V34 (see `data::vector_records`) and a committed
//! subset of them (`tests/data/wallet-search.json`, copied from the class: the three-wallet cycles
//! of the comparator, the 20 and 21 character boundary, mixed case, key prefixes and runs that
//! gallop), then the search over the committed one-node chain.
//!
//! The wallets of a record are built as the difftest's fixtures build them, into the wallet
//! index of the chain's state view.

mod data;
mod snapshot;

use heartwood_api::search::wallets_like;
use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::types::Votes;
use heartwood_state::StateView;
use heartwood_state::wallets::vote_balances::{increase_vote_balances, update_vote_shares};
use heartwood_state::wallets::{Resignation, WalletRepository};
use serde_json::Value;

use snapshot::chain;

fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

fn amount(item: &Value, key: &str) -> u64 {
    item[key].as_str().map_or(0, |n| n.parse().unwrap())
}

/// The chain's state view with the wallets of `fixture` (the JSON of a V34 record) and the ranks
/// of `ranks`.
fn view(fixture: &Value, ranks: &Value) -> StateView {
    let network = chain().milestones().at(1).network();
    let mut wallets = WalletRepository::new(network);
    let list = fixture.as_array().unwrap();
    let votes_of = |item: &Value| {
        let mut votes = Votes::new();
        for vote in item["v"].as_array().into_iter().flatten() {
            let bps = u16::try_from(vote[1].as_u64().unwrap()).unwrap();
            votes.insert(vote[0].as_str().unwrap().to_owned(), bps);
        }
        votes
    };
    for item in list {
        let address = Address::from_base58(item["a"].as_str().unwrap()).unwrap();
        let wallet = wallets.get_or_create(&address);
        wallet.credit(amount(item, "bal")).unwrap();
        wallet.set_nonce(amount(item, "n"));
        wallet.set_second_public_key(
            item["spk"]
                .as_str()
                .map(|k| PublicKeyBytes::from_hex(k).unwrap()),
        );
        wallet.set_votes(&votes_of(item));
        if let Some(key) = item["pk"].as_str() {
            wallets.bind_public_key(&address, PublicKey::from_hex(key).unwrap());
        }
        if let Some(name) = item["u"].as_str() {
            let record = wallets
                .register_delegate(&address, name.to_owned())
                .unwrap();
            record.resigned = item["rs"].as_u64().map(|rs| match rs {
                0 => Resignation::Temporary,
                _ => Resignation::Permanent,
            });
            record.produced_blocks = item["pb"].as_u64().unwrap_or(0);
            record.forged_fees = amount(item, "ff");
            record.forged_rewards = amount(item, "fr");
            record.burned_fees = amount(item, "bf");
            record.donations = amount(item, "dn");
        }
    }
    for item in list.iter().filter(|item| item["v"].is_array()) {
        let address = Address::from_base58(item["a"].as_str().unwrap()).unwrap();
        update_vote_shares(&mut wallets, &address).unwrap();
        increase_vote_balances(&mut wallets, &address, true).unwrap();
    }
    for rank in ranks.as_array().into_iter().flatten() {
        let address = Address::from_base58(rank[0].as_str().unwrap()).unwrap();
        let delegate = wallets
            .wallet_mut(&address)
            .unwrap()
            .delegate_mut()
            .unwrap();
        delegate.rank = Some(u32::try_from(rank[1].as_u64().unwrap()).unwrap());
    }
    let mut view = chain().state.clone();
    view.wallets = wallets;
    view
}

/// The record's search: its list, and whether the text of the list (keys in order) is the
/// reference's.
fn check(record: &Value) {
    let input = &record["input"];
    let state = view(&input["fixture"], &input["ranks"]);
    let list = Value::Array(wallets_like(&state, input["input"].as_str().unwrap()));
    let name = &record["name"];
    assert_eq!(list, record["output"]["wallets"], "{name}");
    assert_eq!(
        text(&list),
        text(&record["output"]["wallets"]),
        "{name}: key order"
    );
}

#[test]
fn matches_the_committed_records() {
    let records: Vec<Value> =
        serde_json::from_str(include_str!("data/wallet-search.json")).unwrap();
    let count = |part: &str| {
        records
            .iter()
            .filter(|r| r["name"].as_str().unwrap().contains(part))
            .count()
    };
    assert!(count("cycle") >= 15 && count("input of") >= 9 && count("mixed case") >= 2);
    for record in &records {
        check(record);
    }
}

#[test]
fn matches_the_vector_records() {
    let Some(records) = data::vector_records("api.walletsLike") else {
        return;
    };
    for record in &records {
        check(record);
    }
}

/// The delegates of the committed chain, found by the prefix of their names: the 53 genesis ones in the
/// numeric order of the names, each as the reference's checkpoint has it.
#[test]
fn finds_the_delegates_of_the_chain() {
    let chain = chain();
    let list = wallets_like(&chain.state, "GENESIS_");
    let delegates = chain.checkpoint["delegates"].as_array().unwrap();
    assert_eq!(list.len(), 53);
    let names: Vec<String> = (1..=list.len()).map(|n| format!("genesis_{n}")).collect();
    let found: Vec<&str> = list
        .iter()
        .map(|w| w["delegate"]["username"].as_str().unwrap())
        .collect();
    assert_eq!(found, names);
    for entry in &list {
        let expected = delegates
            .iter()
            .find(|d| d["address"] == entry["address"])
            .unwrap();
        assert_eq!(entry["delegate"]["rank"], expected["rank"]);
        assert_eq!(entry["delegate"]["voters"], expected["voters"]);
        assert_eq!(entry["delegate"]["votes"], expected["votes"]);
        assert_eq!(entry["publicKey"], expected["publicKey"]);
    }
    let ones = wallets_like(&chain.state, "genesis_1");
    assert_eq!(ones.len(), 11);
    assert_eq!(ones[0]["delegate"]["username"], "genesis_1");
}

/// A longer input is an address or a public key prefix, and the whole address finds the wallet.
#[test]
fn finds_a_wallet_by_address_and_key() {
    let chain = chain();
    let delegate = &chain.checkpoint["delegates"][0];
    let address = delegate["address"].as_str().unwrap();
    for input in [
        address.to_lowercase(),
        address.to_uppercase(),
        address[..21].to_owned(),
    ] {
        let list = wallets_like(&chain.state, &input);
        assert!(
            list.iter().any(|w| w["address"] == delegate["address"]),
            "{input}"
        );
    }
    let key = delegate["publicKey"].as_str().unwrap();
    let list = wallets_like(&chain.state, &key[..40]);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["address"], delegate["address"]);
    assert!(wallets_like(&chain.state, "genesis_1x").is_empty());
}
