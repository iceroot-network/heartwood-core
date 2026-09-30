//! The index orders of the wallet repository: the reference's insertion orders, the re-indexing
//! its handlers run after each block, and the one order of a repository read back from rows; then
//! the reference's own orders over whole chains, the records of kind `state.indexOrders` of vector
//! class V34.
//!
//! The class is read from `tests/vectors`, or from the directory in `STATE_VECTORS` (a relative
//! path is taken from the workspace root), such as a local oracle run in `target/vectors/local`.
//! The crate's directory must hold the class with at least the kind's minimum of records
//! (`tools/oracle/vector-minimums.json`), so a missing class fails the test; a directory named in
//! the environment may lack it, and the test then says so and passes. Each record names a corpus
//! of the differential tests and a line of its replay: this crate replays the corpus in memory,
//! with its revert plan, and compares the live orders at the line and the orders of a replay of
//! the same blocks without reverts; the restart paths, which need the store, are compared in the
//! differential tests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::{sha256, sha256_concat};
use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::TransferItem;
use heartwood_crypto::{Aux, Block, KeyPair, Milestones, Network, Transaction};
use heartwood_state::State;
use heartwood_state::wallets::{Delegate, Wallet, WalletRepository};
use heartwood_transactions::{Handlers, minimum_fee};
use serde_json::{Value, json};

const V10: &str = include_str!("../../crypto/tests/vectors/V10-genesis.jsonl");
const NETWORK: u8 = 90;
const SXP: u64 = 100_000_000;

/// The devnet chain of the genesis vectors, with its keys by delegate name and wallet role.
struct Chain {
    milestones: Milestones,
    network: u8,
    state: State,
    keys: BTreeMap<String, KeyPair>,
}

impl Chain {
    fn new() -> Chain {
        let record: Value = V10
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|record| record["op"] == "genesis.generate")
            .unwrap();
        let output = &record["output"];
        let file = |name: &str| output["files"][name].as_str().unwrap().to_owned();
        let network = Network::load(&file("crypto/network.json")).unwrap();
        let milestones = Milestones::load(&file("crypto/milestones.json"), &network).unwrap();
        let json: Value = serde_json::from_str(&file("crypto/genesisBlock.json")).unwrap();
        let genesis = factory::from_json(&json, &milestones).unwrap();
        let state = State::from_genesis(&genesis, &milestones, &Handlers).unwrap();
        let mut keys = BTreeMap::new();
        for (list, name) in [
            ("delegates", "username"),
            ("genesisWallets", "role"),
            ("testWallets", "role"),
        ] {
            for entry in output[list].as_array().unwrap() {
                let passphrase = entry["passphrase"].as_str().unwrap();
                let pair = KeyPair::from_passphrase(passphrase).unwrap();
                keys.insert(entry[name].as_str().unwrap().to_owned(), pair);
            }
        }
        Chain {
            network: network.pub_key_hash,
            milestones,
            state,
            keys,
        }
    }

    fn address(&self, name: &str) -> Address {
        Address::from_public_key(self.keys[name].public_key(), self.network)
    }

    fn sign(&self, name: &str, ahead: u64, builder: TransactionBuilder) -> Transaction {
        let wallets = self.state.wallets();
        let nonce = wallets
            .find_by_address(&self.address(name))
            .map_or(0, Wallet::nonce);
        let builder = builder.nonce(nonce + 1 + ahead);
        let params = self.milestones.at(self.state.height() + 1);
        let aux = Aux::fixed([0x5a; 32]);
        let transaction = builder
            .clone()
            .build(&self.keys[name], aux, params)
            .unwrap();
        let floor = u64::try_from(minimum_fee(&transaction, params)).unwrap();
        let fee = transaction.fee().max(floor);
        builder
            .fee(fee)
            .build(&self.keys[name], aux, params)
            .unwrap()
    }

    fn transfer(&self, from: &str, ahead: u64, to: &[(&str, u64)]) -> Transaction {
        let items = to
            .iter()
            .map(|&(name, amount)| TransferItem {
                amount,
                recipient: self.address(name),
            })
            .collect();
        self.sign(from, ahead, TransactionBuilder::transfer(items))
    }

    fn register(&self, from: &str, username: &str) -> Transaction {
        self.sign(from, 0, TransactionBuilder::delegate_registration(username))
    }

    /// The next block, in the next slot, forged by the delegate the slot schedules.
    fn block(&self, transactions: Vec<Transaction>) -> Block {
        let block_time = u32::try_from(self.milestones.at(1).block_time()).unwrap();
        let last = self.state.last_block();
        let timestamp = last.header().timestamp + block_time;
        let forging = self.state.forging_delegates();
        let slot = usize::try_from(timestamp / block_time).unwrap();
        let keys = &self.keys[&forging[slot % forging.len()].username];
        let height = last.height() + 1;
        let generator = keys.public_key().to_compressed();
        let reward = (self.state)
            .reward_for_block_in_round(height, &generator, &self.milestones)
            .map_or(0, |reward| reward.reward);
        let count = u32::try_from(transactions.len()).unwrap();
        let ids = transactions.iter().map(|t| t.id().as_bytes().as_slice());
        let draft = BlockDraft {
            version: 0,
            timestamp,
            height,
            previous_block: *last.id().as_bytes(),
            number_of_transactions: count,
            total_amount: transactions.iter().map(Transaction::amount).sum(),
            total_fee: transactions.iter().map(Transaction::fee).sum(),
            reward,
            payload_length: 32 * count,
            payload_hash: sha256_concat(ids),
        };
        let aux = Aux::fixed([0x42; 32]);
        factory::make(draft, transactions, keys, aux, &self.milestones).unwrap()
    }

    fn apply(&mut self, transactions: Vec<Transaction>) -> Block {
        let block = self.block(transactions);
        (self.state)
            .apply_block(&block, &self.milestones, &Handlers)
            .unwrap();
        block
    }

    fn revert(&mut self) {
        (self.state)
            .revert_block(&self.milestones, &Handlers)
            .unwrap();
    }

    fn reapply(&mut self, block: &Block) {
        (self.state)
            .apply_block(block, &self.milestones, &Handlers)
            .unwrap();
    }

    fn addresses(&self) -> Vec<Address> {
        (self.state.wallets().by_address_index())
            .map(|wallet| *wallet.address())
            .collect()
    }

    fn usernames(&self) -> Vec<String> {
        (self.state.wallets().by_username_index())
            .filter_map(Wallet::delegate)
            .map(|delegate| delegate.username().to_owned())
            .collect()
    }

    /// Where `name`'s address stands in the address index, if it is there.
    fn position(&self, name: &str) -> Option<usize> {
        let address = self.address(name);
        self.addresses().iter().position(|a| *a == address)
    }
}

fn key(passphrase: &str) -> (PublicKey, Address) {
    let keys = KeyPair::from_passphrase(passphrase).unwrap();
    let public_key = keys.public_key().clone();
    let address = Address::from_public_key(&public_key, NETWORK);
    (public_key, address)
}

fn names(wallets: &WalletRepository) -> Vec<&str> {
    (wallets.by_username_index())
        .filter_map(Wallet::delegate)
        .map(Delegate::username)
        .collect()
}

#[test]
fn keys_and_names_keep_the_order_they_were_set_in() {
    let mut wallets = WalletRepository::new(NETWORK);
    let [(a_key, a), (b_key, b), (c_key, c)] = ["a", "b", "c"].map(key);
    for (address, public_key) in [(&c, &c_key), (&a, &a_key), (&b, &b_key)] {
        wallets.bind_public_key(address, public_key.clone());
    }
    let order = |wallets: &WalletRepository| -> Vec<Address> {
        (wallets.by_public_key_index())
            .map(|wallet| *wallet.address())
            .collect()
    };
    assert_eq!(order(&wallets), [c, a, b]);
    assert_eq!(
        wallets
            .by_address_index()
            .map(Wallet::address)
            .collect::<Vec<_>>(),
        [&c, &a, &b]
    );

    // A key forgotten and set again goes to the end.
    wallets.forget_public_key(&c);
    wallets.bind_public_key(&c, c_key);
    assert_eq!(order(&wallets), [a, b, c]);

    // Names: registration order, not name order; a name forgotten and registered again moves to
    // the end, while the name-ordered iteration of the ranking stays as it was.
    for (address, name) in [(&b, "zeta"), (&c, "alpha"), (&a, "mid")] {
        wallets.register_delegate(address, name.into()).unwrap();
    }
    assert_eq!(names(&wallets), ["zeta", "alpha", "mid"]);
    wallets.forget_delegate(&b);
    wallets.register_delegate(&b, "zeta".into()).unwrap();
    assert_eq!(names(&wallets), ["alpha", "mid", "zeta"]);
    wallets.forget_delegate(&c);
    wallets.register_delegate(&c, "alpha".into()).unwrap();
    assert_eq!(names(&wallets), ["mid", "zeta", "alpha"]);
    let by_name: Vec<&str> = wallets
        .indexes()
        .usernames()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(by_name, ["alpha", "mid", "zeta"]);
    let delegates: Vec<&Address> = wallets.delegates().map(Wallet::address).collect();
    assert_eq!(delegates, [&c, &a, &b]);
}

#[test]
fn a_lookup_sets_the_address_and_the_first_use_order_stays() {
    let mut wallets = WalletRepository::new(NETWORK);
    let [(_, a), (b_key, b), (_, c)] = ["a", "b", "c"].map(key);
    for address in [&a, &b, &c] {
        wallets.get_or_create(address);
    }
    // Binding a key re-indexes the wallet; a lookup of a wallet in the index keeps its place.
    wallets.bind_public_key(&b, b_key);
    wallets.get_or_create(&a).credit(1).unwrap();
    let by_index: Vec<&Address> = wallets.by_address_index().map(Wallet::address).collect();
    assert_eq!(by_index, [&a, &c, &b]);
    let first_use: Vec<&Address> = wallets.iter().map(Wallet::address).collect();
    assert_eq!(first_use, [&a, &b, &c]);
}

#[test]
fn rows_read_back_set_all_three_indexes_in_their_one_order() {
    let [(a_key, a), (b_key, b), (_, c)] = ["a", "b", "c"].map(key);
    let mut wallets = WalletRepository::new(NETWORK);
    wallets.bind_public_key(&a, a_key);
    wallets.bind_public_key(&b, b_key);
    wallets.register_delegate(&a, "first".into()).unwrap();
    wallets.register_delegate(&b, "second".into()).unwrap();
    wallets.get_or_create(&c).credit(5).unwrap();
    let mut rows: Vec<Wallet> = wallets.iter().cloned().collect();
    rows.reverse();
    let restored = WalletRepository::from_wallets(NETWORK, rows).unwrap();
    let order: Vec<&Address> = restored.by_address_index().map(Wallet::address).collect();
    assert_eq!(order, [&c, &b, &a]);
    let keys: Vec<&Address> = restored
        .by_public_key_index()
        .map(Wallet::address)
        .collect();
    assert_eq!(keys, [&b, &a]);
    assert_eq!(names(&restored), ["second", "first"]);
    // Equality ignores the orders.
    assert_eq!(restored, wallets);
}

#[test]
fn a_block_moves_its_senders_and_recipients_to_the_end_in_transaction_order() {
    let mut chain = Chain::new();
    let funds = chain.transfer(
        "team-placeholder-1",
        0,
        &[("wallet/1", 100 * SXP), ("wallet/2", 100 * SXP)],
    );
    chain.apply(vec![funds]);
    let before = chain.addresses();
    let [two, delegate, one] = ["wallet/2", "genesis_3", "wallet/1"].map(|n| chain.address(n));
    let first = chain.transfer("wallet/2", 0, &[("genesis_3", SXP), ("wallet/1", SXP)]);
    let second = chain.transfer("wallet/1", 0, &[("wallet/2", SXP)]);
    chain.apply(vec![first, second]);
    let after = chain.addresses();
    // The re-indexes after the block run in queue order: wallet/2, genesis_3, wallet/1, then
    // wallet/1 and wallet/2 again; each moves to the end, so the last one queued is last.
    assert_eq!(after[after.len() - 3..], [delegate, one, two]);
    let moved = [two, delegate, one];
    let kept: Vec<Address> = before.into_iter().filter(|a| !moved.contains(a)).collect();
    assert_eq!(after[..after.len() - 3], kept);
    // The delegate's name moved with its wallet.
    assert_eq!(
        chain.usernames().last().map(String::as_str),
        Some("genesis_3")
    );
}

#[test]
fn a_registration_reverted_and_applied_again_moves_to_the_end() {
    let mut chain = Chain::new();
    let funds = chain.transfer("team-placeholder-1", 0, &[("wallet/1", 100 * SXP)]);
    chain.apply(vec![funds]);
    let registration = chain.apply(vec![chain.register("wallet/1", "newcomer")]);
    assert_eq!(
        chain.usernames().last().map(String::as_str),
        Some("newcomer")
    );
    // A transfer to a delegate moves its name after the newcomer's.
    chain.apply(vec![chain.transfer(
        "team-placeholder-1",
        0,
        &[("genesis_1", SXP)],
    )]);
    let names = chain.usernames();
    assert_eq!(names[names.len() - 2..], ["newcomer", "genesis_1"]);

    chain.revert();
    chain.revert();
    assert!(!chain.usernames().iter().any(|name| name == "newcomer"));
    assert_eq!(
        chain.usernames().last().map(String::as_str),
        Some("genesis_1")
    );
    chain.reapply(&registration);
    let names = chain.usernames();
    assert_eq!(names[names.len() - 2..], ["genesis_1", "newcomer"]);
}

#[test]
fn a_wallet_a_revert_empties_leaves_the_address_index_and_a_lookup_sets_it_again() {
    let mut chain = Chain::new();
    let block = chain.apply(vec![chain.transfer(
        "team-placeholder-1",
        0,
        &[("wallet/7", SXP)],
    )]);
    let wallet = chain.address("wallet/7");
    assert_eq!(chain.addresses().last(), Some(&wallet));
    chain.revert();
    assert_eq!(chain.position("wallet/7"), None);
    // The repository keeps the empty wallet, as before, outside the index.
    assert!(chain.state.wallets().has_by_address(&wallet));
    chain.reapply(&block);
    assert_eq!(chain.addresses().last(), Some(&wallet));
}

#[test]
fn a_revert_of_a_round_end_re_indexes_the_round_from_its_last_block_back() {
    let mut chain = Chain::new();
    let round = u32::try_from(chain.milestones.at(1).active_delegates()).unwrap();
    // The genesis block funds both senders, so the round's only other transactions are these.
    for (from, to) in [("genesis-1", "wallet/3"), ("genesis-2", "wallet/4")] {
        chain.apply(vec![chain.transfer(from, 0, &[(to, SXP)])]);
    }
    while chain.state.height() + 1 < round {
        chain.apply(Vec::new());
    }
    chain.apply(vec![chain.transfer(
        "team-placeholder-1",
        0,
        &[("wallet/5", SXP)],
    )]);
    assert_eq!(chain.state.height(), round);
    let position = |chain: &Chain, name: &str| chain.position(name).unwrap();
    assert!(position(&chain, "genesis-1") < position(&chain, "genesis-2"));

    // Reverting the round's last block re-indexes the round's transactions from that block back,
    // so the earlier block's wallets end after the later block's, then the block's own again.
    chain.revert();
    assert!(position(&chain, "genesis-2") < position(&chain, "genesis-1"));
    assert!(position(&chain, "wallet/4") < position(&chain, "wallet/3"));
    assert!(position(&chain, "wallet/3") < position(&chain, "team-placeholder-1"));
    assert_eq!(chain.position("wallet/5"), None);
}

const CLASS: &str = "V34-api-tiers";
const KIND: &str = "state.indexOrders";

/// The workspace root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The records of kind `state.indexOrders`, or `None` when the directory `STATE_VECTORS` names
/// lacks the class. The crate's own directory must hold it.
fn records() -> Option<Vec<Value>> {
    let named = std::env::var_os("STATE_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors"),
        |dir| root().join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    Some(lines.filter(|record| record["op"] == KIND).collect())
}

/// A corpus of the differential tests: its chain definition, its blocks from height 2 and the
/// block counts its revert plan reverts after each height.
struct Corpus {
    milestones: Milestones,
    genesis: Block,
    blocks: Vec<Vec<u8>>,
    reverts: BTreeMap<u32, Vec<u32>>,
}

impl Corpus {
    /// The corpus `name` of the repository whose manifest has the SHA-256 `manifest`.
    fn load(name: &str, manifest: &str) -> Corpus {
        let dir = ["tools/difftest/corpus", "target/corpus"]
            .map(|dir| root().join(dir).join(name))
            .into_iter()
            .find(|dir| dir.join("corpus.json").is_file())
            .unwrap_or_else(|| panic!("no corpus {name} in this tree"));
        let read = |file: &str| std::fs::read_to_string(dir.join(file)).unwrap();
        assert_eq!(
            hex::encode(sha256(read("MANIFEST.sha256").as_bytes())),
            manifest
        );
        let network = Network::load(&read("crypto/network.json")).unwrap();
        let milestones = Milestones::load(&read("crypto/milestones.json"), &network).unwrap();
        let genesis: Value = serde_json::from_str(&read("crypto/genesisBlock.json")).unwrap();
        let genesis = factory::from_json(&genesis, &milestones).unwrap();
        let blocks = (read("blocks.jsonl").lines())
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .map(|block| hex::decode(block["hex"].as_str().unwrap()).unwrap())
            .collect();
        let plan: Value = serde_json::from_str(&read("revert-plan.json")).unwrap();
        let mut reverts: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for step in plan["steps"].as_array().unwrap() {
            let after = u32::try_from(step["after"].as_u64().unwrap()).unwrap();
            let blocks = u32::try_from(step["blocks"].as_u64().unwrap()).unwrap();
            reverts.entry(after).or_default().push(blocks);
        }
        Corpus {
            milestones,
            genesis,
            blocks,
            reverts,
        }
    }

    fn block(&self, height: u32) -> Block {
        let bytes = &self.blocks[usize::try_from(height - 2).unwrap()];
        Block::receive(bytes, &self.milestones).unwrap()
    }

    fn state(&self) -> State {
        State::from_genesis(&self.genesis, &self.milestones, &Handlers).unwrap()
    }

    /// The replay's steps after the first start, as the reference's replay runs them: each block
    /// applied, then each revert-plan step after it, its blocks reverted one at a time and applied
    /// again (`None` for a revert, else the height applied).
    fn steps(&self) -> Vec<Option<u32>> {
        let tip = u32::try_from(self.blocks.len()).unwrap() + 1;
        let mut steps = Vec::new();
        for height in 2..=tip {
            steps.push(Some(height));
            for &count in self.reverts.get(&height).into_iter().flatten() {
                steps.extend(std::iter::repeat_n(None, usize::try_from(count).unwrap()));
                steps.extend((height - count + 1..=height).map(Some));
            }
        }
        steps
    }
}

/// The three orders of `wallets`, in the oracle's shape.
fn orders(wallets: &WalletRepository) -> Value {
    let addresses: Vec<String> = (wallets.by_address_index())
        .map(|wallet| wallet.address().to_base58())
        .collect();
    let keys: Vec<String> = (wallets.by_public_key_index())
        .filter_map(Wallet::public_key)
        .map(PublicKey::to_hex)
        .collect();
    json!({ "addresses": addresses, "publicKeys": keys, "usernames": names(wallets) })
}

/// Where `ours` first differs from `theirs`, the reference's orders, or `None` when they agree.
fn difference(ours: &Value, theirs: &Value) -> Option<String> {
    ["addresses", "publicKeys", "usernames"]
        .into_iter()
        .find_map(|list| {
            let (ours, theirs) = (ours[list].as_array()?, theirs[list].as_array()?);
            let at = (0..ours.len().max(theirs.len())).find(|&i| ours.get(i) != theirs.get(i))?;
            Some(format!(
                "{list}[{at}]: {:?}, the reference {:?}",
                ours.get(at),
                theirs.get(at)
            ))
        })
}

#[test]
fn the_live_and_replayed_orders_are_the_reference_s() {
    let Some(records) = records() else {
        eprintln!("the directory STATE_VECTORS names has no {CLASS}");
        return;
    };
    let minimums: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tools/oracle/vector-minimums.json")).unwrap(),
    )
    .unwrap();
    let minimum = usize::try_from(minimums[CLASS][KIND].as_u64().unwrap()).unwrap();
    assert!(
        records.len() >= minimum,
        "{KIND}: {} records, the minimum is {minimum}",
        records.len()
    );
    let mut by_corpus: BTreeMap<(&str, &str), Vec<&Value>> = BTreeMap::new();
    for record in &records {
        let input = &record["input"];
        let key = (
            input["corpus"].as_str().unwrap(),
            input["manifest"].as_str().unwrap(),
        );
        by_corpus.entry(key).or_default().push(record);
    }
    for ((name, manifest), records) in by_corpus {
        let corpus = Corpus::load(name, manifest);
        let by_line: BTreeMap<u64, &Value> = (records.iter())
            .map(|record| (record["input"]["line"].as_u64().unwrap(), &record["output"]))
            .collect();
        let heights: BTreeSet<u64> = by_line
            .values()
            .map(|out| out["height"].as_u64().unwrap())
            .collect();

        // The live orders: the replay with its reverts, compared at each record's line.
        let mut state = corpus.state();
        let check_line = |state: &State, line: u64| {
            if let Some(output) = by_line.get(&line) {
                assert_eq!(
                    u64::from(state.height()),
                    output["height"],
                    "{name}, line {line}"
                );
                let live = difference(&orders(state.wallets()), &output["live"]);
                assert_eq!(live, None, "{name}, line {line}: the live orders");
            }
        };
        check_line(&state, 1);
        for (line, step) in (2..).zip(corpus.steps()) {
            if line > *by_line.keys().last().unwrap() {
                break;
            }
            match step {
                Some(height) => {
                    let block = corpus.block(height);
                    state
                        .apply_block(&block, &corpus.milestones, &Handlers)
                        .unwrap();
                }
                None => {
                    state.revert_block(&corpus.milestones, &Handlers).unwrap();
                }
            }
            check_line(&state, line);
        }

        // The replayed orders: the same blocks applied once each, in height order.
        let mut state = corpus.state();
        for height in 2..=u32::try_from(*heights.last().unwrap()).unwrap() {
            state
                .apply_block(&corpus.block(height), &corpus.milestones, &Handlers)
                .unwrap();
            if heights.contains(&u64::from(height)) {
                let replayed = orders(state.wallets());
                for output in by_line.values().filter(|out| out["height"] == height) {
                    let replayed = difference(&replayed, &output["replayed"]);
                    assert_eq!(
                        replayed, None,
                        "{name}, height {height}: the replayed orders"
                    );
                }
            }
        }
    }
}
