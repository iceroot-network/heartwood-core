//! The attester on arbitrary offers: an offer's status text and key list. No input may panic it,
//! and it signs only an offer whose text writes back byte for byte as a status tree and passes the
//! attester's checks (its length, height, slot window, network, own keys each offered at most as
//! often as declared).
//!
//! The text is either arbitrary or a status tree written in canonical form, then perhaps cut or
//! changed by one byte, so that most inputs come close to what a relay offers. An offer that is
//! the canonical form of a valid status for the attester's own keys must be signed.

#![no_main]

use std::sync::{Arc, LazyLock};

use arbitrary::Arbitrary;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::{KeyPair, Milestones, Network};
use heartwood_forger::attestation::MAX_STATUS;
use heartwood_forger::{Attester, DelegateKeys};
use heartwood_kernel::ManualClock;
use heartwood_p2p_wire::status::{
    NetworkConfig, PeerConfig, PluginConfig, SignedState, StatusHeaderData, TokenConfig,
    parse_preimage, session_proof, signing_preimage, verify_attestation,
};
use libfuzzer_sys::fuzz_target;

const NETHASH: &str = "c9b03ab996ef3ac216a2ac53eaee71118cbf7995fa44449a7fb7f94bbe18bcca";
/// The session's challenge.
const CHALLENGE: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";
/// The slot of the attester's clock.
const OWN_SLOT: i64 = 1_000;

struct Setup {
    attester: Attester,
    /// The attester's two keys and a key it does not hold, in hex.
    keys: [String; 3],
}

static SETUP: LazyLock<Setup> = LazyLock::new(|| {
    let network = Network::load(
        r#"{"name":"devnet","messagePrefix":"devnet message:\n","pubKeyHash":90,
            "bip32":{"public":70617039,"private":70615956},"nethash":"00",
            "wif":252,"slip44":1,"client":{"token":"dROOT","symbol":"dRT","explorer":""}}"#
            .replace("\"00\"", &format!("\"{NETHASH}\""))
            .as_str(),
    )
    .expect("the network");
    let milestones = Milestones::load(
        r#"[{"height":1,"activeDelegates":53,"blockTime":8,"epoch":"2026-01-01T00:00:00.000Z",
            "block":{"version":0,"maxTransactions":150,"maxPayload":2097152},"reward":0,
            "burn":{"feeBasisPoints":9000,"txAmount":2000000},"donations":{},
            "dynamicReward":{"enabled":false},"transfer":{"maximum":256,"minimum":1}}]"#,
        &network,
    )
    .expect("the milestones");
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).expect("the epoch");
    let clock = Arc::new(ManualClock::new(epoch + OWN_SLOT * 8_000 + 1_000));
    let pair = |p: &str| KeyPair::from_passphrase(p).expect("a key");
    let keys = ["fuzz delegate a", "fuzz delegate b", "fuzz stranger"].map(pair);
    let hex = keys.each_ref().map(|k| k.public_key().to_hex());
    let [a, b, _] = keys;
    let own = DelegateKeys::from_keys(vec![a, b]);
    Setup {
        attester: Attester::new(Arc::new(own), NETHASH.into(), milestones, clock),
        keys: hex,
    }
});

#[derive(Debug, Arbitrary)]
enum Text {
    Any(String),
    Built {
        height: u32,
        header_height: Option<u32>,
        slot_offset: i8,
        other_network: bool,
        forging_allowed: bool,
        version: String,
        amounts: (u64, u64, u64),
        plugins: Vec<(String, bool, u32, Option<bool>)>,
        change: Option<(u16, Option<u8>)>,
    },
}

#[derive(Debug, Arbitrary)]
struct Offer {
    text: Text,
    /// Indices into the setup's keys: 0 and 1 are the attester's, 2 is not.
    keys: Vec<u8>,
}

/// Whether the key indices name only the attester's two keys, each at most once, as it declares
/// them.
fn once_each(keys: &[u8]) -> bool {
    let count = |key: u8| keys.iter().filter(|&&i| i % 3 == key).count();
    count(0) <= 1 && count(1) <= 1 && count(2) == 0
}

fuzz_target!(|offer: Offer| {
    let setup = &*SETUP;
    let keys: Vec<String> = (offer.keys.iter())
        .map(|&i| setup.keys[usize::from(i % 3)].clone())
        .collect();
    let (text, valid) = match offer.text {
        Text::Any(text) => (text, false),
        Text::Built {
            height,
            header_height,
            slot_offset,
            other_network,
            forging_allowed,
            version,
            amounts,
            plugins,
            change,
        } => {
            let state = SignedState {
                height,
                forging_allowed,
                current_slot: OWN_SLOT + i64::from(slot_offset),
                header: StatusHeaderData {
                    id: "ab".repeat(32),
                    version: 0,
                    timestamp: 8,
                    previous_block: "cd".repeat(32),
                    height: header_height.unwrap_or(height),
                    number_of_transactions: 0,
                    total_amount: amounts.0,
                    total_fee: amounts.1,
                    reward: amounts.2,
                    payload_length: 0,
                    payload_hash: "ef".repeat(32),
                    generator_public_key: setup.keys[0].clone(),
                    block_signature: "01".repeat(64),
                },
            };
            let config = PeerConfig {
                version,
                network: NetworkConfig {
                    version: 90,
                    name: "devnet".into(),
                    nethash: if other_network {
                        "00".repeat(32)
                    } else {
                        NETHASH.into()
                    },
                    explorer: String::new(),
                    token: TokenConfig {
                        name: "dROOT".into(),
                        symbol: "dRT".into(),
                    },
                },
                plugins: (plugins.into_iter())
                    .map(|(name, enabled, port, estimate_total_count)| {
                        let plugin = PluginConfig {
                            enabled,
                            port,
                            estimate_total_count,
                        };
                        (name, plugin)
                    })
                    .collect(),
            };
            let mut bytes = signing_preimage(&state, &config);
            let written = bytes.clone();
            if let Some((at, byte)) = change {
                let at = usize::from(at) % (bytes.len() + 1);
                match byte {
                    Some(byte) => bytes.insert(at, byte),
                    None => bytes.truncate(at),
                }
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            // A tree that reads back from its own text, with the checks' conditions, must be
            // signed; the reader refuses some trees (a plugin named like a prototype property).
            let reads_back = parse_preimage(&written).is_ok();
            let valid = text.as_bytes() == written
                && reads_back
                && header_height.is_none_or(|h| h == height)
                && slot_offset.unsigned_abs() <= 1
                && !other_network
                && text.len() <= MAX_STATUS
                && once_each(&offer.keys);
            (text, valid)
        }
    };
    match setup.attester.sign_offer(&text, &keys, CHALLENGE) {
        Ok(signed) => {
            let (state, config) = parse_preimage(text.as_bytes()).expect("a signed offer reads");
            assert_eq!(signing_preimage(&state, &config), text.as_bytes());
            assert_eq!(state.height, state.header.height);
            assert!(state.current_slot.abs_diff(OWN_SLOT) <= 1);
            assert_eq!(config.network.nethash, NETHASH);
            assert!(text.len() <= MAX_STATUS);
            assert!(once_each(&offer.keys));
            assert_eq!(signed.signatures.len(), keys.len());
            for (key, signature) in keys.iter().zip(&signed.signatures) {
                assert!(verify_attestation(text.as_bytes(), key, signature));
            }
            let proof = session_proof(CHALLENGE).expect("a challenge");
            assert_eq!(signed.proofs.len(), keys.len());
            for (key, proven) in keys.iter().zip(&signed.proofs) {
                assert!(verify_attestation(&proof, key, proven));
            }
        }
        Err(error) => assert!(!valid, "a valid offer was refused: {error}"),
    }
});
