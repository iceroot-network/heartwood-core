//! The pool-entry checks, against fake views of the pool and of the delegates' nodes.

mod support;

use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::ResignationType;
use heartwood_crypto::{Transaction, TransactionType};
use heartwood_state::wallets::{Resignation, WalletRepository};
use heartwood_transactions::{OnlineDelegates, PoolError, PoolQuery, throw_if_cannot_enter_pool};
use support::*;

/// A pool that holds transactions of the listed senders and kinds, and registrations of the
/// listed names.
#[derive(Default)]
struct FakePool {
    senders: Vec<(Address, TransactionType)>,
    usernames: Vec<String>,
}

impl PoolQuery for FakePool {
    fn has_sender_kind(&self, sender: &Address, kind: TransactionType) -> bool {
        self.senders.contains(&(*sender, kind))
    }

    fn has_pending_username(&self, username: &str) -> bool {
        self.usernames.iter().any(|pending| pending == username)
    }
}

/// The delegates whose nodes are online.
struct FakeOnline(Vec<&'static str>);

impl OnlineDelegates for FakeOnline {
    fn is_online(&self, username: &str) -> bool {
        self.0.contains(&username)
    }
}

struct Setup {
    wallets: WalletRepository,
    sender: Account,
    delegates: [Account; 3],
}

fn setup() -> Setup {
    let mut wallets = WalletRepository::new(NETWORK);
    let sender = Account::new("sender");
    fund(&mut wallets, &sender, 100 * FEE, 1);
    let delegates = [
        Account::new("delegate a"),
        Account::new("delegate b"),
        Account::new("delegate c"),
    ];
    for (account, name) in delegates.iter().zip(["del_a", "del_b", "del_c"]) {
        delegate(&mut wallets, account, name);
    }
    Setup {
        wallets,
        sender,
        delegates,
    }
}

fn enter(
    setup: &Setup,
    transaction: &Transaction,
    pool: &FakePool,
    online: &FakeOnline,
) -> Result<(), &'static str> {
    throw_if_cannot_enter_pool(&ctx(), &setup.wallets, transaction, pool, online)
        .map_err(|error| error.code())
}

fn pending(account: &Account, kind: TransactionType) -> FakePool {
    FakePool {
        senders: vec![(account.address, kind)],
        usernames: Vec::new(),
    }
}

#[test]
fn transfers_and_burns_always_enter() {
    let setup = setup();
    let all_pending = FakePool {
        senders: vec![
            (setup.sender.address, TransactionType::Transfer),
            (setup.sender.address, TransactionType::Burn),
        ],
        usernames: Vec::new(),
    };
    let offline = FakeOnline(Vec::new());
    let transfer = setup
        .sender
        .sign(transfer(&setup.delegates[0], 1).nonce(2).fee(FEE));
    let burn = setup
        .sender
        .sign(TransactionBuilder::burn(BURN_MINIMUM).nonce(2).fee(FEE));
    assert_eq!(enter(&setup, &transfer, &all_pending, &offline), Ok(()));
    assert_eq!(enter(&setup, &burn, &all_pending, &offline), Ok(()));
}

#[test]
fn one_second_signature_registration_per_sender() {
    let setup = setup();
    let second = Account::new("second");
    let tx = setup.sender.sign(
        TransactionBuilder::second_signature(second.public_key().to_compressed())
            .nonce(2)
            .fee(FEE),
    );
    let online = FakeOnline(Vec::new());
    assert_eq!(enter(&setup, &tx, &FakePool::default(), &online), Ok(()));
    assert_eq!(
        enter(
            &setup,
            &tx,
            &pending(&setup.sender, TransactionType::SecondSignature),
            &online
        ),
        Err("ERR_PENDING")
    );
    // Another kind pending from the same sender does not count.
    assert_eq!(
        enter(
            &setup,
            &tx,
            &pending(&setup.sender, TransactionType::Vote),
            &online
        ),
        Ok(())
    );
}

#[test]
fn one_delegate_registration_per_sender_and_per_name() {
    let setup = setup();
    let tx = setup.sender.sign(
        TransactionBuilder::delegate_registration("newcomer")
            .nonce(2)
            .fee(FEE),
    );
    let online = FakeOnline(Vec::new());
    assert_eq!(enter(&setup, &tx, &FakePool::default(), &online), Ok(()));
    let by_sender = pending(&setup.sender, TransactionType::DelegateRegistration);
    let result =
        throw_if_cannot_enter_pool(&ctx(), &setup.wallets, &tx, &by_sender, &online).unwrap_err();
    assert!(matches!(result, PoolError::SenderPending { .. }));
    let by_name = FakePool {
        senders: Vec::new(),
        usernames: vec!["newcomer".into()],
    };
    let result =
        throw_if_cannot_enter_pool(&ctx(), &setup.wallets, &tx, &by_name, &online).unwrap_err();
    assert_eq!(
        result,
        PoolError::UsernamePending {
            username: "newcomer".into()
        }
    );
    assert_eq!(result.code(), "ERR_PENDING");
}

#[test]
fn resignations() {
    let mut setup = setup();
    let [a, ..] = &setup.delegates;
    let resign = |account: &Account, resignation_type| {
        account.sign(resignation(resignation_type).nonce(2).fee(FEE))
    };
    let nobody = FakeOnline(Vec::new());
    let a_online = FakeOnline(vec!["del_a"]);
    // A resignation of any type from a sender with one pending.
    for resignation_type in [None, Some(ResignationType::NotResigned)] {
        assert_eq!(
            enter(
                &setup,
                &resign(a, resignation_type),
                &pending(a, TransactionType::DelegateResignation),
                &a_online
            ),
            Err("ERR_PENDING")
        );
    }
    // Resigning needs no node; a revoke does.
    assert_eq!(
        enter(&setup, &resign(a, None), &FakePool::default(), &nobody),
        Ok(())
    );
    assert_eq!(
        enter(
            &setup,
            &resign(a, Some(ResignationType::PermanentResign)),
            &FakePool::default(),
            &nobody
        ),
        Ok(())
    );
    resign_as(&mut setup.wallets, a, Resignation::Temporary);
    let revoke = resign(a, Some(ResignationType::NotResigned));
    let offline = throw_if_cannot_enter_pool(
        &ctx(),
        &setup.wallets,
        &revoke,
        &FakePool::default(),
        &nobody,
    )
    .unwrap_err();
    assert_eq!(
        offline,
        PoolError::Offline {
            username: "del_a".into()
        }
    );
    assert_eq!(offline.code(), "ERR_OFFLINE");
    assert_eq!(
        enter(&setup, &revoke, &FakePool::default(), &a_online),
        Ok(())
    );
    // A revoke by a wallet that is not a delegate is left to the apply checks.
    let stranger = resign(&setup.sender, Some(ResignationType::NotResigned));
    assert_eq!(
        enter(&setup, &stranger, &FakePool::default(), &nobody),
        Ok(())
    );
}

fn resign_as(wallets: &mut WalletRepository, account: &Account, resigned: Resignation) {
    support::resign(wallets, account, resigned, 10);
}

#[test]
fn votes() {
    let mut setup = setup();
    let vote = |entries: &[(&str, u16)]| {
        setup.sender.sign(
            TransactionBuilder::vote(support::votes(entries))
                .nonce(2)
                .fee(FEE),
        )
    };
    let tx = vote(&[("del_a", 5_000), ("del_b", 5_000)]);
    let all = FakeOnline(vec!["del_a", "del_b", "del_c"]);
    assert_eq!(enter(&setup, &tx, &FakePool::default(), &all), Ok(()));
    assert_eq!(
        enter(
            &setup,
            &tx,
            &pending(&setup.sender, TransactionType::Vote),
            &all
        ),
        Err("ERR_PENDING")
    );
    // The first offline name in the order of the vote.
    let only_a = FakeOnline(vec!["del_a"]);
    let two_offline = vote(&[("del_b", 5_000), ("del_c", 5_000)]);
    assert_eq!(
        throw_if_cannot_enter_pool(
            &ctx(),
            &setup.wallets,
            &two_offline,
            &FakePool::default(),
            &only_a
        ),
        Err(PoolError::Offline {
            username: "del_b".into()
        })
    );
    // A resigned delegate needs no node, and an unknown name is left to the apply checks.
    let unvote = vote(&[]);
    assert_eq!(
        enter(&setup, &unvote, &FakePool::default(), &only_a),
        Ok(())
    );
    let [_, b, c] = &setup.delegates;
    resign_as(&mut setup.wallets, b, Resignation::Temporary);
    resign_as(&mut setup.wallets, c, Resignation::Permanent);
    let tx = vote(&[("del_b", 5_000), ("del_c", 5_000)]);
    assert_eq!(enter(&setup, &tx, &FakePool::default(), &only_a), Ok(()));
    let unknown = vote(&[("del_a", 5_000), ("nobody", 5_000)]);
    assert_eq!(
        enter(&setup, &unknown, &FakePool::default(), &only_a),
        Ok(())
    );
}
