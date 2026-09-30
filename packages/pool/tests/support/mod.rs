//! Shared setup of the pool tests: a small network with a fee table, keys, funded wallets, a
//! pool in a temporary directory and transactions signed with a fixed aux.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::{Aux, KeyPair, Params, Transaction};
use heartwood_kernel::clock::ManualClock;
use heartwood_pool::{
    Checked, DelegateVersions, Entry, Options, Pool, PoolContext, Processor, ProcessorResult,
    Refusal, Reindexed,
};
use heartwood_state::wallets::WalletRepository;

/// The network byte.
pub const NETWORK: u8 = 90;
/// The aux every test signature uses.
pub const AUX: Aux = Aux::fixed([0x42; 32]);
/// The tip of the tests; the pool applies the rules of the next height.
pub const TIP: u32 = 500;
/// A fee above every floor of the tests.
pub const FEE: u64 = 10_000_000;
/// The time of the tests, in milliseconds.
pub const NOW_MS: i64 = 1_800_000_000_000;

pub fn network() -> Network {
    Network {
        name: "devnet".into(),
        message_prefix: "devnet message:\n".into(),
        address_character: Some("d".into()),
        bip32_public: 70_617_039,
        bip32_private: 70_615_956,
        pub_key_hash: NETWORK,
        nethash: "00".repeat(32),
        wif: 252,
        slip44: 1,
        token: "dROOT".into(),
        symbol: "dRT".into(),
        explorer: String::new(),
    }
}

pub static NETWORK_FILE: LazyLock<Network> = LazyLock::new(network);

/// A milestone file with three active delegates and a fee table; `first` is appended to the
/// milestone at height 1, and `later` (JSON objects as text) are added after it.
pub fn milestones_with(first: &str, later: &str) -> Milestones {
    let text = format!(
        r#"[{{"height":1,"activeDelegates":3,"blockTime":8,
        "block":{{"version":0,"maxTransactions":150,"maxPayload":2097152}},
        "epoch":"2026-01-01T00:00:00.000Z","reward":0,
        "burn":{{"feeBasisPoints":9000,"txAmount":1000}},"transfer":{{}},
        "blocksToRevokeDelegateResignation":106,
        "fees":{{"staticFees":{{"transfer":10000000,"vote":9000000,"burn":0}}}},
        "dynamicFees":{{"enabled":true,"minFee":100,"addonBytes":{{"transfer":10,"delegateRegistration":1000,"vote":20}}}}{first}}}{later}]"#
    );
    Milestones::load(&text, &network()).expect("the test milestones load")
}

pub static MILESTONES: LazyLock<Milestones> = LazyLock::new(|| milestones_with("", ""));

/// Milestones whose dynamic fees are off, so the pool's own fee rules apply.
pub static POOL_FEES: LazyLock<Milestones> =
    LazyLock::new(|| milestones_with(r#","dynamicFees":{"enabled":false}"#, ""));

/// The rules of the pool's height under [`MILESTONES`].
pub fn params() -> &'static Params {
    MILESTONES.at(TIP + 1)
}

/// A key pair and its address.
pub struct Account {
    pub keys: KeyPair,
    pub address: Address,
}

impl Account {
    pub fn new(passphrase: &str) -> Account {
        let keys = KeyPair::from_passphrase(passphrase).expect("a key pair");
        let address = Address::from_public_key(keys.public_key(), NETWORK);
        Account { keys, address }
    }

    pub fn public_key(&self) -> &PublicKey {
        self.keys.public_key()
    }

    /// Sign `builder` with this account under the pool's rules.
    pub fn sign(&self, builder: TransactionBuilder) -> Transaction {
        builder
            .build(&self.keys, AUX, params())
            .expect("the transaction builds")
    }

    /// A transfer of `amount` to `to` with `nonce` and `fee`.
    pub fn transfer(&self, to: &Account, amount: u64, nonce: u64, fee: u64) -> Transaction {
        self.sign(transfer(to, amount).nonce(nonce).fee(fee))
    }
}

/// Give the wallet of `account` a balance and a nonce, and a bound key when the nonce shows that
/// it has sent a transaction.
pub fn fund(wallets: &mut WalletRepository, account: &Account, balance: u64, nonce: u64) {
    if nonce > 0 {
        wallets.bind_public_key(&account.address, account.public_key().clone());
    }
    let wallet = wallets.get_or_create(&account.address);
    wallet.credit(balance).expect("the balance fits");
    wallet.set_nonce(nonce);
}

/// Make `account` a funded delegate named `username`.
pub fn delegate(wallets: &mut WalletRepository, account: &Account, username: &str) {
    fund(wallets, account, 1_000 * FEE, 1);
    wallets
        .register_delegate(&account.address, username.to_owned())
        .expect("the name is free");
}

/// A transfer of `amount` to `to`.
pub fn transfer(to: &Account, amount: u64) -> TransactionBuilder {
    TransactionBuilder::transfer(vec![TransferItem {
        amount,
        recipient: to.address,
    }])
}

/// A resignation of `resignation_type`.
pub fn resignation(resignation_type: Option<ResignationType>) -> TransactionBuilder {
    TransactionBuilder::delegate_resignation(resignation_type)
}

/// Votes from `(name, basis points)` pairs.
pub fn votes(entries: &[(&str, u16)]) -> Votes {
    entries
        .iter()
        .map(|(name, bps)| ((*name).to_owned(), *bps))
        .collect()
}

/// A fresh directory for one test.
pub fn tempdir(name: &str) -> PathBuf {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let n = COUNT.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("heartwood-pool-{}-{n}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
}

/// The reference's options, with the storage in `dir` and one worker.
pub fn options(dir: &std::path::Path) -> Options {
    let mut options = Options::defaults(&|_| None, dir, 2);
    options.worker_count = 1;
    options
}

/// The accounts of the standard fixture: five delegates (three active ones and two more, so that
/// one may resign) and four senders, three of them funded.
pub struct Fixture {
    pub wallets: WalletRepository,
    pub delegates: [Account; 5],
    pub alice: Account,
    pub bob: Account,
    pub carol: Account,
    pub dave: Account,
    pub online: DelegateVersions,
    /// What the pool's handler runs leave for the state to re-index.
    pub reindexed: Reindexed,
}

impl Fixture {
    pub fn new() -> Fixture {
        let mut wallets = WalletRepository::new(NETWORK);
        let delegates =
            ["a", "b", "c", "d", "e"].map(|n| Account::new(&format!("pool delegate {n}")));
        for (account, name) in delegates
            .iter()
            .zip(["del_a", "del_b", "del_c", "del_d", "del_e"])
        {
            delegate(&mut wallets, account, name);
        }
        let [alice, bob, carol, dave] =
            ["pool alice", "pool bob", "pool carol", "pool dave"].map(Account::new);
        fund(&mut wallets, &alice, 1_000 * FEE, 1);
        fund(&mut wallets, &bob, 1_000 * FEE, 0);
        fund(&mut wallets, &carol, 1_000 * FEE, 3);
        let mut online = DelegateVersions::new();
        online.set(
            delegates[0].public_key().to_compressed(),
            90,
            "4.3.1".into(),
        );
        Fixture {
            wallets,
            delegates,
            alice,
            bob,
            carol,
            dave,
            online,
            reindexed: Reindexed::default(),
        }
    }

    /// The pool's context at [`TIP`] under `milestones`.
    pub fn ctx<'a>(&'a self, milestones: &'a Milestones) -> PoolContext<'a> {
        PoolContext {
            wallets: &self.wallets,
            tip: TIP,
            milestones,
            network: &NETWORK_FILE,
            online: &self.online,
            reindexed: &self.reindexed,
        }
    }
}

/// A pool with its processor and a manual clock.
pub struct Harness {
    pub dir: PathBuf,
    pub pool: Pool,
    pub processor: Processor,
    pub clock: ManualClock,
}

impl Harness {
    pub fn new(name: &str, adjust: impl FnOnce(&mut Options)) -> Harness {
        let dir = tempdir(name);
        let mut options = options(&dir);
        adjust(&mut options);
        Harness {
            pool: Pool::open(options, None).expect("the pool opens"),
            dir,
            processor: Processor::new(),
            clock: ManualClock::new(NOW_MS),
        }
    }

    /// Process `entries` under `ctx`.
    pub fn process(&mut self, ctx: &PoolContext<'_>, entries: Vec<Entry>) -> ProcessorResult {
        self.processor
            .process(&mut self.pool, ctx, &self.clock, entries)
            .expect("the storage works")
    }

    /// Process the bytes of `transactions` under `ctx`.
    pub fn submit(
        &mut self,
        ctx: &PoolContext<'_>,
        transactions: &[&Transaction],
    ) -> ProcessorResult {
        let entries = transactions.iter().map(|tx| bytes(tx)).collect();
        self.process(ctx, entries)
    }

    /// Add `transaction` directly, as the second loop does.
    pub fn add(&mut self, ctx: &PoolContext<'_>, transaction: &Transaction) -> Result<(), Refusal> {
        match self
            .pool
            .add_transaction(ctx, Checked::Decoded(Box::new(transaction.clone())))
        {
            Ok(()) => Ok(()),
            Err(heartwood_pool::AddError::Refused(refusal)) => Err(refusal),
            Err(error) => panic!("storage: {error}"),
        }
    }

    /// The ids of the pool's transactions in `getAll` order.
    pub fn all(&self) -> Vec<String> {
        ids(self.pool.query().all())
    }

    /// The ids of the stored rows, in storage order.
    pub fn stored(&self) -> Vec<String> {
        self.pool
            .stored()
            .expect("the storage reads")
            .iter()
            .map(|row| row.id.to_hex())
            .collect()
    }
}

/// The bytes entry of `transaction`.
pub fn bytes(transaction: &Transaction) -> Entry {
    Entry::Bytes(transaction.bytes().to_vec())
}

/// The ids of `transactions`.
pub fn ids<'a>(transactions: impl IntoIterator<Item = &'a Transaction>) -> Vec<String> {
    transactions
        .into_iter()
        .map(|tx| tx.id().to_hex())
        .collect()
}

/// The ids of `transactions`, as the pool shares them.
pub fn shared_ids(transactions: &[Arc<Transaction>]) -> Vec<String> {
    ids(transactions.iter().map(Arc::as_ref))
}

/// The id of `transaction`.
pub fn id(transaction: &Transaction) -> String {
    transaction.id().to_hex()
}

/// `transaction` as the reference's `toString` writes it.
pub fn text(transaction: &Transaction) -> String {
    transaction.display(NETWORK).to_string()
}

/// The code and message of the refusal of entry `id`.
pub fn error_of(result: &ProcessorResult, id: &str) -> (String, String) {
    let refusal = result
        .errors
        .as_ref()
        .and_then(|errors| errors.get(id))
        .unwrap_or_else(|| panic!("no refusal of {id} in {result:?}"));
    (refusal.code.as_str().to_owned(), refusal.message.clone())
}
