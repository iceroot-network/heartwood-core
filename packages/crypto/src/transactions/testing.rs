//! Test support: devnet parameters and generators of valid transactions.

use proptest::collection::{btree_set, vec};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

use crate::identities::{Address, KeyPair, PublicKeyBytes};
use crate::managers::{Milestones, Network, Params};
use crate::transactions::types::{Asset, Memo, ResignationType, TransferItem, Votes};
use crate::validation::MAX_AMOUNT;

/// The devnet network byte.
pub(crate) const NETWORK: u8 = 90;

/// The devnet rules of the golden vectors, at height 1.
pub(crate) fn devnet() -> Params {
    let ranks: Map<String, Value> = (1..=53u64)
        .map(|rank| {
            (
                rank.to_string(),
                json!(180_000_000 + (rank - 1) / 11 * 10_000_000),
            )
        })
        .collect();
    let milestone = json!([{
        "height": 1,
        "activeDelegates": 53,
        "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 },
        "blockTime": 8,
        "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 },
        "epoch": "2026-01-01T00:00:00.000Z",
        "fees": { "staticFees": { "transfer": 50_000_000, "vote": 9_000_000 } },
        "transfer": { "maximum": 256, "minimum": 1 },
        "reward": 0,
        "dynamicReward": { "enabled": true, "ranks": ranks, "secondaryReward": 180_000_000 },
        "donations": {}
    }]);
    let network = Network {
        name: "devnet".into(),
        message_prefix: "devnet message:\n".into(),
        address_character: Some("d".into()),
        bip32_public: 70_617_039,
        bip32_private: 70_615_956,
        pub_key_hash: NETWORK,
        nethash: "0".repeat(64),
        wif: 252,
        slip44: 1,
        token: "dROOT".into(),
        symbol: "dRT".into(),
        explorer: String::new(),
    };
    Milestones::load(&milestone.to_string(), &network)
        .expect("the test milestones load")
        .at(1)
        .clone()
}

/// A key pair from a passphrase.
pub(crate) fn keys(passphrase: &str) -> KeyPair {
    KeyPair::from_passphrase(passphrase).expect("a valid passphrase key")
}

/// A delegate name that the rules accept.
pub(crate) fn delegate_name() -> impl Strategy<Value = String> {
    "[a-z!@$&.][a-z0-9!@$&_.]{0,19}"
}

/// Votes that the rules accept: 1 to 53 delegate names whose shares add up to 10,000.
pub(crate) fn valid_votes() -> impl Strategy<Value = Votes> {
    btree_set(delegate_name(), 1..=53)
        .prop_flat_map(|names| {
            let cuts = btree_set(1u16..10_000, names.len() - 1);
            (Just(names), cuts)
        })
        .prop_map(|(names, cuts)| {
            let mut bounds: Vec<u16> = cuts.into_iter().collect();
            bounds.push(10_000);
            let mut previous = 0;
            names
                .into_iter()
                .zip(bounds)
                .map(|(name, bound)| {
                    let share = bound - previous;
                    previous = bound;
                    (name, share)
                })
                .collect()
        })
}

/// A memo of 1 to 255 bytes, or none.
pub(crate) fn memo() -> impl Strategy<Value = Option<Memo>> {
    proptest::option::of(
        ".{1,120}".prop_filter_map("a memo of at most 255 bytes", |text: String| {
            Memo::new(&text).ok()
        }),
    )
}

/// An asset that the rules accept, of any of the six types.
pub(crate) fn valid_asset() -> impl Strategy<Value = Asset> {
    let recipient = any::<[u8; 20]>().prop_map(|hash| {
        let mut bytes = [NETWORK; 21];
        bytes[1..].copy_from_slice(&hash);
        Address::from_bytes(bytes)
    });
    let item = (1..=MAX_AMOUNT, recipient)
        .prop_map(|(amount, recipient)| TransferItem { amount, recipient });
    prop_oneof![
        vec(item, 1..=8).prop_map(|transfers| Asset::Transfer { transfers }),
        "[a-z]{1,12}".prop_map(|seed| Asset::SecondSignature {
            public_key: PublicKeyBytes::from_array(
                keys(&seed)
                    .public_key()
                    .as_bytes()
                    .try_into()
                    .expect("33 bytes")
            ),
        }),
        delegate_name().prop_map(|username| Asset::DelegateRegistration { username }),
        prop_oneof![
            Just(None),
            Just(Some(ResignationType::PermanentResign)),
            Just(Some(ResignationType::NotResigned)),
        ]
        .prop_map(|resignation_type| Asset::DelegateResignation { resignation_type }),
        (0..=MAX_AMOUNT).prop_map(|amount| Asset::Burn { amount }),
        prop_oneof![Just(Votes::new()), valid_votes()].prop_map(|votes| Asset::Vote { votes }),
    ]
}
