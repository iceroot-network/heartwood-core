//! Shared setup of the handler tests: a small network with a fee table, keys, wallets and
//! transactions signed with a fixed aux.

#![allow(dead_code)]

use std::sync::LazyLock;

use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::managers::{Milestones, Network};
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{Aux, KeyPair, Params, Transaction};
use heartwood_state::wallets::{Resignation, Wallet, WalletRepository};
use heartwood_state::{ApplyContext, TransactionHandlers};
use heartwood_transactions::Handlers;

/// The network byte.
pub const NETWORK: u8 = 90;
/// The aux every test signature uses.
pub const AUX: Aux = Aux::fixed([0x42; 32]);
/// The height the tests apply transactions at.
pub const HEIGHT: u32 = 500;
/// `burn.txAmount`.
pub const BURN_MINIMUM: u64 = 1_000;
/// `blocksToRevokeDelegateResignation`.
pub const REVOKE_DELAY: u64 = 106;
/// A fee above every floor of the tests.
pub const FEE: u64 = 10_000_000;

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

/// A milestone file with three active delegates, a fee table and a revoke delay; `extra` is
/// appended to the first milestone (JSON members as text).
pub fn milestones_with(extra: &str) -> Milestones {
    let text = format!(
        r#"[{{"height":1,"activeDelegates":3,"blockTime":8,"block":{{"version":0}},
        "epoch":"2026-01-01T00:00:00.000Z","reward":0,
        "burn":{{"feeBasisPoints":9000,"txAmount":{BURN_MINIMUM}}},"transfer":{{}},
        "blocksToRevokeDelegateResignation":{REVOKE_DELAY},
        "dynamicFees":{{"enabled":true,"minFee":100,"addonBytes":{{"transfer":10,"delegateRegistration":1000,"vote":20}}}}{extra}}}]"#
    );
    Milestones::load(&text, &network()).expect("the test milestones load")
}

pub static MILESTONES: LazyLock<Milestones> = LazyLock::new(|| milestones_with(""));

pub fn params() -> &'static Params {
    MILESTONES.at(HEIGHT)
}

/// The context of a block at [`HEIGHT`].
pub fn ctx() -> ApplyContext<'static> {
    ApplyContext {
        height: HEIGHT,
        params: params(),
        last_block_height: HEIGHT - 1,
    }
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

    /// Sign `builder` with this account.
    pub fn sign(&self, builder: TransactionBuilder) -> Transaction {
        builder
            .build(&self.keys, AUX, params())
            .expect("the transaction builds")
    }

    /// Sign `builder` with this account as a transaction of a genesis block, whose fee may be 0.
    pub fn sign_genesis(&self, builder: TransactionBuilder) -> Transaction {
        builder
            .build_with(&self.keys, AUX, params(), Mode::Genesis)
            .expect("the transaction builds")
    }

    /// Sign `builder` with this account and `second` as the second signature.
    pub fn sign_twice(&self, builder: TransactionBuilder, second: &Account) -> Transaction {
        builder
            .build_second_signed(&self.keys, AUX, &second.keys, AUX, params())
            .expect("the transaction builds")
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

/// Resign the delegate of `account` as `resigned` at `height`.
pub fn resign(
    wallets: &mut WalletRepository,
    account: &Account,
    resigned: Resignation,
    height: u32,
) {
    let delegate = wallets
        .wallet_mut(&account.address)
        .and_then(Wallet::delegate_mut)
        .expect("a delegate");
    delegate.resigned = Some(resigned);
    delegate.resigned_at = Some(height);
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

/// The wallets a digest shows: every wallet that is not empty, by address.
pub fn visible(wallets: &WalletRepository) -> Vec<Wallet> {
    let mut shown: Vec<Wallet> = wallets
        .iter()
        .filter(|wallet| **wallet != Wallet::new(*wallet.address()))
        .cloned()
        .collect();
    shown.sort_by_key(|wallet| wallet.address().to_base58());
    shown
}

/// The class of the refusal of `transaction`, after checking that the refusal changed nothing.
pub fn refused(wallets: &mut WalletRepository, transaction: &Transaction) -> &'static str {
    let before = wallets.clone();
    let checked = Handlers.throw_if_cannot_be_applied(&ctx(), wallets, transaction);
    let rejected = Handlers
        .apply(&ctx(), wallets, transaction)
        .expect_err("the transaction is refused");
    assert_eq!(
        checked.map_err(|error| error.class()),
        Err(rejected.class())
    );
    assert_eq!(*wallets, before, "a refusal changed the wallets");
    rejected.class()
}

/// Apply `transaction`, then revert it, checking that the revert restores the visible wallets.
/// Returns the wallets after the apply.
pub fn apply_and_revert(
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> WalletRepository {
    let before = visible(wallets);
    Handlers
        .throw_if_cannot_be_applied(&ctx(), wallets, transaction)
        .expect("the checks pass");
    let undo = Handlers
        .apply(&ctx(), wallets, transaction)
        .expect("the transaction applies");
    let applied = wallets.clone();
    Handlers
        .revert(&ctx(), wallets, transaction, &undo)
        .expect("the transaction reverts");
    assert_eq!(
        visible(wallets),
        before,
        "the revert did not restore the wallets"
    );
    applied
}
