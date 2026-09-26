//! Wallets, the repository and its indexes, and vote balances: the order in which a voter's
//! remainder is handed out, the empty wallets a lookup creates, the upkeep of the public-key and
//! username indexes, the delegates' vote balances and voter counts, and a voter's shares against
//! the golden vectors of class V15.

use heartwood_crypto::transactions::types::Votes;
use heartwood_crypto::{Address, KeyPair, PublicKey};
use heartwood_state::errors::WalletError;
use heartwood_state::wallets::vote_balances::{
    calculate_vote_amounts, decrease_vote_balances, increase_vote_balances, update_vote_shares,
    update_wallet_vote_balance,
};
use heartwood_state::{Delegate, Wallet, WalletRepository};
use serde_json::Value;

const NETWORK: u8 = 90;

const V15: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/V15-vote-amounts.jsonl"
));

/// The records of a vector file, after its meta record.
fn records(text: &str, class: &str) -> Vec<Value> {
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a JSON record"));
    let meta = lines.next().expect("a meta record");
    assert_eq!(meta["op"], "meta");
    assert_eq!(meta["class"], class);
    let records: Vec<Value> = lines.collect();
    assert_eq!(meta["records"], records.len());
    records
}

fn votes(entries: &[(&str, u16)]) -> Votes {
    entries
        .iter()
        .map(|&(name, basis_points)| (name.to_owned(), basis_points))
        .collect()
}

fn key(passphrase: &str) -> (PublicKey, Address) {
    let public_key = KeyPair::from_passphrase(passphrase)
        .unwrap()
        .public_key()
        .clone();
    let address = Address::from_public_key(&public_key, NETWORK);
    (public_key, address)
}

/// A wallet with a key, and as a delegate when `username` is given.
fn add(wallets: &mut WalletRepository, passphrase: &str, username: Option<&str>) -> Address {
    let (public_key, address) = key(passphrase);
    wallets.bind_public_key(&address, public_key);
    if let Some(username) = username {
        wallets
            .register_delegate(&address, username.into())
            .unwrap();
    }
    address
}

fn delegate<'a>(wallets: &'a WalletRepository, username: &str) -> &'a Delegate {
    wallets
        .find_by_username(username)
        .unwrap()
        .delegate()
        .unwrap()
}

/// `(vote balance, voters)` of each named delegate.
fn tallies(wallets: &WalletRepository, usernames: &[&str]) -> Vec<(u64, u64)> {
    usernames
        .iter()
        .map(|username| {
            let delegate = delegate(wallets, username);
            (delegate.vote_balance, delegate.voters)
        })
        .collect()
}

#[test]
fn v15_vote_amounts() {
    for record in records(V15, "V15-vote-amounts") {
        let input = &record["input"];
        let balance: u64 = input["balance"].as_str().unwrap().parse().unwrap();
        let votes: Votes = input["votes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                let name = entry[0].as_str().unwrap().to_owned();
                (name, u16::try_from(entry[1].as_u64().unwrap()).unwrap())
            })
            .collect();
        let actual: Vec<(String, String)> = calculate_vote_amounts(balance, &votes)
            .into_iter()
            .map(|(name, share)| (name, share.to_string()))
            .collect();
        let expected: Vec<(String, String)> = record["output"]["amounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| {
                let text = |value: &Value| value.as_str().unwrap().to_owned();
                (text(&pair[0]), text(&pair[1]))
            })
            .collect();
        assert_eq!(actual, expected, "V15 {}", record["name"]);
    }
}

#[test]
fn the_remainder_goes_to_the_first_votes_in_the_kept_order() {
    let mut wallets = WalletRepository::new(NETWORK);
    add(&mut wallets, "a9", Some("a9"));
    add(&mut wallets, "a10", Some("a10"));
    let voter = add(&mut wallets, "voter", None);
    let wallet = wallets.get_or_create(&voter);
    wallet.credit(101).unwrap();
    // The wallet keeps its votes in the canonical order: basis points descending, then names in
    // byte order, which puts a10 before a9.
    wallet.set_votes(&votes(&[("a9", 5_000), ("a10", 5_000)]));
    update_vote_shares(&mut wallets, &voter).unwrap();
    let shares: Vec<(&str, u64)> = wallets
        .find_by_address(&voter)
        .unwrap()
        .vote_shares()
        .iter()
        .map(|(name, &share)| (name.as_str(), share))
        .collect();
    assert_eq!(shares, [("a10", 51), ("a9", 50)]);

    // Higher basis points come first whatever their names, and the unit left over goes to the
    // first entry, not to the largest fraction: 102 splits as 68.0034 and 33.9966.
    let wallet = wallets.get_or_create(&voter);
    wallet.set_votes(&votes(&[("a10", 3_333), ("a9", 6_667)]));
    wallet.credit(1).unwrap();
    update_vote_shares(&mut wallets, &voter).unwrap();
    let wallet = wallets.find_by_address(&voter).unwrap();
    assert_eq!(wallet.vote_shares()["a9"], 69);
    assert_eq!(wallet.vote_shares()["a10"], 33);
}

#[test]
fn a_lookup_for_a_change_creates_an_empty_wallet() {
    let mut wallets = WalletRepository::new(NETWORK);
    let (_, address) = key("lazy");
    assert!(!wallets.has_by_address(&address));
    assert!(wallets.wallet_mut(&address).is_none());
    assert!(wallets.is_empty());

    // Reading vote balances of a wallet that does not exist creates nothing.
    update_wallet_vote_balance(&mut wallets, &address).unwrap();
    increase_vote_balances(&mut wallets, &address, true).unwrap();
    decrease_vote_balances(&mut wallets, &address, true).unwrap();
    update_vote_shares(&mut wallets, &address).unwrap();
    assert!(wallets.is_empty());

    let wallet = wallets.get_or_create(&address);
    assert_eq!(wallet, &Wallet::new(address));
    assert_eq!(wallet.balance(), 0);
    assert_eq!(wallet.nonce(), 0);
    assert!(wallet.public_key().is_none());
    assert!(wallet.second_public_key().is_none());
    assert!(!wallet.is_delegate());
    assert!(!wallet.has_voted());
    assert!(wallet.vote_shares().is_empty());
    wallet.set_nonce(4);
    assert_eq!(wallets.get_or_create(&address).nonce(), 4);
    assert_eq!(wallets.len(), 1);
    assert_eq!(
        wallets.iter().map(Wallet::address).collect::<Vec<_>>(),
        [&address]
    );
}

#[test]
fn wallets_iterate_in_the_order_they_were_first_used() {
    let mut wallets = WalletRepository::new(NETWORK);
    let addresses: Vec<Address> = ["c", "a", "b"].iter().map(|p| key(p).1).collect();
    for address in &addresses {
        wallets.get_or_create(address);
    }
    wallets.get_or_create(&addresses[1]).credit(1).unwrap();
    let order: Vec<Address> = wallets.iter().map(|wallet| *wallet.address()).collect();
    assert_eq!(order, addresses);
}

#[test]
fn the_indexes_follow_keys_and_names() {
    let mut wallets = WalletRepository::new(NETWORK);
    let (public_key, address) = key("indexed");
    let compressed = public_key.to_compressed();
    let (_, other) = key("other");

    wallets.bind_public_key(&address, public_key.clone());
    assert!(wallets.has_by_public_key(&compressed));
    assert_eq!(wallets.indexes().by_public_key(&compressed), Some(&address));
    assert_eq!(
        wallets
            .find_by_public_key(&compressed)
            .and_then(Wallet::public_key),
        Some(&public_key)
    );

    // A delegate name moves with its record: registering again under another name frees the
    // first one.
    wallets.register_delegate(&address, "first".into()).unwrap();
    wallets
        .register_delegate(&address, "second".into())
        .unwrap();
    assert!(!wallets.has_by_username("first"));
    assert_eq!(wallets.indexes().by_username("second"), Some(&address));
    assert_eq!(
        wallets.register_delegate(&other, "second".into()),
        Err(WalletError::UsernameTaken {
            username: "second".into()
        })
    );
    assert!(!wallets.has_by_address(&other));
    wallets.register_delegate(&other, "first".into()).unwrap();
    let names: Vec<(&str, &Address)> = wallets.indexes().usernames().collect();
    assert_eq!(names, [("first", &other), ("second", &address)]);
    let delegates: Vec<&Address> = wallets.delegates().map(Wallet::address).collect();
    assert_eq!(delegates, [&other, &address]);

    assert_eq!(
        wallets.delegate_mut("nobody"),
        Err(WalletError::UnknownDelegate {
            username: "nobody".into()
        })
    );
    wallets.delegate_mut("second").unwrap().credit(7).unwrap();
    assert_eq!(wallets.find_by_username("second").unwrap().balance(), 7);

    let forgotten = wallets.forget_delegate(&address).unwrap();
    assert_eq!(forgotten.username(), "second");
    assert!(wallets.forget_delegate(&address).is_none());
    assert!(!wallets.has_by_username("second"));
    assert!(
        wallets
            .find_by_address(&address)
            .unwrap()
            .delegate()
            .is_none()
    );

    wallets.forget_public_key(&address);
    assert!(!wallets.has_by_public_key(&compressed));
    assert!(wallets.find_by_public_key(&compressed).is_none());
    assert!(wallets.has_by_address(&address));
    // Forgetting again, or for a wallet that does not exist, changes nothing.
    wallets.forget_public_key(&address);
    wallets.forget_public_key(&key("missing").1);
    assert!(!wallets.has_by_address(&key("missing").1));
}

#[test]
fn vote_balances_follow_the_voters() {
    let mut wallets = WalletRepository::new(NETWORK);
    add(&mut wallets, "d1", Some("d1"));
    add(&mut wallets, "d2", Some("d2"));
    add(&mut wallets, "d3", Some("d3"));
    let voter = add(&mut wallets, "voter", None);
    let wallet = wallets.get_or_create(&voter);
    wallet.credit(1_001).unwrap();
    wallet.set_votes(&votes(&[("d2", 4_000), ("d1", 6_000)]));

    update_vote_shares(&mut wallets, &voter).unwrap();
    increase_vote_balances(&mut wallets, &voter, true).unwrap();
    assert_eq!(
        tallies(&wallets, &["d1", "d2", "d3"]),
        [(601, 1), (400, 1), (0, 0)]
    );

    // A balance change moves the shares, not the voter counts.
    wallets.get_or_create(&voter).credit(1_000).unwrap();
    update_wallet_vote_balance(&mut wallets, &voter).unwrap();
    assert_eq!(
        tallies(&wallets, &["d1", "d2", "d3"]),
        [(1_201, 1), (800, 1), (0, 0)]
    );
    // Nothing changed since: the same shares again.
    update_wallet_vote_balance(&mut wallets, &voter).unwrap();
    assert_eq!(
        tallies(&wallets, &["d1", "d2", "d3"]),
        [(1_201, 1), (800, 1), (0, 0)]
    );

    // A new vote: take the old shares and voters, set the votes, add the new ones.
    decrease_vote_balances(&mut wallets, &voter, true).unwrap();
    assert_eq!(
        tallies(&wallets, &["d1", "d2", "d3"]),
        [(0, 0), (0, 0), (0, 0)]
    );
    wallets
        .get_or_create(&voter)
        .set_votes(&votes(&[("d3", 10_000)]));
    update_vote_shares(&mut wallets, &voter).unwrap();
    increase_vote_balances(&mut wallets, &voter, true).unwrap();
    assert_eq!(
        tallies(&wallets, &["d1", "d2", "d3"]),
        [(0, 0), (0, 0), (2_001, 1)]
    );

    // A delegate that votes for itself counts its own balance.
    let d3 = wallets.find_by_username("d3").unwrap().address().to_owned();
    let wallet = wallets.get_or_create(&d3);
    wallet.credit(5).unwrap();
    wallet.set_votes(&votes(&[("d3", 10_000)]));
    update_vote_shares(&mut wallets, &d3).unwrap();
    increase_vote_balances(&mut wallets, &d3, true).unwrap();
    assert_eq!(tallies(&wallets, &["d3"]), [(2_006, 2)]);
}

#[test]
fn a_refused_change_leaves_every_delegate_as_it_was() {
    let mut wallets = WalletRepository::new(NETWORK);
    add(&mut wallets, "d1", Some("d1"));
    add(&mut wallets, "d2", Some("d2"));
    let voter = add(&mut wallets, "voter", None);
    let wallet = wallets.get_or_create(&voter);
    wallet.credit(100).unwrap();
    wallet.set_votes(&votes(&[("d1", 5_000), ("ghost", 5_000)]));
    update_vote_shares(&mut wallets, &voter).unwrap();
    let before = wallets.clone();
    assert_eq!(
        increase_vote_balances(&mut wallets, &voter, true),
        Err(WalletError::UnknownDelegate {
            username: "ghost".into()
        })
    );
    assert_eq!(wallets, before);

    // A vote balance that would pass 2^64 − 1.
    wallets
        .get_or_create(&voter)
        .set_votes(&votes(&[("d1", 5_000), ("d2", 5_000)]));
    update_vote_shares(&mut wallets, &voter).unwrap();
    wallets
        .delegate_mut("d2")
        .unwrap()
        .delegate_mut()
        .unwrap()
        .vote_balance = u64::MAX;
    let before = wallets.clone();
    assert_eq!(
        increase_vote_balances(&mut wallets, &voter, false),
        Err(WalletError::DelegateRange {
            username: "d2".into()
        })
    );
    assert_eq!(wallets, before);

    // Taking back more than a delegate holds.
    assert_eq!(
        decrease_vote_balances(&mut wallets, &voter, true),
        Err(WalletError::DelegateRange {
            username: "d1".into()
        })
    );
    assert_eq!(wallets, before);

    // A vote for a delegate whose registration is gone refuses the update, even when the
    // shares stay the same.
    let d1 = wallets.find_by_username("d1").unwrap().address().to_owned();
    wallets.forget_delegate(&d1);
    let before = wallets.clone();
    assert_eq!(
        update_wallet_vote_balance(&mut wallets, &voter),
        Err(WalletError::UnknownDelegate {
            username: "d1".into()
        })
    );
    assert_eq!(wallets, before);
}
