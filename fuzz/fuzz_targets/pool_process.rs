//! Fuzz the pool: arbitrary sequences of steps (batches of entries from a small fixture's keys,
//! the clock moving on, removals, re-adds, blocks, a restart of the storage) on a small fixture.
//!
//! No step may panic; the pool never holds more than its limits; each sender's transactions apply
//! in order on a fresh view of the wallets; and a collation is in fee order, each sender's
//! transactions in nonce order. The storage always holds exactly the pool's transactions.

#![no_main]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::LazyLock;

use arbitrary::Arbitrary;
use heartwood_crypto::identities::Address;
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem};
use heartwood_crypto::{Aux, KeyPair, Transaction, TransactionId};
use heartwood_kernel::clock::ManualClock;
use heartwood_pool::{DelegateVersions, Entry, Options, Pool, PoolContext, Processor, collate};
use heartwood_state::wallets::WalletRepository;
use heartwood_state::{ApplyContext, TransactionHandlers};
use heartwood_transactions::Handlers;
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

const NETWORK: u8 = 90;
const FEE: u64 = 10_000_000;
const TIP: u32 = 100;

/// The network, the rules, the state's wallets and a catalogue of signed transactions.
struct Fixture {
    network: Network,
    milestones: Milestones,
    wallets: WalletRepository,
    online: DelegateVersions,
    catalogue: Vec<(Vec<u8>, Value)>,
    dir: PathBuf,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let network = Network {
        name: "devnet".into(),
        message_prefix: String::new(),
        address_character: None,
        bip32_public: 0,
        bip32_private: 0,
        pub_key_hash: NETWORK,
        nethash: "00".repeat(32),
        wif: 0,
        slip44: 1,
        token: String::new(),
        symbol: "dRT".into(),
        explorer: String::new(),
    };
    let milestones = Milestones::load(
        r#"[{"height":1,"activeDelegates":2,"blockTime":8,
        "block":{"version":0,"maxTransactions":6,"maxPayload":2000},
        "epoch":"2026-01-01T00:00:00.000Z","reward":0,
        "burn":{"feeBasisPoints":9000,"txAmount":1000},"transfer":{},
        "fees":{"staticFees":{"transfer":10000000}},
        "dynamicFees":{"enabled":true,"minFee":100,"addonBytes":{"transfer":10}}}]"#,
        &network,
    )
    .expect("the milestones load");
    let params = milestones.at(TIP + 1);
    let account = |name: &str| {
        let keys = KeyPair::from_passphrase(name).expect("a key pair");
        let address = Address::from_public_key(keys.public_key(), NETWORK);
        (keys, address)
    };
    let mut wallets = WalletRepository::new(NETWORK);
    let delegates: Vec<_> = ["a", "b", "c", "d"]
        .map(|n| account(&format!("fuzz delegate {n}")))
        .into();
    for (n, (keys, address)) in delegates.iter().enumerate() {
        wallets.bind_public_key(address, keys.public_key().clone());
        let wallet = wallets.get_or_create(address);
        wallet.credit(100 * FEE).expect("fits");
        wallet.set_nonce(1);
        wallets
            .register_delegate(address, format!("del_{n}"))
            .expect("a free name");
    }
    let senders: Vec<_> = ["a", "b", "c"]
        .map(|n| account(&format!("fuzz sender {n}")))
        .into();
    for (n, (keys, address)) in senders.iter().enumerate() {
        if n > 0 {
            wallets.bind_public_key(address, keys.public_key().clone());
        }
        let wallet = wallets.get_or_create(address);
        wallet.credit(20 * FEE).expect("fits");
        wallet.set_nonce(u64::from(n > 0));
    }
    let mut online = DelegateVersions::new();
    online.set(
        delegates[0].0.public_key().to_compressed(),
        1,
        "4.3.1".into(),
    );
    let aux = Aux::fixed([7; 32]);
    let mut transactions: Vec<Transaction> = Vec::new();
    let recipient = delegates[3].1;
    for (n, (keys, _)) in senders.iter().enumerate() {
        let first = u64::from(n > 0) + 1;
        for nonce in first..first + 4 {
            for fee in [FEE / 100, FEE, 3 * FEE] {
                let item = TransferItem {
                    amount: nonce * 1_000,
                    recipient,
                };
                transactions.push(
                    TransactionBuilder::transfer(vec![item])
                        .nonce(nonce)
                        .fee(fee)
                        .build(keys, aux, params)
                        .expect("builds"),
                );
            }
        }
        let votes = [("del_0", 10_000u16)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        transactions.push(
            TransactionBuilder::vote(votes)
                .nonce(first)
                .fee(FEE)
                .build(keys, aux, params)
                .expect("builds"),
        );
        transactions.push(
            TransactionBuilder::delegate_registration("fuzzname")
                .nonce(first)
                .fee(2 * FEE)
                .build(keys, aux, params)
                .expect("builds"),
        );
    }
    for (keys, _) in &delegates {
        transactions.push(
            TransactionBuilder::delegate_resignation(Some(ResignationType::PermanentResign))
                .nonce(2)
                .fee(FEE)
                .build(keys, aux, params)
                .expect("builds"),
        );
    }
    let mut catalogue: Vec<(Vec<u8>, Value)> = transactions
        .iter()
        .map(|tx| (tx.bytes().to_vec(), tx.to_json(params)))
        .collect();
    // A removed type: a transfer's type byte changed.
    let mut removed = catalogue[0].clone();
    removed.0[7] = 4;
    removed.1["type"] = Value::from(4);
    catalogue.push(removed);
    // Memory-backed when the system has it: the pool's file is opened for every input.
    let shm = std::path::Path::new("/dev/shm");
    let base = if shm.is_dir() {
        shm.to_path_buf()
    } else {
        std::env::temp_dir()
    };
    // Directories of earlier fuzz processes that have exited are removed.
    const PREFIX: &str = "heartwood-fuzz-pool-";
    for stale in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let name = stale.file_name().to_string_lossy().into_owned();
        if let Some(pid) = name.strip_prefix(PREFIX)
            && !std::path::Path::new("/proc").join(pid).exists()
        {
            let _ = std::fs::remove_dir_all(stale.path());
        }
    }
    let dir = base.join(format!("{PREFIX}{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    Fixture {
        network,
        milestones,
        wallets,
        online,
        catalogue,
        dir,
    }
});

#[derive(Debug, Arbitrary)]
struct EntrySpec {
    index: u8,
    json: bool,
    flip: Option<(u16, u8)>,
}

#[derive(Debug, Arbitrary)]
enum Step {
    Process {
        entries: Vec<EntrySpec>,
        seconds: u8,
    },
    Remove(u8),
    Forge,
    CleanUp,
    Readd,
    Restart,
    Collate {
        validate: bool,
        exclude: u8,
    },
}

#[derive(Debug, Arbitrary)]
struct Case {
    max_in_pool: u8,
    per_sender: u8,
    age: u8,
    workers: u8,
    pool_fees: bool,
    static_fees: bool,
    steps: Vec<Step>,
}

fuzz_target!(|case: Case| run(case));

fn run(case: Case) {
    let fixture = &*FIXTURE;
    let storage = fixture.dir.join("pool.sqlite");
    let marker = fixture.dir.join("clear-pool.lock");
    let mut options = Options::defaults(&|_| None, &fixture.dir, 2);
    options.storage = storage;
    options.max_transactions_in_pool = usize::from(case.max_in_pool % 8) + 1;
    options.max_transactions_per_sender = usize::from(case.per_sender % 4) + 1;
    options.max_transaction_age = u32::from(case.age % 6) + 1;
    options.worker_count = usize::from(case.workers % 3) + 1;
    options.dynamic_fees.enabled = !case.static_fees;
    let milestones_off;
    let milestones = if case.pool_fees {
        milestones_off = Milestones::load(
            r#"[{"height":1,"activeDelegates":2,"blockTime":8,
            "block":{"version":0,"maxTransactions":6,"maxPayload":2000},
            "epoch":"2026-01-01T00:00:00.000Z","reward":0,
            "burn":{"feeBasisPoints":9000,"txAmount":1000},"transfer":{},
            "fees":{"staticFees":{"transfer":10000000}},"dynamicFees":{"enabled":false}}]"#,
            &fixture.network,
        )
        .expect("the milestones load");
        &milestones_off
    } else {
        &fixture.milestones
    };
    std::fs::write(&marker, b"").expect("the marker");
    let mut pool = Pool::open(options.clone(), Some(&marker)).expect("the pool opens");
    let mut processor = Processor::new();
    let clock = ManualClock::new(1_800_000_000_000);
    let mut wallets = fixture.wallets.clone();
    let mut tip = TIP;
    for step in case.steps.into_iter().take(12) {
        let ctx = PoolContext {
            wallets: &wallets,
            tip,
            milestones,
            network: &fixture.network,
            online: &fixture.online,
        };
        match step {
            Step::Process { entries, seconds } => {
                clock.advance(i64::from(seconds % 41) * 1_000);
                let entries: Vec<Entry> = entries
                    .iter()
                    .take(6)
                    .map(|spec| entry(fixture, spec))
                    .collect();
                let count = entries.len();
                let result = processor
                    .process(&mut pool, &ctx, &clock, entries)
                    .expect("the storage works");
                assert_eq!(result.accept.len() + result.invalid.len(), count);
                assert!(result.excess.iter().all(|id| result.invalid.contains(id)));
                assert!(result.broadcast.iter().all(|id| result.accept.contains(id)));
                assert_eq!(result.errors.is_some(), !result.invalid.is_empty());
            }
            Step::Remove(n) => {
                let all: Vec<(Address, TransactionId)> = pool
                    .query()
                    .all()
                    .iter()
                    .map(|tx| (tx.data().sender_address(ctx.params()), *tx.id()))
                    .collect();
                if let Some((sender, id)) = all.get(usize::from(n) % all.len().max(1)) {
                    let removed = pool.remove_transaction(&ctx, sender, id).expect("storage");
                    assert!(removed.iter().any(|tx| tx.id() == id));
                }
            }
            Step::Forge => {
                // The synced node's sequence: a block of the collated candidates, then remove
                // forged, clean up and re-add at the new tip.
                let collated = collate(&mut pool, &ctx, true, &[]).expect("storage");
                let apply_ctx = ApplyContext {
                    height: tip + 1,
                    params: ctx.params(),
                    last_block_height: tip,
                };
                let mut next = wallets.clone();
                for tx in &collated.candidates {
                    Handlers
                        .apply(&apply_ctx, &mut next, tx)
                        .expect("a validated candidate applies in block order");
                }
                for tx in &collated.candidates {
                    let sender = tx.data().sender_address(ctx.params());
                    pool.remove_forged_transaction(&sender, tx.id())
                        .expect("storage");
                }
                wallets = next;
                tip += 1;
                let ctx = PoolContext {
                    wallets: &wallets,
                    tip,
                    milestones,
                    network: &fixture.network,
                    online: &fixture.online,
                };
                pool.clean_up(&ctx).expect("storage");
                pool.readd(&ctx, &[]).expect("storage");
            }
            Step::CleanUp => {
                pool.clean_up(&ctx).expect("storage");
            }
            Step::Readd => {
                pool.readd(&ctx, &[]).expect("storage");
            }
            Step::Restart => {
                drop(pool);
                pool = Pool::open(options.clone(), None).expect("the pool opens");
                pool.readd(&ctx, &[]).expect("storage");
            }
            Step::Collate { validate, exclude } => {
                let highest: Vec<TransactionId> = pool
                    .query()
                    .from_highest_priority()
                    .iter()
                    .map(|tx| *tx.id())
                    .collect();
                let exclude: Vec<TransactionId> = highest
                    .get(usize::from(exclude))
                    .copied()
                    .into_iter()
                    .collect();
                let collated = collate(&mut pool, &ctx, validate, &exclude).expect("storage");
                // A subsequence of the highest-priority order, each sender's nonces in order:
                // consecutive, unless an excluded transaction left a gap that nothing validated.
                let gaps = !validate && !exclude.is_empty();
                let mut rest = highest.iter();
                for tx in &collated.candidates {
                    assert!(rest.any(|id| id == tx.id()), "out of priority order");
                    assert!(!exclude.contains(tx.id()));
                }
                let mut last: Vec<(Address, u64)> = Vec::new();
                for tx in &collated.candidates {
                    let sender = tx.data().sender_address(ctx.params());
                    if let Some((_, nonce)) = last.iter_mut().find(|(a, _)| *a == sender) {
                        if gaps {
                            assert!(tx.data().nonce > *nonce, "nonces out of order");
                        } else {
                            assert_eq!(tx.data().nonce, *nonce + 1, "nonces out of order");
                        }
                        *nonce = tx.data().nonce;
                    } else {
                        last.push((sender, tx.data().nonce));
                    }
                }
            }
        }
        let ctx = PoolContext {
            wallets: &wallets,
            tip,
            milestones,
            network: &fixture.network,
            online: &fixture.online,
        };
        check(&pool, &ctx, &options);
    }
}

/// The entry of `spec`: a catalogue transaction as bytes or JSON, perhaps with a byte changed.
fn entry(fixture: &Fixture, spec: &EntrySpec) -> Entry {
    let (bytes, json) = &fixture.catalogue[usize::from(spec.index) % fixture.catalogue.len()];
    if spec.json {
        return Entry::Data(json.clone());
    }
    let mut bytes = bytes.clone();
    if let Some((at, value)) = spec.flip {
        let at = usize::from(at) % bytes.len();
        bytes[at] ^= value.max(1);
    }
    Entry::Bytes(bytes)
}

/// The pool's invariants.
fn check(pool: &Pool, ctx: &PoolContext<'_>, options: &Options) {
    assert!(
        pool.size() <= options.max_transactions_in_pool,
        "over the pool's limit"
    );
    let apply_ctx = ctx.apply_context();
    for sender in pool.mempool().senders() {
        assert!(!sender.is_empty(), "an empty sender mempool is kept");
        assert!(
            sender.len() <= options.max_transactions_per_sender,
            "over the sender's limit"
        );
        let mut view = ctx.wallets.clone();
        for tx in sender.from_earliest() {
            Handlers
                .apply(&apply_ctx, &mut view, tx)
                .expect("a sender's transactions apply in order on a fresh view");
        }
    }
    let pooled: BTreeSet<TransactionId> = pool.query().all().iter().map(|tx| *tx.id()).collect();
    let stored: BTreeSet<TransactionId> = pool
        .stored()
        .expect("storage")
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(pooled, stored, "the storage is not the pool");
}
