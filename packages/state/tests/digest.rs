//! The canonical state digest: the bytes of each kind of record, the keys left out when their
//! value is absent, the order of wallets and donations, text escapes, and the tagged hashes of
//! sections, heights and replays.
//!
//! Addresses and keys are the reference's own values for the devnet chain definition of the
//! `V10-genesis` vectors (its first seed); the expected hashes were computed independently of
//! this crate, from the literal section text.

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::types::Votes;
use heartwood_crypto::{Aux, Block, BlockId, KeyPair, Milestones, Network};
use heartwood_state::digest::{
    DigestLine, Sections, Step, block_section, chain_digest, missed_section, supply_section,
    wallet_section,
};
use heartwood_state::round_state::MissedBlock;
use heartwood_state::stores::Totals;
use heartwood_state::wallets::{Resignation, WalletRepository};
use serde_json::Value;

const V10: &str = include_str!("../../crypto/tests/vectors/V10-genesis.jsonl");
const NETWORK: u8 = 90;

/// The crypto files of the first chain definition of `V10-genesis`.
fn devnet() -> (Milestones, Value) {
    let record: Value = V10
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["op"] == "genesis.generate")
        .unwrap();
    let files = &record["output"]["files"];
    let file = |name: &str| files[name].as_str().unwrap();
    let network = Network::load(file("crypto/network.json")).unwrap();
    let milestones = Milestones::load(file("crypto/milestones.json"), &network).unwrap();
    let genesis = serde_json::from_str(file("crypto/genesisBlock.json")).unwrap();
    (milestones, genesis)
}

fn address(text: &str) -> Address {
    Address::from_base58_for(text, NETWORK).unwrap()
}

fn key(hex: &str) -> PublicKey {
    PublicKey::from_hex(hex).unwrap()
}

const GENESIS_1: (&str, &str) = (
    "dHrw5YKu2GcERR3hiwc4euftTpnBTNLyJx",
    "03cd5721eff66053eb2d7d38c63044be5ea3e7049eb5b22c91ed88e141e1abcee2",
);
const GENESIS_2: (&str, &str) = (
    "dKVpLt8jZk4PTGPE1Sr7dCfbRmDd37GZ1r",
    "02f060d6f4d82a10865296502171b136118e33b3830cfeaed06ec7a975028c4968",
);
const WALLET_1: (&str, &str) = (
    "dPPBDPkvZsz1xs52r1cToidEbdmzbYpCSB",
    "02a109e32065ed1f48b49723f7e3d844dc84c2dc2f7fdba7cd2aecfc578fca10af",
);
const WALLET_2: &str = "dQ7BfoVGndGzTK8Bjb4Kqc6G5GaVXU7jTK";
const WALLET_3: &str = "dM1UwvdL4GXh1SR8YwbqSJinwno2HowKJN";
const WALLET_4: &str = "db8LAnj9J5r8WHQWRjzDvBHuRaK58Tk67J";
const TEAM: &str = "daTBxkSJk2tZhujYcYSQtxj5RRFZ8HcxW8";

/// Wallets of every shape, created in an order that is not the digest's.
fn wallets() -> WalletRepository {
    let mut wallets = WalletRepository::new(NETWORK);
    // A nonce and nothing else.
    wallets.get_or_create(&address(WALLET_4)).set_nonce(1);
    // A balance and nothing else.
    wallets.get_or_create(&address(TEAM)).credit(5).unwrap();
    // Created by a lookup and left empty: not listed.
    wallets.get_or_create(&address(WALLET_2));
    // A key and a second key.
    wallets.bind_public_key(&address(WALLET_1.0), key(WALLET_1.1));
    let second = PublicKeyBytes::from_array(std::array::from_fn(|i| if i == 0 { 3 } else { 0xab }));
    wallets
        .get_or_create(&address(WALLET_1.0))
        .set_second_public_key(Some(second));
    // A delegate without a key, whose name needs escapes, ranked and resigned for now.
    let delegate = wallets
        .register_delegate(&address(WALLET_3), "we\"ird\\name\u{1}\t".into())
        .unwrap();
    delegate.rank = Some(2);
    delegate.resigned = Some(Resignation::Temporary);
    // A delegate resigned for good, with votes and no rank.
    wallets.bind_public_key(&address(GENESIS_2.0), key(GENESIS_2.1));
    let delegate = wallets
        .register_delegate(&address(GENESIS_2.0), "genesis_2".into())
        .unwrap();
    delegate.vote_balance = 300;
    delegate.voters = 1;
    delegate.resigned = Some(Resignation::Permanent);
    // A forging delegate that votes, with every counter set.
    wallets.bind_public_key(&address(GENESIS_1.0), key(GENESIS_1.1));
    let delegate = wallets
        .register_delegate(&address(GENESIS_1.0), "genesis_1".into())
        .unwrap();
    delegate.vote_balance = 700;
    delegate.voters = 1;
    delegate.rank = Some(1);
    delegate.produced_blocks = 3;
    delegate.forged_fees = 10;
    delegate.forged_rewards = 20;
    delegate.burned_fees = 1;
    delegate.donations = 2;
    delegate.last_block = Some(BlockId::from_array([7; 32]));
    let wallet = wallets.get_or_create(&address(GENESIS_1.0));
    wallet.credit(1000).unwrap();
    wallet.set_nonce(2);
    let votes: Votes = [
        ("genesis_2".to_owned(), 5000),
        ("genesis_1".to_owned(), 5000),
    ]
    .into_iter()
    .collect();
    wallet.set_votes(&votes);
    wallets
}

/// The `#W` section of [`wallets`]: sorted by the bytes of the address text, so upper case before
/// lower case.
const WALLET_SECTION: &str = concat!(
    "#W\n",
    r#"{"a":"dHrw5YKu2GcERR3hiwc4euftTpnBTNLyJx","pk":"03cd5721eff66053eb2d7d38c63044be5ea3e7049eb5b22c91ed88e141e1abcee2","bal":"1000","n":"2","u":"genesis_1","vb":"700","vc":1,"rk":1,"pb":3,"ff":"10","fr":"20","bf":"1","dn":"2","lb":"0707070707070707070707070707070707070707070707070707070707070707","v":[["genesis_1",5000],["genesis_2",5000]]}"#,
    "\n",
    r#"{"a":"dKVpLt8jZk4PTGPE1Sr7dCfbRmDd37GZ1r","pk":"02f060d6f4d82a10865296502171b136118e33b3830cfeaed06ec7a975028c4968","bal":"0","n":"0","u":"genesis_2","vb":"300","vc":1,"rs":1,"pb":0,"ff":"0","fr":"0","bf":"0","dn":"0"}"#,
    "\n",
    r#"{"a":"dM1UwvdL4GXh1SR8YwbqSJinwno2HowKJN","bal":"0","n":"0","u":"we\"ird\\name\u0001\t","vb":"0","vc":0,"rk":2,"rs":0,"pb":0,"ff":"0","fr":"0","bf":"0","dn":"0"}"#,
    "\n",
    r#"{"a":"dPPBDPkvZsz1xs52r1cToidEbdmzbYpCSB","pk":"02a109e32065ed1f48b49723f7e3d844dc84c2dc2f7fdba7cd2aecfc578fca10af","bal":"0","n":"0","spk":"03abababababababababababababababababababababababababababababababab"}"#,
    "\n",
    r#"{"a":"daTBxkSJk2tZhujYcYSQtxj5RRFZ8HcxW8","bal":"5","n":"0"}"#,
    "\n",
    r#"{"a":"db8LAnj9J5r8WHQWRjzDvBHuRaK58Tk67J","bal":"0","n":"1"}"#,
    "\n",
);

const GENESIS_BLOCK_SECTION: &str = concat!(
    "#B\n",
    r#"{"h":1,"id":"fbdd5afb435e34729406ebc7d385727237bf386440799270536c39b9ee45605e","ts":0,"gen":"0334f21b095dbab8c1c602ff0df73371605dd9b79193eb55c89b50d86be0defdea","reward":"0","fee":"0","burned":"0","don":[]}"#,
    "\n",
);

const SUPPLY_SECTION: &str = "#S\n{\"issued\":\"10000000000000000\",\"supply\":\"1005\"}\n";

const MISSED_SECTION: &str = "#M\n[16,\"genesis_3\"]\n[-8,\"genesis_1\"]\n";

/// An `#R` section, written by hand: the hashes do not depend on how a section was made.
const ROUND_SECTION: &str = concat!(
    "#R\n",
    r#"{"round":1,"rh":1,"order":["03cd5721eff66053eb2d7d38c63044be5ea3e7049eb5b22c91ed88e141e1abcee2","02f060d6f4d82a10865296502171b136118e33b3830cfeaed06ec7a975028c4968"],"prod":[],"active":[["02f060d6f4d82a10865296502171b136118e33b3830cfeaed06ec7a975028c4968","300"],["03cd5721eff66053eb2d7d38c63044be5ea3e7049eb5b22c91ed88e141e1abcee2","700"]]}"#,
    "\n",
);

#[test]
fn wallet_records_keys_order_and_escapes() {
    assert_eq!(wallet_section(&wallets()), WALLET_SECTION);
    assert_eq!(
        wallet_section(&WalletRepository::new(NETWORK)),
        "#W\n",
        "no wallet, no record"
    );
}

#[test]
fn a_wallet_back_to_empty_is_not_listed() {
    let mut wallets = WalletRepository::new(NETWORK);
    let team = address(TEAM);
    wallets.get_or_create(&team).credit(5).unwrap();
    wallets.get_or_create(&team).debit(5).unwrap();
    wallets.bind_public_key(&address(WALLET_1.0), key(WALLET_1.1));
    wallets.forget_public_key(&address(WALLET_1.0));
    let votes: Votes = [("genesis_1".to_owned(), 10_000)].into_iter().collect();
    wallets.get_or_create(&address(WALLET_4)).set_votes(&votes);
    wallets
        .get_or_create(&address(WALLET_4))
        .set_votes(&Votes::new());
    assert_eq!(wallets.len(), 3);
    assert_eq!(wallet_section(&wallets), "#W\n");
}

#[test]
fn block_record_of_the_genesis_block() {
    let (milestones, genesis) = devnet();
    let block = factory::from_json(&genesis, &milestones).unwrap();
    assert_eq!(block_section(&block, &milestones), GENESIS_BLOCK_SECTION);
}

/// A block at height 2 signed by a test key, with `reward` and no transactions.
fn reward_block(reward: u64, milestones: &Milestones) -> Block {
    let keys = KeyPair::from_passphrase("digest test forger").unwrap();
    let draft = BlockDraft {
        version: 0,
        timestamp: 8,
        height: 2,
        previous_block: [0; 32],
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward,
        payload_length: 0,
        payload_hash: sha256_concat([]),
    };
    factory::make(draft, Vec::new(), &keys, Aux::fixed([0x42; 32]), milestones).unwrap()
}

#[test]
fn block_record_sorts_donations_and_keeps_zero_amounts() {
    let (milestones, _) = devnet();
    let placeholder_1 = "dZDbtMv86KMqo8iPMtAJLhoT5BVKfyVokA";
    let placeholder_2 = "dPiLD2Fi1dnQhnskBkSnDEPraaWtbTcYPj";
    let block = reward_block(180_000_000, &milestones);
    // The milestone lists the recipients in the other order.
    let listed: Vec<String> = block
        .donations(&milestones)
        .iter()
        .map(|(address, _)| address.to_base58())
        .collect();
    assert_eq!(listed, [placeholder_1, placeholder_2]);
    let expected = format!(
        "#B\n{{\"h\":2,\"id\":\"{}\",\"ts\":8,\"gen\":\"{}\",\"reward\":\"180000000\",\"fee\":\"0\",\
         \"burned\":\"0\",\"don\":[[\"{placeholder_2}\",\"9000000\"],[\"{placeholder_1}\",\"9000000\"]]}}\n",
        block.id().to_hex(),
        block.header().generator_public_key.to_hex(),
    );
    assert_eq!(block_section(&block, &milestones), expected);

    let block = reward_block(19, &milestones);
    let expected = format!(
        "#B\n{{\"h\":2,\"id\":\"{}\",\"ts\":8,\"gen\":\"{}\",\"reward\":\"19\",\"fee\":\"0\",\
         \"burned\":\"0\",\"don\":[[\"{placeholder_2}\",\"0\"],[\"{placeholder_1}\",\"0\"]]}}\n",
        block.id().to_hex(),
        block.header().generator_public_key.to_hex(),
    );
    assert_eq!(block_section(&block, &milestones), expected);
}

#[test]
fn supply_and_missed_records() {
    let totals = Totals {
        issued: 10_000_000_000_000_000,
        rewards: 7,
        burned_fees: 3,
        burned_amounts: 4,
    };
    assert_eq!(supply_section(&totals, &wallets()), SUPPLY_SECTION);
    let missed = [
        MissedBlock {
            timestamp: 16,
            height: 5,
            username: "genesis_3".into(),
        },
        MissedBlock {
            timestamp: -8,
            height: 5,
            username: "genesis_1".into(),
        },
    ];
    assert_eq!(missed_section(&missed), MISSED_SECTION);
    assert_eq!(missed_section(&[]), "#M\n");
}

#[test]
fn section_height_and_chain_hashes() {
    let sections = Sections {
        block: GENESIS_BLOCK_SECTION.into(),
        supply: SUPPLY_SECTION.into(),
        round: ROUND_SECTION.into(),
        missed: MISSED_SECTION.into(),
        wallets: WALLET_SECTION.into(),
    };
    let hashes = sections.hashes();
    let hex = |bytes: &[u8; 32]| heartwood_crypto::utils::hex::encode(bytes);
    assert_eq!(
        hex(&hashes.block),
        "9ecf8ccc6e89b7ed4a07149890d4d1dea23985eefde5876b292655943bf0abb8"
    );
    assert_eq!(
        hex(&hashes.supply),
        "a530a937bbd49246d1507c175a6835a084f86b1088080af4d0b69462b9890a87"
    );
    assert_eq!(
        hex(&hashes.round),
        "799c3825f8b85658516e4912dcd4de74fa405f76be141eb78feec946c44aebff"
    );
    assert_eq!(
        hex(&hashes.missed),
        "865a6f95ee5001fc55f304ddf66eeebcb9a5b38f14d68e7da10e6ffd611e8104"
    );
    assert_eq!(
        hex(&hashes.wallets),
        "2bd68ff645d37f1304ecc048129ebf11ca2b518e1ff7ca10d71d04dacd486b5d"
    );
    let state_digest = hashes.state_digest();
    assert_eq!(
        hex(&state_digest),
        "fc62fe73091921de7b34b39a78b4ff46b55ef5e28affff9de4a851f7cc811c54"
    );
    let first = chain_digest(&[0; 32], &state_digest);
    assert_eq!(
        hex(&first),
        "b5379c29a1377d06d840a78183555af2d961b209ae177288139d22c2b3c05946"
    );
    let second = chain_digest(&first, &state_digest);
    assert_ne!(second, first, "the running digest moves on every line");

    let line = DigestLine {
        height: 1,
        step: Step::Apply,
        id: BlockId::from_array([0xfb; 32]),
        reward: Some(180_000_000),
        sections: hashes,
        state_digest,
        chain_digest: first,
    };
    let expected = format!(
        "{{\"h\":1,\"step\":\"apply\",\"id\":\"{}\",\"rw\":\"180000000\",\"b\":\"{}\",\"s\":\"{}\",\
         \"r\":\"{}\",\"m\":\"{}\",\"w\":\"{}\",\"sd\":\"{}\",\"cd\":\"{}\"}}",
        "fb".repeat(32),
        hex(&hashes.block),
        hex(&hashes.supply),
        hex(&hashes.round),
        hex(&hashes.missed),
        hex(&hashes.wallets),
        hex(&state_digest),
        hex(&first),
    );
    assert_eq!(line.to_json(), expected);
    let genesis = DigestLine {
        step: Step::Genesis,
        reward: None,
        ..line
    };
    assert!(!genesis.to_json().contains("\"rw\""));
    assert!(genesis.to_json().contains("\"step\":\"genesis\""));
}
