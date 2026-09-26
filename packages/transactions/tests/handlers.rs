//! The six handlers: every check in the reference's order, what apply changes, that revert
//! restores the wallets, and that a refusal changes nothing.

mod support;

use heartwood_crypto::TransactionType;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem};
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::vote_balances::{increase_vote_balances, update_vote_shares};
use heartwood_state::wallets::{Resignation, Wallet, WalletRepository};
use heartwood_state::{ApplyContext, TransactionHandlers};
use heartwood_transactions::{Handlers, minimum_fee};
use support::*;

fn wallet<'a>(wallets: &'a WalletRepository, account: &Account) -> &'a Wallet {
    wallets
        .find_by_address(&account.address)
        .expect("the wallet exists")
}

/// Three delegates and a funded sender.
fn network_of_delegates() -> (WalletRepository, Account, [Account; 3]) {
    let mut wallets = WalletRepository::new(NETWORK);
    let sender = Account::new("sender");
    fund(&mut wallets, &sender, 100 * FEE, 0);
    let delegates = [
        Account::new("delegate a"),
        Account::new("delegate b"),
        Account::new("delegate c"),
    ];
    for (account, name) in delegates.iter().zip(["del_a", "del_b", "del_c"]) {
        delegate(&mut wallets, account, name);
    }
    (wallets, sender, delegates)
}

/// Make `account` vote with `entries`, its shares added to its delegates.
fn set_votes(wallets: &mut WalletRepository, account: &Account, entries: &[(&str, u16)]) {
    wallets
        .get_or_create(&account.address)
        .set_votes(&votes(entries));
    update_vote_shares(wallets, &account.address).unwrap();
    increase_vote_balances(wallets, &account.address, true).unwrap();
}

fn vote_balance(wallets: &WalletRepository, username: &str) -> (u64, u64) {
    let delegate = wallets
        .find_by_username(username)
        .and_then(Wallet::delegate)
        .expect("a delegate");
    (delegate.vote_balance, delegate.voters)
}

// ---- the base checks ----------------------------------------------------------------------

#[test]
fn the_fee_floor_comes_before_the_nonce() {
    let (mut wallets, sender, _) = network_of_delegates();
    let to = Account::new("recipient");
    let probe = sender.sign(transfer(&to, 1).nonce(1).fee(FEE));
    let floor = u64::try_from(minimum_fee(&probe, params())).unwrap();
    // The size does not depend on the fee, so the floor is the same for every fee.
    let low = sender.sign(transfer(&to, 1).nonce(7).fee(floor - 1));
    assert_eq!(refused(&mut wallets, &low), "TransactionFeeTooLowError");
    let at_floor = sender.sign(transfer(&to, 1).nonce(1).fee(floor));
    apply_and_revert(&mut wallets, &at_floor);
    let wrong_nonce = sender.sign(transfer(&to, 1).nonce(2).fee(floor));
    assert_eq!(refused(&mut wallets, &wrong_nonce), "UnexpectedNonceError");
}

#[test]
fn the_nonce_comes_before_the_balance() {
    let (mut wallets, sender, _) = network_of_delegates();
    let balance = wallet(&wallets, &sender).balance();
    let over = TransactionBuilder::burn(balance).nonce(2).fee(FEE);
    assert_eq!(
        refused(&mut wallets, &sender.sign(over.clone())),
        "UnexpectedNonceError"
    );
    assert_eq!(
        refused(&mut wallets, &sender.sign(over.nonce(1))),
        "InsufficientBalanceError"
    );
    // The whole balance is exactly enough.
    let exact = TransactionBuilder::burn(balance - FEE).nonce(1).fee(FEE);
    let applied = apply_and_revert(&mut wallets, &sender.sign(exact));
    assert_eq!(wallet(&applied, &sender).balance(), 0);
    assert_eq!(wallet(&applied, &sender).nonce(), 1);
}

#[test]
fn a_sender_without_a_wallet_has_nothing_to_spend() {
    let mut wallets = WalletRepository::new(NETWORK);
    let stranger = Account::new("stranger");
    let burn = TransactionBuilder::burn(BURN_MINIMUM).nonce(1).fee(FEE);
    assert_eq!(
        refused(&mut wallets, &stranger.sign(burn)),
        "InsufficientBalanceError"
    );
    assert!(wallets.is_empty());
}

#[test]
fn a_refused_first_transaction_does_not_bind_the_key() {
    let mut wallets = WalletRepository::new(NETWORK);
    let fresh = Account::new("fresh sender");
    wallets.get_or_create(&fresh.address).credit(FEE).unwrap();
    let too_much = transfer(&Account::new("recipient"), FEE).nonce(1).fee(FEE);
    assert_eq!(
        refused(&mut wallets, &fresh.sign(too_much)),
        "InsufficientBalanceError"
    );
    assert!(wallet(&wallets, &fresh).public_key().is_none());
    assert!(!wallets.has_by_public_key(&fresh.public_key().to_compressed()));
}

#[test]
fn a_first_transaction_binds_the_key_and_its_revert_forgets_it() {
    let mut wallets = WalletRepository::new(NETWORK);
    let fresh = Account::new("fresh sender");
    wallets
        .get_or_create(&fresh.address)
        .credit(10 * FEE)
        .unwrap();
    let burn = fresh.sign(TransactionBuilder::burn(BURN_MINIMUM).nonce(1).fee(FEE));
    let applied = apply_and_revert(&mut wallets, &burn);
    assert_eq!(
        wallet(&applied, &fresh).public_key(),
        Some(fresh.public_key())
    );
    assert!(applied.has_by_public_key(&fresh.public_key().to_compressed()));
    assert!(wallet(&wallets, &fresh).public_key().is_none());
    assert!(!wallets.has_by_public_key(&fresh.public_key().to_compressed()));
}

#[test]
fn a_key_that_is_not_the_senders_is_a_mismatch() {
    let (mut wallets, sender, _) = network_of_delegates();
    let other = Account::new("someone else");
    // The sender's address with another key bound, which no transaction can produce.
    wallets.bind_public_key(&sender.address, other.public_key().clone());
    let burn = sender.sign(TransactionBuilder::burn(BURN_MINIMUM).nonce(1).fee(FEE));
    assert_eq!(refused(&mut wallets, &burn), "SenderWalletMismatchError");
}

#[test]
fn second_signatures() {
    let (mut wallets, sender, _) = network_of_delegates();
    let second = Account::new("second passphrase");
    let wrong = Account::new("wrong second passphrase");
    let burn = || TransactionBuilder::burn(BURN_MINIMUM).nonce(1).fee(FEE);

    // No second key: a second signature is unexpected.
    assert_eq!(
        refused(&mut wallets, &sender.sign_twice(burn(), &second)),
        "UnexpectedSecondSignatureError"
    );
    apply_and_revert(&mut wallets, &sender.sign(burn()));

    // A second key: the signature is required and must verify.
    wallets
        .get_or_create(&sender.address)
        .set_second_public_key(Some(second.public_key().to_compressed()));
    assert_eq!(
        refused(&mut wallets, &sender.sign(burn())),
        "InvalidSecondSignatureError"
    );
    assert_eq!(
        refused(&mut wallets, &sender.sign_twice(burn(), &wrong)),
        "InvalidSecondSignatureError"
    );
    apply_and_revert(&mut wallets, &sender.sign_twice(burn(), &second));
    // The second-signature check comes after the balance check.
    let over = TransactionBuilder::burn(1_000 * FEE).nonce(1).fee(FEE);
    assert_eq!(
        refused(&mut wallets, &sender.sign(over)),
        "InsufficientBalanceError"
    );
}

// ---- 1/6 transfer -------------------------------------------------------------------------

#[test]
fn a_transfer_credits_every_item_and_its_revert_takes_them_back() {
    let (mut wallets, sender, _) = network_of_delegates();
    let a = Account::new("recipient a");
    let b = Account::new("recipient b");
    let items = vec![
        TransferItem {
            amount: 5,
            recipient: a.address,
        },
        TransferItem {
            amount: 7,
            recipient: b.address,
        },
        TransferItem {
            amount: 11,
            recipient: a.address,
        },
        TransferItem {
            amount: 13,
            recipient: sender.address,
        },
    ];
    let before = wallet(&wallets, &sender).balance();
    let tx = sender.sign(TransactionBuilder::transfer(items).nonce(1).fee(FEE));
    let applied = apply_and_revert(&mut wallets, &tx);
    assert_eq!(wallet(&applied, &a).balance(), 16);
    assert_eq!(wallet(&applied, &b).balance(), 7);
    assert_eq!(wallet(&applied, &sender).balance(), before - FEE - 36 + 13);
}

#[test]
fn a_transfer_checks_its_items_before_the_base_checks() {
    let (mut wallets, sender, _) = network_of_delegates();
    let balance = wallet(&wallets, &sender).balance();
    let to = Account::new("recipient");
    // Over the balance with the fee, and a wrong nonce: the items' check comes first.
    let over = sender.sign(transfer(&to, balance - FEE + 1).nonce(9).fee(FEE));
    assert_eq!(refused(&mut wallets, &over), "InsufficientBalanceError");
    let exact = sender.sign(transfer(&to, balance - FEE).nonce(1).fee(FEE));
    let applied = apply_and_revert(&mut wallets, &exact);
    assert_eq!(wallet(&applied, &sender).balance(), 0);
}

#[test]
fn a_failed_credit_puts_every_wallet_back() {
    let (mut wallets, sender, _) = network_of_delegates();
    let full = Account::new("full recipient");
    let other = Account::new("other recipient");
    wallets
        .get_or_create(&full.address)
        .credit(u64::MAX)
        .unwrap();
    let items = vec![
        TransferItem {
            amount: 3,
            recipient: other.address,
        },
        TransferItem {
            amount: 1,
            recipient: full.address,
        },
    ];
    let tx = sender.sign(TransactionBuilder::transfer(items).nonce(1).fee(FEE));
    let before = wallets.clone();
    let rejected = Handlers.apply(&ctx(), &mut wallets, &tx).unwrap_err();
    assert_eq!(rejected.class, "Error");
    assert_eq!(visible(&wallets), visible(&before));
}

// ---- 1/1 second-signature registration ----------------------------------------------------

#[test]
fn a_second_signature_registration() {
    let (mut wallets, sender, _) = network_of_delegates();
    let second = Account::new("second passphrase");
    let register = |nonce| {
        TransactionBuilder::second_signature(second.public_key().to_compressed())
            .nonce(nonce)
            .fee(FEE)
    };
    let applied = apply_and_revert(&mut wallets, &sender.sign(register(1)));
    assert_eq!(
        wallet(&applied, &sender).second_public_key(),
        Some(&second.public_key().to_compressed())
    );
    assert!(wallet(&wallets, &sender).second_public_key().is_none());

    // The sender's own key, also when the wallet has not bound it yet.
    let own = TransactionBuilder::second_signature(sender.public_key().to_compressed())
        .nonce(1)
        .fee(FEE);
    assert_eq!(
        refused(&mut wallets, &sender.sign(own)),
        "PublicKeyAlreadyAssociatedWithWalletError"
    );
    let fresh = Account::new("fresh sender");
    wallets
        .get_or_create(&fresh.address)
        .credit(10 * FEE)
        .unwrap();
    let own = TransactionBuilder::second_signature(fresh.public_key().to_compressed())
        .nonce(1)
        .fee(FEE);
    assert_eq!(
        refused(&mut wallets, &fresh.sign(own)),
        "PublicKeyAlreadyAssociatedWithWalletError"
    );

    // Already registered: refused before the nonce is checked.
    wallets
        .get_or_create(&sender.address)
        .set_second_public_key(Some(second.public_key().to_compressed()));
    assert_eq!(
        refused(&mut wallets, &sender.sign(register(5))),
        "SecondSignatureAlreadyRegisteredError"
    );
}

// ---- 1/2 delegate registration ------------------------------------------------------------

#[test]
fn a_delegate_registration() {
    let (mut wallets, sender, delegates) = network_of_delegates();
    let register = |name: &str, nonce| {
        TransactionBuilder::delegate_registration(name)
            .nonce(nonce)
            .fee(FEE)
    };
    let applied = apply_and_revert(&mut wallets, &sender.sign(register("newcomer", 1)));
    let registered = wallet(&applied, &sender).delegate().expect("a delegate");
    assert_eq!(registered.username(), "newcomer");
    assert_eq!(
        (
            registered.vote_balance,
            registered.voters,
            registered.rank,
            registered.resigned
        ),
        (0, 0, None, None)
    );
    assert!(applied.has_by_username("newcomer"));
    assert!(!wallets.has_by_username("newcomer"));
    assert!(!wallet(&wallets, &sender).is_delegate());

    // A taken name, before the nonce is checked.
    assert_eq!(
        refused(&mut wallets, &sender.sign(register("del_b", 4))),
        "WalletUsernameAlreadyRegisteredError"
    );
    // A delegate may not register again, whatever the name.
    assert_eq!(
        refused(&mut wallets, &delegates[0].sign(register("del_b", 2))),
        "WalletIsAlreadyDelegateError"
    );
    assert_eq!(
        refused(&mut wallets, &delegates[0].sign(register("fresh_name", 2))),
        "WalletIsAlreadyDelegateError"
    );
}

// ---- 1/7 delegate resignation -------------------------------------------------------------

/// Four delegates, so that one may resign with three active delegates.
fn four_delegates() -> (WalletRepository, [Account; 4]) {
    let (mut wallets, _, [a, b, c]) = network_of_delegates();
    let d = Account::new("delegate d");
    delegate(&mut wallets, &d, "del_d");
    (wallets, [a, b, c, d])
}

fn resign_tx(
    account: &Account,
    resignation_type: Option<ResignationType>,
) -> heartwood_crypto::Transaction {
    account.sign(resignation(resignation_type).nonce(2).fee(FEE))
}

#[test]
fn resignations_from_an_active_delegate() {
    let (mut wallets, [a, ..]) = four_delegates();
    for (resignation_type, resigned) in [
        (None, Resignation::Temporary),
        (
            Some(ResignationType::TemporaryResign),
            Resignation::Temporary,
        ),
        (
            Some(ResignationType::PermanentResign),
            Resignation::Permanent,
        ),
    ] {
        let applied = apply_and_revert(&mut wallets, &resign_tx(&a, resignation_type));
        let delegate = wallet(&applied, &a).delegate().unwrap();
        assert_eq!(delegate.resigned, Some(resigned));
        assert_eq!(delegate.resigned_at, Some(HEIGHT));
        let delegate = wallet(&wallets, &a).delegate().unwrap();
        assert_eq!((delegate.resigned, delegate.resigned_at), (None, None));
    }
    assert_eq!(
        refused(
            &mut wallets,
            &resign_tx(&a, Some(ResignationType::NotResigned))
        ),
        "WalletNotResignedError"
    );
    assert_eq!(
        refused(&mut wallets, &resign_tx(&Account::new("delegate e"), None)),
        "WalletNotADelegateError"
    );
}

#[test]
fn resignations_from_a_permanently_resigned_delegate() {
    let (mut wallets, [a, ..]) = four_delegates();
    resign(&mut wallets, &a, Resignation::Permanent, 10);
    for (resignation_type, class) in [
        (
            Some(ResignationType::PermanentResign),
            "WalletAlreadyPermanentlyResignedError",
        ),
        (None, "IrrevocableResignationError"),
        (
            Some(ResignationType::TemporaryResign),
            "IrrevocableResignationError",
        ),
        (
            Some(ResignationType::NotResigned),
            "IrrevocableResignationError",
        ),
    ] {
        assert_eq!(
            refused(&mut wallets, &resign_tx(&a, resignation_type)),
            class
        );
    }
}

#[test]
fn resignations_from_a_temporarily_resigned_delegate() {
    let (mut wallets, [a, ..]) = four_delegates();
    let resigned_at = 100;
    resign(&mut wallets, &a, Resignation::Temporary, resigned_at);
    assert_eq!(
        refused(&mut wallets, &resign_tx(&a, None)),
        "WalletAlreadyTemporarilyResignedError"
    );
    // Temporary to permanent.
    let applied = apply_and_revert(
        &mut wallets,
        &resign_tx(&a, Some(ResignationType::PermanentResign)),
    );
    let delegate = wallet(&applied, &a).delegate().unwrap();
    assert_eq!(
        (delegate.resigned, delegate.resigned_at),
        (Some(Resignation::Permanent), Some(HEIGHT))
    );
    let delegate = wallet(&wallets, &a).delegate().unwrap();
    assert_eq!(
        (delegate.resigned, delegate.resigned_at),
        (Some(Resignation::Temporary), Some(resigned_at))
    );

    // The revoke waits 106 blocks from the resignation to the last block.
    let revoke = resign_tx(&a, Some(ResignationType::NotResigned));
    for (since, allowed) in [(0, false), (105, false), (106, true), (107, true)] {
        let last_block_height = resigned_at + since;
        let ctx = ApplyContext {
            height: last_block_height + 1,
            params: params(),
            last_block_height,
        };
        let checked = Handlers.throw_if_cannot_be_applied(&ctx, &wallets, &revoke);
        if allowed {
            checked.unwrap();
            let before = wallets.clone();
            let undo = Handlers.apply(&ctx, &mut wallets, &revoke).unwrap();
            let delegate = wallet(&wallets, &a).delegate().unwrap();
            assert_eq!(
                (delegate.resigned, delegate.resigned_at),
                (None, Some(ctx.height))
            );
            Handlers.revert(&ctx, &mut wallets, &revoke, &undo).unwrap();
            assert_eq!(visible(&wallets), visible(&before));
        } else {
            let error = checked.unwrap_err();
            assert_eq!(error.class(), "NotEnoughTimeSinceResignationError");
            assert_eq!(
                error,
                heartwood_transactions::TransactionError::NotEnoughTimeSinceResignation {
                    blocks: REVOKE_DELAY - u64::from(since)
                }
            );
        }
    }
}

#[test]
fn a_revoke_without_a_configured_delay() {
    let (mut wallets, [a, ..]) = four_delegates();
    resign(&mut wallets, &a, Resignation::Temporary, HEIGHT - 1);
    let milestones = milestones_with("")
        .all()
        .first()
        .map(|params| params.as_json().clone());
    assert!(milestones.is_some());
    let no_delay = {
        let text = r#"[{"height":1,"activeDelegates":3,"blockTime":8,"block":{"version":0},
        "epoch":"2026-01-01T00:00:00.000Z","reward":0,
        "burn":{"feeBasisPoints":9000,"txAmount":1000},"transfer":{}}]"#;
        heartwood_crypto::Milestones::load(text, &network()).unwrap()
    };
    let ctx = ApplyContext {
        height: HEIGHT,
        params: no_delay.at(HEIGHT),
        last_block_height: HEIGHT - 1,
    };
    let revoke = a.sign(
        resignation(Some(ResignationType::NotResigned))
            .nonce(2)
            .fee(FEE),
    );
    Handlers
        .throw_if_cannot_be_applied(&ctx, &wallets, &revoke)
        .unwrap();
    // The resignation must still be found up to the last block.
    resign(&mut wallets, &a, Resignation::Temporary, HEIGHT);
    let error = Handlers
        .throw_if_cannot_be_applied(&ctx, &wallets, &revoke)
        .unwrap_err();
    assert_eq!(error.class(), "TypeError");
    let delegate = wallets
        .wallet_mut(&a.address)
        .and_then(Wallet::delegate_mut)
        .unwrap();
    delegate.resigned_at = None;
    assert_eq!(
        Handlers
            .throw_if_cannot_be_applied(&ctx, &wallets, &revoke)
            .unwrap_err()
            .class(),
        "TypeError"
    );
}

#[test]
fn a_resignation_keeps_enough_delegates_for_a_round() {
    let (mut wallets, [a, b, c, d]) = four_delegates();
    // Four unresigned delegates and three active ones: one may resign.
    apply_and_revert(&mut wallets, &resign_tx(&a, None));
    resign(&mut wallets, &d, Resignation::Temporary, 10);
    for account in [&a, &b, &c] {
        for resignation_type in [None, Some(ResignationType::PermanentResign)] {
            assert_eq!(
                refused(&mut wallets, &resign_tx(account, resignation_type)),
                "NotEnoughDelegatesError"
            );
        }
    }
    // A resigned delegate is not counted again: it may resign permanently.
    apply_and_revert(
        &mut wallets,
        &resign_tx(&d, Some(ResignationType::PermanentResign)),
    );
    // The count comes before the base checks.
    let late = a.sign(resignation(None).nonce(9).fee(FEE));
    assert_eq!(refused(&mut wallets, &late), "NotEnoughDelegatesError");
}

#[test]
fn a_resignation_revert_needs_its_undo_record() {
    let (mut wallets, [a, ..]) = four_delegates();
    let tx = resign_tx(&a, None);
    let undo = Handlers.apply(&ctx(), &mut wallets, &tx).unwrap();
    assert_eq!(
        undo,
        TxUndo::Resignation {
            previous: None,
            previous_height: None
        }
    );
    let before = wallets.clone();
    let rejected = Handlers
        .revert(&ctx(), &mut wallets, &tx, &TxUndo::None)
        .unwrap_err();
    assert_eq!(rejected.class, "Error");
    assert_eq!(wallets, before);
}

// ---- 2/0 burn -----------------------------------------------------------------------------

#[test]
fn a_burn() {
    let (mut wallets, sender, _) = network_of_delegates();
    let before = wallet(&wallets, &sender).balance();
    let at_minimum = sender.sign(TransactionBuilder::burn(BURN_MINIMUM).nonce(1).fee(FEE));
    let applied = apply_and_revert(&mut wallets, &at_minimum);
    assert_eq!(
        wallet(&applied, &sender).balance(),
        before - BURN_MINIMUM - FEE
    );
    // Below the minimum: refused before the nonce is checked.
    let below = sender.sign(TransactionBuilder::burn(BURN_MINIMUM - 1).nonce(3).fee(FEE));
    assert_eq!(refused(&mut wallets, &below), "InsufficientBurnAmountError");
}

// ---- 2/2 vote -----------------------------------------------------------------------------

fn vote_tx(
    account: &Account,
    entries: &[(&str, u16)],
    nonce: u64,
) -> heartwood_crypto::Transaction {
    account.sign(
        TransactionBuilder::vote(votes(entries))
            .nonce(nonce)
            .fee(FEE),
    )
}

#[test]
fn a_vote_moves_the_shares() {
    let (mut wallets, sender, _) = network_of_delegates();
    let balance = wallet(&wallets, &sender).balance();
    let applied = apply_and_revert(
        &mut wallets,
        &vote_tx(&sender, &[("del_b", 3_333), ("del_a", 6_667)], 1),
    );
    let after = balance - FEE;
    // Shares of the balance after the fee; the remainder goes to the first entries in the kept
    // order (basis points descending).
    let a_share = after * 6_667 / 10_000;
    let b_share = after * 3_333 / 10_000;
    let remainder = after - a_share - b_share;
    assert_eq!(
        vote_balance(&applied, "del_a"),
        (a_share + remainder.min(1), 1)
    );
    assert_eq!(
        vote_balance(&applied, "del_b"),
        (b_share + remainder.saturating_sub(1), 1)
    );
    let kept: Vec<&str> = wallet(&applied, &sender)
        .votes()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(kept, ["del_a", "del_b"]);
    assert_eq!(vote_balance(&wallets, "del_a"), (0, 0));
}

#[test]
fn a_new_vote_replaces_the_old_one_and_its_revert_restores_it() {
    let (mut wallets, sender, _) = network_of_delegates();
    set_votes(&mut wallets, &sender, &[("del_a", 5_000), ("del_b", 5_000)]);
    let balance = wallet(&wallets, &sender).balance();
    let tx = vote_tx(&sender, &[("del_c", 10_000)], 1);
    let applied = apply_and_revert(&mut wallets, &tx);
    assert_eq!(vote_balance(&applied, "del_a"), (0, 0));
    assert_eq!(vote_balance(&applied, "del_b"), (0, 0));
    assert_eq!(vote_balance(&applied, "del_c"), (balance - FEE, 1));
    assert_eq!(vote_balance(&wallets, "del_a"), (balance / 2, 1));
    let undo = Handlers.apply(&ctx(), &mut wallets.clone(), &tx).unwrap();
    assert_eq!(
        undo,
        TxUndo::Vote {
            previous: votes(&[("del_a", 5_000), ("del_b", 5_000)])
        }
    );
    // An unvote.
    let applied = apply_and_revert(&mut wallets, &vote_tx(&sender, &[], 1));
    assert!(wallet(&applied, &sender).votes().is_empty());
    assert_eq!(vote_balance(&applied, "del_a"), (0, 0));
}

#[test]
fn vote_checks_in_order() {
    let (mut wallets, sender, [_, _, c]) = network_of_delegates();
    // More names than active delegates, some unknown, and a wrong nonce: the count comes first.
    let four = [
        ("del_a", 2_500),
        ("del_b", 2_500),
        ("del_c", 2_500),
        ("nobody", 2_500),
    ];
    assert_eq!(
        refused(&mut wallets, &vote_tx(&sender, &four, 7)),
        "VotedForTooManyDelegatesError"
    );
    // No vote to remove.
    assert_eq!(
        refused(&mut wallets, &vote_tx(&sender, &[], 7)),
        "NoVoteError"
    );
    // An unknown name, before the nonce.
    assert_eq!(
        refused(
            &mut wallets,
            &vote_tx(&sender, &[("del_a", 5_000), ("nobody", 5_000)], 7)
        ),
        "VotedForNonDelegateError"
    );
    assert_eq!(
        refused(&mut wallets, &vote_tx(&sender, &[("del_a", 10_000)], 7)),
        "UnexpectedNonceError"
    );
    // The same names and shares, in any order.
    set_votes(&mut wallets, &sender, &[("del_a", 4_000), ("del_b", 6_000)]);
    assert_eq!(
        refused(
            &mut wallets,
            &vote_tx(&sender, &[("del_a", 4_000), ("del_b", 6_000)], 7)
        ),
        "AlreadyVotedForSameDelegatesError"
    );
    // Other shares for the same names are a new vote.
    apply_and_revert(
        &mut wallets,
        &vote_tx(&sender, &[("del_a", 6_000), ("del_b", 4_000)], 1),
    );
    // A resigned delegate may still be voted for.
    resign(&mut wallets, &c, Resignation::Permanent, 10);
    apply_and_revert(&mut wallets, &vote_tx(&sender, &[("del_c", 10_000)], 1));
}

#[test]
fn a_failed_vote_balance_update_puts_every_wallet_back() {
    let (mut wallets, sender, [a, ..]) = network_of_delegates();
    set_votes(&mut wallets, &sender, &[("del_a", 10_000)]);
    // A delegate whose vote balance no longer holds the sender's share: a state no chain has.
    wallets
        .wallet_mut(&a.address)
        .and_then(Wallet::delegate_mut)
        .unwrap()
        .vote_balance = 0;
    let before = wallets.clone();
    let rejected = Handlers
        .apply(
            &ctx(),
            &mut wallets,
            &vote_tx(&sender, &[("del_b", 10_000)], 1),
        )
        .unwrap_err();
    assert_eq!(rejected.class, "Error");
    assert_eq!(visible(&wallets), visible(&before));
    assert!(!wallets.has_by_public_key(&sender.public_key().to_compressed()));
}

// ---- the genesis path ---------------------------------------------------------------------

fn genesis_ctx() -> ApplyContext<'static> {
    ApplyContext {
        height: 1,
        params: MILESTONES.at(1),
        last_block_height: 0,
    }
}

#[test]
fn genesis_transfers_credit_and_report_what_the_sender_owes() {
    let mut wallets = WalletRepository::new(NETWORK);
    let issuer = Account::new("issuer");
    let a = Account::new("holder a");
    let items = vec![
        TransferItem {
            amount: 70,
            recipient: a.address,
        },
        TransferItem {
            amount: 30,
            recipient: a.address,
        },
    ];
    let tx = issuer.sign_genesis(TransactionBuilder::transfer(items).nonce(1).fee(0));
    for (issuance, owed) in [(true, 0), (false, 100)] {
        let mut wallets = wallets.clone();
        assert_eq!(
            Handlers.bootstrap(&genesis_ctx(), &mut wallets, &tx, issuance),
            Ok(owed)
        );
        assert_eq!(wallet(&wallets, &a).balance(), 100);
        assert_eq!(
            wallet(&wallets, &issuer).public_key(),
            Some(issuer.public_key())
        );
        assert_eq!(wallet(&wallets, &issuer).balance(), 0);
        assert_eq!(wallet(&wallets, &issuer).nonce(), 0);
    }
    // A credit past 2^64 - 1 is refused and changes nothing.
    wallets
        .get_or_create(&a.address)
        .credit(u64::MAX - 99)
        .unwrap();
    let before = wallets.clone();
    assert!(
        Handlers
            .bootstrap(&genesis_ctx(), &mut wallets, &tx, true)
            .is_err()
    );
    assert_eq!(visible(&wallets), visible(&before));
}

#[test]
fn genesis_registrations_resignations_votes_and_second_keys() {
    let mut wallets = WalletRepository::new(NETWORK);
    let a = Account::new("genesis delegate");
    let second = Account::new("genesis second");
    let register = a.sign_genesis(
        TransactionBuilder::delegate_registration("genesis_a")
            .nonce(1)
            .fee(0),
    );
    let resign = a.sign_genesis(
        resignation(Some(ResignationType::PermanentResign))
            .nonce(2)
            .fee(0),
    );
    let vote = a.sign_genesis(
        TransactionBuilder::vote(votes(&[("genesis_a", 10_000)]))
            .nonce(3)
            .fee(0),
    );
    let second_key = a.sign_genesis(
        TransactionBuilder::second_signature(second.public_key().to_compressed())
            .nonce(4)
            .fee(0),
    );
    // A resignation needs the delegate first.
    let before = wallets.clone();
    let rejected = Handlers
        .bootstrap(&genesis_ctx(), &mut wallets, &resign, false)
        .unwrap_err();
    assert_eq!(rejected.class, "Error");
    assert_eq!(wallets, before);
    for tx in [&register, &resign, &vote, &second_key] {
        assert_eq!(
            Handlers.bootstrap(&genesis_ctx(), &mut wallets, tx, false),
            Ok(0)
        );
    }
    let built = wallet(&wallets, &a);
    let delegate = built.delegate().unwrap();
    assert_eq!(delegate.username(), "genesis_a");
    assert_eq!(
        (delegate.resigned, delegate.resigned_at),
        (Some(Resignation::Permanent), Some(1))
    );
    assert_eq!(built.votes(), &votes(&[("genesis_a", 10_000)]));
    // Vote balances are built after every genesis transaction is in.
    assert_eq!(delegate.vote_balance, 0);
    assert_eq!(
        built.second_public_key(),
        Some(&second.public_key().to_compressed())
    );
    assert_eq!((built.balance(), built.nonce()), (0, 0));
    // A second wallet may not take the name.
    let b = Account::new("genesis delegate b");
    let taken = b.sign_genesis(
        TransactionBuilder::delegate_registration("genesis_a")
            .nonce(1)
            .fee(0),
    );
    let before = wallets.clone();
    let rejected = Handlers
        .bootstrap(&genesis_ctx(), &mut wallets, &taken, false)
        .unwrap_err();
    assert_eq!(rejected.class, "WalletUsernameAlreadyRegisteredError");
    assert_eq!(wallets, before);
}

#[test]
fn the_type_keys() {
    let (_, sender, _) = network_of_delegates();
    for (builder, key, kind) in [
        (transfer(&sender, 1), "transfer", TransactionType::Transfer),
        (
            TransactionBuilder::burn(BURN_MINIMUM),
            "burn",
            TransactionType::Burn,
        ),
        (
            resignation(None),
            "delegateResignation",
            TransactionType::DelegateResignation,
        ),
    ] {
        let tx = sender.sign(builder.nonce(1).fee(FEE));
        assert_eq!((tx.key(), tx.kind()), (key, kind));
    }
}
