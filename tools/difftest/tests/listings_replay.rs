//! The listings replay: the pool traffic and the blocks of each node of a saved split mixed run
//! (five relays, two of them Heartwood's, partitioned 3 and 2 and healed), replayed headless, to
//! settle where a wallet listing's tie order comes from.
//!
//! The fixture (`fixtures/listings-replay/split.json`) holds, for four nodes (the majority's pair
//! node 1 and node 3, the minority's pair node 2 and node 5), every step the node took in time
//! order: each pool submission it received, with its live verdict where it was sent; each block
//! as it held it, a revert where it left the minority's branch at the heal; and a validating
//! collation at each slot of its own delegates. It also holds the first page of the wallet listing
//! each node served at the end.
//!
//! - Through Heartwood alone (always run): each node's steps give the verdicts the node gave live
//!   and the listing page it served, the tie places included; the majority's two orders differ
//!   only by the parties of the refusals one node alone saw, and without them they are one order.
//! - Through both implementations (ignored for the reason `integration`, with the oracle of
//!   `DIFFTEST_TEST_SOLAR_DIR` or `DIFFTEST_TEST_IMAGE`): after every step the reference and
//!   Heartwood hold the same address, public-key and username orders and give the same verdicts,
//!   through first sight, removal and re-indexing.
//! - The listings' judge with a run's evidence: a wallet whose place a node's own refusals moved is
//!   free inside its tie; any other order difference still fails.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use heartwood_crypto::Transaction;
use heartwood_crypto::blocks::factory;
use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::utils::hex;
use heartwood_difftest::levels::l2::call;
use heartwood_difftest::lockstep::heart::Heart;
use heartwood_difftest::oracle::{Mode, Oracle, OracleServer};
use heartwood_kernel::config::crypto::{self, CryptoTexts};
use heartwood_kernel::utils::round_calculator::{calculate_round, is_new_round};
use heartwood_state::digest::Step as DigestStep;
use heartwood_state::wallets::parties;
use serde_json::{Value, json};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/listings-replay/split.json")
}

/// The fixture, read once per test.
struct Fixture {
    raw: Value,
    chain: Vec<Vec<u8>>,
    branch_from: u32,
    branch: Vec<Vec<u8>>,
    transactions: Vec<Vec<u8>>,
}

fn bytes_of(list: &Value) -> Vec<Vec<u8>> {
    list.as_array()
        .expect("a list of hex")
        .iter()
        .map(|h| hex::decode(h.as_str().expect("hex")).expect("hex"))
        .collect()
}

impl Fixture {
    fn load() -> Fixture {
        let text = std::fs::read_to_string(fixture_path()).expect("the fixture");
        let raw: Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(raw["format"], "heartwood-listings-replay/1");
        Fixture {
            chain: bytes_of(&raw["chain"]),
            branch_from: u32::try_from(raw["branch"]["from"].as_u64().expect("from")).expect("u32"),
            branch: bytes_of(&raw["branch"]["blocks"]),
            transactions: bytes_of(&raw["transactions"]),
            raw,
        }
    }

    fn steps(&self, node: &str) -> Vec<Value> {
        self.raw["views"][node]["steps"]
            .as_array()
            .expect("the node's steps")
            .clone()
    }

    fn page(&self, node: &str) -> Vec<String> {
        (self.raw["pages"][node].as_array().expect("a page").iter())
            .map(|a| a.as_str().expect("an address").to_owned())
            .collect()
    }

    fn block(&self, chain: &str, height: u32) -> &[u8] {
        let (list, from) = match chain {
            "majority" => (&self.chain, 2),
            "minority" => (&self.branch, self.branch_from),
            other => panic!("no chain {other}"),
        };
        (height.checked_sub(from))
            .and_then(|i| list.get(usize::try_from(i).ok()?))
            .unwrap_or_else(|| panic!("no block {height} of the {chain}"))
    }

    /// The sender and the recipients of transaction `index`, as the handlers re-index them.
    fn parties(&self, heart: &Heart, index: usize) -> Vec<String> {
        let params = heart.milestones.at(heart.height() + 1);
        let transaction =
            Transaction::decode(&self.transactions[index], params).expect("a transaction");
        parties(&transaction, heart.network.pub_key_hash)
            .map(|a| a.to_base58())
            .collect()
    }
}

/// The three index orders of a state: addresses (base58), public keys (hex), usernames.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Orders {
    addresses: Vec<String>,
    public_keys: Vec<String>,
    usernames: Vec<String>,
}

impl Orders {
    fn of_reference(answer: &Value) -> Orders {
        let list = |key: &str| -> Vec<String> {
            (answer[key].as_array().expect("an order").iter())
                .map(|v| v.as_str().expect("a key").to_owned())
                .collect()
        };
        Orders {
            addresses: list("addresses"),
            public_keys: list("publicKeys"),
            usernames: list("usernames"),
        }
    }

    fn first_difference(&self, other: &Orders) -> Option<String> {
        for (name, a, b) in [
            ("address", &self.addresses, &other.addresses),
            ("public-key", &self.public_keys, &other.public_keys),
            ("username", &self.usernames, &other.usernames),
        ] {
            if a != b {
                let at = a
                    .iter()
                    .zip(b)
                    .position(|(x, y)| x != y)
                    .unwrap_or(a.len().min(b.len()));
                return Some(format!(
                    "the {name} orders differ at place {at} ({} and {} keys): {:?} against {:?}",
                    a.len(),
                    b.len(),
                    a.get(at),
                    b.get(at)
                ));
            }
        }
        None
    }
}

/// The verdict of a pool result on the one transaction submitted, `id`: `accept`, or the
/// refusal's code (the processor names an entry given as bytes by its place, `0`).
fn verdict(result: &Value, id: &str) -> String {
    let accepted = (result["accept"].as_array().into_iter().flatten()).any(|a| a == id);
    if accepted {
        return "accept".to_owned();
    }
    let errors = result["errors"].as_object();
    (errors.and_then(|e| e.get(id).or_else(|| e.get("0"))))
        .and_then(|e| e["type"].as_str())
        .unwrap_or("no verdict")
        .to_owned()
}

/// Heartwood's side of a node: its state and pool, as the lockstep producer drives them.
struct HeartSide {
    heart: Heart,
}

impl HeartSide {
    fn open(fixture: &Fixture) -> HeartSide {
        let file = |name: &str| Some(fixture.raw["crypto"][name].to_string());
        let files = crypto::parse(&CryptoTexts {
            network: file("network"),
            milestones: file("milestones"),
            genesis: file("genesisBlock"),
            exceptions: file("exceptions"),
        })
        .expect("the crypto files");
        let genesis = factory::from_json(&files.genesis, &files.milestones).expect("the genesis");
        HeartSide {
            heart: Heart::open(&files, &genesis).expect("Heartwood's state"),
        }
    }

    fn orders(&self) -> Orders {
        let indexes = self.heart.state.wallets().indexes();
        Orders {
            addresses: indexes
                .addresses_in_order()
                .map(Address::to_base58)
                .collect(),
            public_keys: indexes
                .public_keys_in_order()
                .map(|(k, _)| k.to_hex())
                .collect(),
            usernames: indexes
                .usernames_in_order()
                .map(|(u, _)| u.to_owned())
                .collect(),
        }
    }

    /// The first page of `GET /wallets`: the address index's wallets, the highest balance first,
    /// a tie in the index's order.
    fn listing(&self, limit: usize) -> Vec<String> {
        self.listed(limit).into_iter().map(|(a, _)| a).collect()
    }

    /// The same page's wallets with their balances.
    fn listed(&self, limit: usize) -> Vec<(String, u64)> {
        let wallets = self.heart.state.wallets();
        let mut listed: Vec<(String, u64)> = (wallets.indexes().addresses_in_order())
            .map(|a| {
                let balance = wallets.find_by_address(a).map_or(0, |w| w.balance());
                (a.to_base58(), balance)
            })
            .collect();
        listed.sort_by_key(|a| std::cmp::Reverse(a.1));
        listed.truncate(limit);
        listed
    }
}

/// The reference's side of a node: a lockstep session of the oracle.
struct ReferenceSide {
    server: OracleServer,
    session: Value,
}

impl ReferenceSide {
    fn open(fixture: &Fixture, oracle: &Oracle) -> ReferenceSide {
        let mut server = oracle.server().expect("the oracle server");
        let opened = call(
            &mut server,
            "lockstep.open",
            1,
            &json!({"crypto": fixture.raw["crypto"], "options": {}}),
        )
        .expect("lockstep.open");
        ReferenceSide {
            session: opened["session"].clone(),
            server,
        }
    }

    fn ask(&mut self, op: &str, height: u32, mut args: Value) -> Value {
        args["session"] = self.session.clone();
        call(&mut self.server, op, u64::from(height), &args).unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn orders(&mut self, height: u32) -> Orders {
        Orders::of_reference(&self.ask("lockstep.indexOrders", height, json!({})))
    }
}

/// What a replay of one node's steps saw.
#[derive(Debug, Default)]
struct Replayed {
    /// Live verdicts the replay did not give: (transaction, live, Heartwood's, the reference's).
    verdicts: Vec<String>,
    /// The steps after which the two implementations' orders differed.
    disagreements: Vec<String>,
    /// Steps by what they did to the address index: a refusal that moved a wallet, an address
    /// that entered it, an address that left it.
    moved_by_refusal: usize,
    entered: usize,
    left: usize,
    /// The addresses whose place a refusal moved, in the order of the refusals.
    refused_parties: Vec<String>,
    /// The refusals by code: (moved a wallet, moved none).
    refusals: BTreeMap<String, (usize, usize)>,
    /// Steps run.
    steps: usize,
}

/// Replay `node`'s steps through Heartwood, and through the reference when `reference` is given;
/// `skip` names transactions (by index) to leave out.
fn replay(
    fixture: &Fixture,
    node: &str,
    mut reference: Option<&mut ReferenceSide>,
    skip: &BTreeSet<usize>,
) -> (HeartSide, Replayed) {
    let ids: Vec<String> = (fixture.raw["ids"].as_array().expect("ids").iter())
        .map(|v| v.as_str().expect("an id").to_owned())
        .collect();
    let mut heart = HeartSide::open(fixture);
    let mut seen = Replayed::default();
    let online = |heart: &mut HeartSide, reference: &mut Option<&mut ReferenceSide>, next: u32| {
        let milestones = heart.heart.milestones.clone();
        let round = calculate_round(next, &milestones).round;
        let keys: Vec<PublicKeyBytes> = heart.heart.forging_keys();
        heart.heart.set_online(round, &keys);
        if let Some(r) = reference.as_deref_mut() {
            let texts: Vec<String> = keys.iter().map(PublicKeyBytes::to_hex).collect();
            r.ask(
                "lockstep.online",
                next - 1,
                json!({"round": round, "delegates": texts}),
            );
        }
    };
    online(&mut heart, &mut reference, 2);
    let mut before = heart.orders();
    for (n, step) in fixture.steps(node).iter().enumerate() {
        let height = heart.heart.height();
        let mut refused = None;
        let mut submitted = None;
        let label;
        if let Some(chain) = step["apply"].as_str() {
            let h = u32::try_from(step["height"].as_u64().expect("height")).expect("u32");
            assert_eq!(h, height + 1, "{node}: step {n} applies {h} on {height}");
            let bytes = fixture.block(chain, h);
            let posted = heart
                .heart
                .apply(bytes, DigestStep::Apply)
                .unwrap_or_else(|e| {
                    panic!("{node}: Heartwood refuses block {h} of the {chain}: {e}")
                });
            if let Some(r) = reference.as_deref_mut() {
                let mut args = json!({"hex": hex::encode(bytes), "step": "apply"});
                if let Some(round) = posted.expire_at {
                    args["expireAt"] = json!(round);
                }
                let answer = r.ask("lockstep.apply", h, args);
                assert_eq!(
                    answer["accepted"], true,
                    "{node}: the reference refuses block {h}: {answer}"
                );
            }
            if is_new_round(h + 1, &heart.heart.milestones) {
                online(&mut heart, &mut reference, h + 1);
            }
            label = format!("block {h} of the {chain}");
        } else if let Some(count) = step["revert"].as_u64() {
            let count = u32::try_from(count).expect("u32");
            heart.heart.revert(count).expect("Heartwood's revert");
            if let Some(r) = reference.as_deref_mut() {
                r.ask("lockstep.revert", height, json!({"count": count}));
            }
            label = format!("a revert of {count} blocks from {height}");
        } else if step.get("collate").is_some() {
            heart.heart.candidates().expect("Heartwood's collation");
            if let Some(r) = reference.as_deref_mut() {
                r.ask(
                    "pool.collate",
                    height,
                    json!({"validate": true, "exclude": []}),
                );
            }
            label = format!("a collation at {height}");
        } else {
            let index = usize::try_from(step["submit"].as_u64().expect("a step")).expect("usize");
            if skip.contains(&index) {
                continue;
            }
            let now = step["now"].as_i64().expect("now");
            let bytes = fixture.transactions[index].clone();
            let id = &ids[index];
            let ours = heart
                .heart
                .submit(now, vec![bytes.clone()])
                .expect("Heartwood's pool");
            let ours = verdict(&ours, id);
            let theirs = reference.as_deref_mut().map(|r| {
                let answer = r.ask(
                    "lockstep.submit",
                    height,
                    json!({"transactions": [hex::encode(&bytes)], "now": now}),
                );
                verdict(&answer, id)
            });
            let live = step["verdict"].as_str();
            if live.is_some_and(|l| l != ours) || theirs.as_ref().is_some_and(|t| *t != ours) {
                seen.verdicts.push(format!(
                    "{id} at {height}: live {live:?}, Heartwood {ours}, the reference {theirs:?}"
                ));
            }
            refused = (ours != "accept").then(|| ours.clone());
            submitted = Some(index);
            label = format!("transaction {id} at {height} ({ours})");
        }
        seen.steps += 1;
        let after = heart.orders();
        if let Some(r) = reference.as_deref_mut() {
            let theirs = r.orders(heart.heart.height());
            if let Some(d) = after.first_difference(&theirs) {
                seen.disagreements
                    .push(format!("{node}, after {label}: {d}"));
            }
        }
        let had: BTreeSet<&String> = before.addresses.iter().collect();
        let has: BTreeSet<&String> = after.addresses.iter().collect();
        seen.entered += usize::from(has.difference(&had).next().is_some());
        seen.left += usize::from(had.difference(&has).next().is_some());
        if let Some(code) = refused {
            let moved = after.addresses != before.addresses;
            let count = seen.refusals.entry(code).or_default();
            if moved {
                count.0 += 1;
                seen.moved_by_refusal += 1;
                if let Some(index) = submitted {
                    (seen.refused_parties).extend(fixture.parties(&heart.heart, index));
                }
            } else {
                count.1 += 1;
            }
        }
        before = after;
    }
    (heart, seen)
}

/// After a node's steps, its chain unwound one block at a time to the genesis block through both
/// implementations, the orders compared after each revert: every wallet a block created leaves
/// the address index again when the revert empties it.
fn unwind(heart: &mut HeartSide, reference: &mut ReferenceSide, node: &str, seen: &mut Replayed) {
    let mut before = heart.orders();
    while heart.heart.height() > 1 {
        let height = heart.heart.height();
        heart.heart.revert(1).expect("Heartwood's revert");
        reference.ask("lockstep.revert", height, json!({"count": 1}));
        let after = heart.orders();
        if let Some(d) = after.first_difference(&reference.orders(height - 1)) {
            seen.disagreements
                .push(format!("{node}, after the revert of block {height}: {d}"));
        }
        let had: BTreeSet<&String> = before.addresses.iter().collect();
        let has: BTreeSet<&String> = after.addresses.iter().collect();
        seen.left += usize::from(had.difference(&has).next().is_some());
        seen.steps += 1;
        before = after;
    }
}

#[test]
fn each_node_s_traffic_replays_to_the_verdicts_and_the_listing_it_served() {
    let fixture = Fixture::load();
    for node in ["node1", "node3", "node2", "node5"] {
        let (heart, seen) = replay(&fixture, node, None, &BTreeSet::new());
        assert!(
            seen.verdicts.is_empty(),
            "{node}: verdicts the node did not give live: {:#?}",
            seen.verdicts
        );
        // Only the handler's refusals move a wallet: a refusal before it leaves the orders alone.
        for (code, (moved, _)) in &seen.refusals {
            assert!(
                code == "ERR_APPLY" || *moved == 0,
                "{node}: {code} moved a wallet"
            );
        }
        eprintln!(
            "{node}: refusals by code (moved a wallet, moved none): {:?}",
            seen.refusals
        );
        let page = fixture.page(node);
        let listed = heart.listing(page.len());
        assert_eq!(
            listed, page,
            "{node}: the replayed listing is not the page the node served"
        );
    }
}

/// The transactions (by index) that only `node` of the pair (`node`, `other`) received, and the
/// verdicts it gave them.
fn received_alone(fixture: &Fixture, node: &str, other: &str) -> BTreeMap<usize, String> {
    let of = |n: &str| -> BTreeSet<usize> {
        (fixture.steps(n).iter())
            .filter_map(|s| s["submit"].as_u64())
            .map(|i| usize::try_from(i).expect("usize"))
            .collect()
    };
    let theirs = of(other);
    (fixture.steps(node).iter())
        .filter_map(|s| {
            let i = usize::try_from(s["submit"].as_u64()?).ok()?;
            (!theirs.contains(&i)).then(|| (i, s["verdict"].as_str().unwrap_or("").to_owned()))
        })
        .collect()
}

#[test]
fn the_majority_s_tie_follows_the_refusals_one_node_alone_received() {
    let fixture = Fixture::load();
    let (one, _) = replay(&fixture, "node1", None, &BTreeSet::new());
    let (three, seen) = replay(&fixture, "node3", None, &BTreeSet::new());
    // Node 1 received nothing node 3 did not; node 3 alone received the refusals sent to it.
    assert!(received_alone(&fixture, "node1", "node3").is_empty());
    let alone = received_alone(&fixture, "node3", "node1");
    assert!(!alone.is_empty() && alone.values().all(|v| v != "accept"));
    let (a, b) = (one.orders(), three.orders());
    assert_ne!(a.addresses, b.addresses, "the two nodes' orders differ");
    // Without the refusals node 3 alone received, its order is node 1's.
    let skip: BTreeSet<usize> = alone.keys().copied().collect();
    let (without, _) = replay(&fixture, "node3", None, &skip);
    assert_eq!(
        without.orders(),
        a,
        "without its own refusals node 3 holds node 1's orders"
    );
    // The wallets those refusals moved are the only ones out of place: the delegate whose two
    // resignations node 3 refused among them.
    let genesis_1 =
        (one.heart.state.wallets().find_by_username("genesis_1")).map(|w| w.address().to_base58());
    let moved: BTreeSet<String> = seen.refused_parties.iter().cloned().collect();
    let strip = |list: &[String]| -> Vec<String> {
        list.iter()
            .filter(|a| !moved.contains(*a))
            .cloned()
            .collect()
    };
    assert_eq!(strip(&a.addresses), strip(&b.addresses));
    assert!(genesis_1.is_some_and(|g| moved.contains(&g)), "{moved:?}");
}

// The integration test: the real reference.

/// The oracle of the integration tests; `None` when the run skips them on purpose
/// (`DIFFTEST_TEST_NO_ORACLE`). Without either, the test fails: it must not pass unrun.
fn oracle_or_skip(test: &str) -> Option<Oracle> {
    if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
        return Some(Oracle::new(Mode::Local {
            solar_dir: PathBuf::from(dir),
        }));
    }
    if let Ok(image) = std::env::var("DIFFTEST_TEST_IMAGE") {
        return Some(Oracle::new(Mode::Container { image }));
    }
    assert!(
        std::env::var_os("DIFFTEST_TEST_NO_ORACLE").is_some(),
        "{test}: no test oracle configured: set DIFFTEST_TEST_IMAGE or DIFFTEST_TEST_SOLAR_DIR \
         (make test-integration sets the image when it is built), or DIFFTEST_TEST_NO_ORACLE to \
         skip the test"
    );
    eprintln!("{test}: skipped (DIFFTEST_TEST_NO_ORACLE)");
    None
}

#[test]
#[ignore = "integration"]
fn both_implementations_hold_one_order_through_each_node_s_traffic() {
    let Some(oracle) = oracle_or_skip("the listings replay") else {
        return;
    };
    let fixture = Fixture::load();
    let (mut moved, mut entered, mut left) = (0, 0, 0);
    for node in ["node1", "node3", "node2", "node5"] {
        let mut reference = ReferenceSide::open(&fixture, &oracle);
        let (mut heart, mut seen) = replay(&fixture, node, Some(&mut reference), &BTreeSet::new());
        unwind(&mut heart, &mut reference, node, &mut seen);
        assert!(
            seen.disagreements.is_empty(),
            "{node}: the implementations' orders differ: {:#?}",
            &seen.disagreements[..seen.disagreements.len().min(5)]
        );
        assert!(
            seen.verdicts.is_empty(),
            "{node}: verdicts that differ: {:#?}",
            seen.verdicts
        );
        eprintln!(
            "{node}: {} steps (the unwinding to the genesis block included), one order after \
             each; {} refusals moved a wallet, {} steps added an address, {} removed one",
            seen.steps, seen.moved_by_refusal, seen.entered, seen.left
        );
        (moved, entered, left) = (
            moved + seen.moved_by_refusal,
            entered + seen.entered,
            left + seen.left,
        );
    }
    // The replay went through every way an index changes: re-indexing by a refusal, an address's
    // first sight, and an emptied wallet's removal.
    assert!(
        moved > 0 && entered > 0 && left > 0,
        "{moved} {entered} {left}"
    );
}
