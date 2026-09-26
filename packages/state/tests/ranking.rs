//! The delegate ranking and the delegates of a round: the golden vectors of class V08, and the
//! rules one by one (resigned delegates lose their rank, equal vote balances are ordered by key
//! bytes, the refusals leave every rank as it was).

use heartwood_crypto::{Address, KeyPair, PublicKey};
use heartwood_kernel::RoundInfo;
use heartwood_state::dpos::{build_delegate_ranking, set_delegates_round};
use heartwood_state::errors::RoundError;
use heartwood_state::round_state::RoundDelegate;
use heartwood_state::{Resignation, WalletRepository};
use serde_json::{Value, json};

const NETWORK: u8 = 90;
/// `activeDelegates` of the devnet the vectors were made on.
const DEVNET_ACTIVE_DELEGATES: u64 = 53;

const V08: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/V08-ranking.jsonl"
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

fn round_of(max_delegates: u64) -> RoundInfo {
    RoundInfo {
        round: 1,
        round_height: 1,
        next_round: 1,
        max_delegates,
    }
}

/// A delegate of a test set.
struct Entry<'a> {
    username: &'a str,
    public_key: PublicKey,
    vote_balance: u64,
    resigned: Option<Resignation>,
}

/// The delegates of `entries`, each in its own wallet at an address made from its position, so
/// that two delegates can hold the same key. Returns the addresses in entry order.
fn repository(entries: &[Entry<'_>]) -> (WalletRepository, Vec<Address>) {
    let mut wallets = WalletRepository::new(NETWORK);
    let mut addresses = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut bytes = [NETWORK; 21];
        bytes[13..].copy_from_slice(&u64::try_from(index).unwrap().to_be_bytes());
        let address = Address::from_bytes(bytes);
        wallets.bind_public_key(&address, entry.public_key.clone());
        let delegate = wallets
            .register_delegate(&address, entry.username.into())
            .unwrap();
        delegate.vote_balance = entry.vote_balance;
        delegate.resigned = entry.resigned;
        addresses.push(address);
    }
    (wallets, addresses)
}

fn ranks(wallets: &WalletRepository, addresses: &[Address]) -> Vec<Option<u32>> {
    addresses
        .iter()
        .map(|address| wallets.find_by_address(address)?.delegate()?.rank)
        .collect()
}

fn names(round: &[RoundDelegate]) -> Vec<&str> {
    round.iter().map(|d| d.username.as_str()).collect()
}

/// The error class the reference raises for a refused ranking or round.
fn class(error: &RoundError) -> &'static str {
    match error {
        RoundError::NoPublicKey { .. } => "AssertionException",
        _ => "Error",
    }
}

#[test]
fn v08_ranking() {
    for record in records(V08, "V08-ranking") {
        let input = &record["input"];
        let delegates = input["delegates"].as_array().unwrap();
        let entries: Vec<Entry<'_>> = delegates
            .iter()
            .map(|d| Entry {
                username: d["username"].as_str().unwrap(),
                public_key: PublicKey::from_hex(d["publicKey"].as_str().unwrap()).unwrap(),
                vote_balance: d["voteBalance"].as_str().unwrap().parse().unwrap(),
                resigned: d["resigned"].as_u64().map(|number| match number {
                    0 => Resignation::Temporary,
                    _ => Resignation::Permanent,
                }),
            })
            .collect();
        let max_delegates = input["maxDelegates"]
            .as_u64()
            .unwrap_or(DEVNET_ACTIVE_DELEGATES);
        let (mut wallets, addresses) = repository(&entries);
        let label = format!("V08 {}", record["name"]);

        let ranked = match build_delegate_ranking(&mut wallets) {
            Ok(ranked) => ranked,
            Err(error) => {
                assert_eq!(record["error"]["class"], class(&error), "{label}");
                continue;
            }
        };
        let output = &record["output"];
        let actual_ranks: Vec<Value> = entries
            .iter()
            .zip(ranks(&wallets, &addresses))
            .map(|(entry, rank)| json!([entry.username, rank]))
            .collect();
        assert_eq!(output["ranks"], Value::from(actual_ranks), "{label}");
        match set_delegates_round(&wallets, &ranked, &round_of(max_delegates)) {
            Ok(round) => assert_eq!(output["round"], json!(names(&round)), "{label}"),
            Err(error) => assert_eq!(output["round"]["error"]["class"], class(&error), "{label}"),
        }
    }
}

fn keys(count: usize) -> Vec<PublicKey> {
    (0..count)
        .map(|i| {
            KeyPair::from_passphrase(&format!("ranking {i}"))
                .unwrap()
                .public_key()
                .clone()
        })
        .collect()
}

#[test]
fn vote_balance_first_then_key_bytes() {
    let keys = keys(4);
    let mut order: Vec<usize> = (0..4).collect();
    order.sort_by_key(|&i| keys[i].as_bytes().to_vec());
    let entries: Vec<Entry<'_>> = ["a", "b", "c", "d"]
        .iter()
        .zip(&keys)
        .map(|(username, public_key)| Entry {
            username,
            public_key: public_key.clone(),
            vote_balance: 10,
            resigned: None,
        })
        .collect();
    let (mut wallets, addresses) = repository(&entries);
    let ranked = build_delegate_ranking(&mut wallets).unwrap();
    let by_key: Vec<Address> = order.iter().map(|&i| addresses[i]).collect();
    assert_eq!(ranked, by_key);

    // A larger vote balance comes first whatever its key.
    let last = order[3];
    wallets
        .wallet_mut(&addresses[last])
        .unwrap()
        .delegate_mut()
        .unwrap()
        .vote_balance = 11;
    let ranked = build_delegate_ranking(&mut wallets).unwrap();
    assert_eq!(ranked[0], addresses[last]);
    assert_eq!(ranks(&wallets, &addresses)[last], Some(1));
}

#[test]
fn resigned_delegates_lose_their_rank() {
    let keys = keys(3);
    let entries: Vec<Entry<'_>> = ["a", "b", "c"]
        .iter()
        .zip(&keys)
        .zip([30, 20, 10])
        .map(|((username, public_key), vote_balance)| Entry {
            username,
            public_key: public_key.clone(),
            vote_balance,
            resigned: None,
        })
        .collect();
    let (mut wallets, addresses) = repository(&entries);
    build_delegate_ranking(&mut wallets).unwrap();
    assert_eq!(ranks(&wallets, &addresses), [Some(1), Some(2), Some(3)]);

    for (address, resignation) in [
        (addresses[0], Resignation::Temporary),
        (addresses[2], Resignation::Permanent),
    ] {
        wallets
            .wallet_mut(&address)
            .unwrap()
            .delegate_mut()
            .unwrap()
            .resigned = Some(resignation);
    }
    // Until the next ranking, a resigned delegate keeps the rank of the last one.
    assert_eq!(ranks(&wallets, &addresses), [Some(1), Some(2), Some(3)]);
    let ranked = build_delegate_ranking(&mut wallets).unwrap();
    assert_eq!(ranked, [addresses[1]]);
    assert_eq!(ranks(&wallets, &addresses), [None, Some(1), None]);
}

#[test]
fn refusals_leave_every_rank_as_it_was() {
    let keys = keys(3);
    let entries = [
        Entry {
            username: "a",
            public_key: keys[0].clone(),
            vote_balance: 5,
            resigned: None,
        },
        Entry {
            username: "b",
            public_key: keys[1].clone(),
            vote_balance: 9,
            resigned: Some(Resignation::Temporary),
        },
        Entry {
            username: "c",
            public_key: keys[0].clone(),
            vote_balance: 5,
            resigned: None,
        },
    ];
    let (mut wallets, addresses) = repository(&entries);
    wallets
        .wallet_mut(&addresses[1])
        .unwrap()
        .delegate_mut()
        .unwrap()
        .rank = Some(7);
    let before = wallets.clone();
    assert_eq!(
        build_delegate_ranking(&mut wallets),
        Err(RoundError::IdenticalPublicKeys {
            first: "a".into(),
            second: "c".into()
        })
    );
    assert_eq!(wallets, before);

    // The same key at different vote balances is never compared.
    wallets
        .wallet_mut(&addresses[2])
        .unwrap()
        .delegate_mut()
        .unwrap()
        .vote_balance = 6;
    let ranked = build_delegate_ranking(&mut wallets).unwrap();
    assert_eq!(ranked, [addresses[2], addresses[0]]);
    assert_eq!(ranks(&wallets, &addresses), [Some(2), None, Some(1)]);
}

#[test]
fn a_delegate_without_a_key_is_ranked_unless_its_vote_balance_ties() {
    let keys = keys(2);
    let mut wallets = WalletRepository::new(NETWORK);
    let keyless = Address::from_bytes([NETWORK; 21]);
    wallets
        .register_delegate(&keyless, "keyless".into())
        .unwrap()
        .vote_balance = 100;
    for (username, public_key) in ["a", "b"].iter().zip(&keys) {
        let address = Address::from_public_key(public_key, NETWORK);
        wallets.bind_public_key(&address, public_key.clone());
        wallets
            .register_delegate(&address, (*username).into())
            .unwrap()
            .vote_balance = 50;
    }
    let ranked = build_delegate_ranking(&mut wallets).unwrap();
    assert_eq!(ranked[0], keyless);
    // Its round needs a key, though.
    assert_eq!(
        set_delegates_round(&wallets, &ranked, &round_of(1)),
        Err(RoundError::NoPublicKey {
            username: "keyless".into()
        })
    );

    wallets
        .wallet_mut(&keyless)
        .unwrap()
        .delegate_mut()
        .unwrap()
        .vote_balance = 50;
    let before = wallets.clone();
    assert_eq!(
        build_delegate_ranking(&mut wallets),
        Err(RoundError::NoPublicKey {
            username: "keyless".into()
        })
    );
    assert_eq!(wallets, before);
}

#[test]
fn a_round_takes_the_first_ranked_delegates() {
    let keys = keys(4);
    let entries: Vec<Entry<'_>> = ["a", "b", "c", "d"]
        .iter()
        .zip(&keys)
        .zip([1, 4, 3, 2])
        .map(|((username, public_key), vote_balance)| Entry {
            username,
            public_key: public_key.clone(),
            vote_balance,
            resigned: None,
        })
        .collect();
    let (mut wallets, _) = repository(&entries);
    let ranked = build_delegate_ranking(&mut wallets).unwrap();
    let round = set_delegates_round(&wallets, &ranked, &round_of(3)).unwrap();
    assert_eq!(names(&round), ["b", "c", "d"]);
    assert_eq!(
        round
            .iter()
            .map(|d| (d.public_key, d.vote_balance))
            .collect::<Vec<_>>(),
        [
            (keys[1].to_compressed(), 4),
            (keys[2].to_compressed(), 3),
            (keys[3].to_compressed(), 2)
        ]
    );
    assert!(
        set_delegates_round(&wallets, &ranked, &round_of(0))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        set_delegates_round(&wallets, &ranked, &round_of(5)),
        Err(RoundError::NotEnoughDelegates {
            required: 5,
            ranked: 4
        })
    );
    assert_eq!(
        set_delegates_round(&wallets, &ranked, &round_of(u64::MAX)),
        Err(RoundError::NotEnoughDelegates {
            required: u64::MAX,
            ranked: 4
        })
    );
}
