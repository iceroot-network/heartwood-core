//! A block's transactions and its forger's payout, applied and reverted, as the reference's
//! `BlockState` does.
//!
//! Applying a transaction runs its handler, then brings the vote balances up to date for every
//! wallet whose balance it changed: each transfer recipient in item order, then the sender.
//! Applying a block applies its transactions in order with the rules of the block's height; if
//! one is refused, the ones before it are reverted in reverse order and the error is returned,
//! so a refused block leaves the wallets as they were. Then the forger is paid: the reward less
//! the donations, plus the fees less their burned part; the donations go to their recipients.
//!
//! Reverting a block takes the payout back first, then reverts the transactions in reverse order.
//!
//! The only errors the reference can reach are the handlers' refusals, which change nothing. The
//! others (a balance, a delegate value or a total leaving the `u64` range, a vote for a name that
//! is not registered) cannot happen on a state built from accepted blocks; when one does, the
//! steps already taken are undone in reverse order before the error is returned.

use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::Asset;
use heartwood_crypto::{Block, BlockId, Milestones, Transaction, TransactionType};

use crate::errors::{ApplyError, RevertError, WalletError};
use crate::handlers::{ApplyContext, TransactionHandlers};
use crate::stores::state::Totals;
use crate::undo::{BlockUndo, ForgerUndo, TxUndo};
use crate::wallets::vote_balances::update_wallet_vote_balance;
use crate::wallets::{Delegate, WalletRepository};

/// Apply `transaction` with `handlers`, then update the vote balances of the wallets it changed.
pub fn apply_transaction<H: TransactionHandlers>(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    handlers: &H,
) -> Result<TxUndo, H::Error> {
    let undo = handlers.apply(ctx, wallets, transaction)?;
    if let Err(error) = refresh_vote_balances(wallets, transaction) {
        // Take the transaction back; its revert cannot fail right after its apply.
        let _ = handlers.revert(ctx, wallets, transaction, &undo);
        let _ = refresh_vote_balances(wallets, transaction);
        return Err(error.into());
    }
    Ok(undo)
}

/// Revert `transaction` with `handlers` and its undo record, then update the vote balances of
/// the wallets it changed.
pub fn revert_transaction<H: TransactionHandlers>(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    undo: &TxUndo,
    handlers: &H,
) -> Result<(), H::Error> {
    handlers.revert(ctx, wallets, transaction, undo)?;
    if let Err(error) = refresh_vote_balances(wallets, transaction) {
        // Apply the transaction again; its apply cannot fail right after its revert.
        let _ = handlers.apply(ctx, wallets, transaction);
        let _ = refresh_vote_balances(wallets, transaction);
        return Err(error.into());
    }
    Ok(())
}

/// Apply the transactions of `block` and pay its forger, adding the reward and the burned fees
/// and amounts to `totals`. Returns the block's undo record without a round part, which the
/// round state adds. On an error the wallets and `totals` are unchanged.
///
/// The forger is the wallet that has bound the block's generator key; it must be a registered
/// delegate before any transaction of the block is applied.
pub fn apply_block<H: TransactionHandlers>(
    wallets: &mut WalletRepository,
    totals: &mut Totals,
    block: &Block,
    milestones: &Milestones,
    handlers: &H,
) -> Result<BlockUndo, ApplyError<H::Error>> {
    let forger = forger_address(wallets, block)?;
    let payout = Payout::of(block, milestones).ok_or(ApplyError::TotalsOverflow)?;
    let after = totals
        .with_block(payout.reward, payout.burned_fee, payout.burned_amount)
        .ok_or(ApplyError::TotalsOverflow)?;
    let height = block.height();
    let ctx = context(height, milestones);

    let mut undos = Vec::with_capacity(block.transactions().len());
    for (index, transaction) in block.transactions().iter().enumerate() {
        match apply_transaction(&ctx, wallets, transaction, handlers) {
            Ok(undo) => undos.push(undo),
            Err(rejected) => {
                revert_applied(&ctx, wallets, block.transactions(), &undos, handlers);
                return Err(ApplyError::Transaction {
                    index,
                    id: *transaction.id(),
                    rejected,
                });
            }
        }
    }
    let last_block = match pay_forger(wallets, &forger, &payout, block.id()) {
        Ok(last_block) => last_block,
        Err(error) => {
            revert_applied(&ctx, wallets, block.transactions(), &undos, handlers);
            return Err(error.into());
        }
    };
    let before = std::mem::replace(totals, after);
    Ok(BlockUndo {
        block: block.clone(),
        transactions: undos,
        forger: ForgerUndo {
            address: forger,
            last_block,
        },
        totals: before,
        round: None,
    })
}

/// Revert the forger's payout of the block of `undo`, then its transactions in reverse order,
/// and restore `totals`. On an error the wallets and `totals` are unchanged.
pub fn revert_block<H: TransactionHandlers>(
    wallets: &mut WalletRepository,
    totals: &mut Totals,
    undo: &BlockUndo,
    milestones: &Milestones,
    handlers: &H,
) -> Result<(), RevertError<H::Error>> {
    let block = &undo.block;
    // The payout was computed when the block was applied, so it cannot fail here; if it did, the
    // totals could not be taken back.
    let payout = Payout::of(block, milestones).ok_or(RevertError::TotalsUnderflow)?;
    let ctx = context(block.height(), milestones);
    unpay_forger(wallets, &undo.forger, &payout)?;

    let transactions = block.transactions();
    let none = TxUndo::None;
    for (index, transaction) in transactions.iter().enumerate().rev() {
        let record = undo.transactions.get(index).unwrap_or(&none);
        if let Err(rejected) = revert_transaction(&ctx, wallets, transaction, record, handlers) {
            // Apply again what this revert took back, in block order, and pay the forger again.
            for later in transactions.iter().skip(index.saturating_add(1)) {
                let _ = apply_transaction(&ctx, wallets, later, handlers);
            }
            let _ = pay_forger(wallets, &undo.forger.address, &payout, block.id());
            return Err(RevertError::Transaction {
                index,
                id: *transaction.id(),
                rejected,
            });
        }
    }
    *totals = undo.totals;
    Ok(())
}

/// The rules block `height` is applied and reverted with: the milestone at `height`, and the
/// last block before it at `height − 1`.
fn context(height: u32, milestones: &Milestones) -> ApplyContext<'_> {
    ApplyContext {
        height,
        params: milestones.at(height),
        last_block_height: height.saturating_sub(1),
    }
}

/// The address of the delegate that forged `block`: the wallet that bound its generator key.
fn forger_address<E>(wallets: &WalletRepository, block: &Block) -> Result<Address, ApplyError<E>> {
    let generator = block.header().generator_public_key;
    wallets
        .find_by_public_key(&generator)
        .filter(|wallet| wallet.is_delegate())
        .map(|wallet| *wallet.address())
        .ok_or(ApplyError::GeneratorNotDelegate { generator })
}

/// Revert, in reverse order, the first `undos.len()` transactions of `transactions`, which were
/// just applied, so their reverts cannot fail.
fn revert_applied<H: TransactionHandlers>(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transactions: &[Transaction],
    undos: &[TxUndo],
    handlers: &H,
) {
    for (transaction, undo) in transactions.iter().zip(undos).rev() {
        let _ = revert_transaction(ctx, wallets, transaction, undo, handlers);
    }
}

/// Bring the delegates of every wallet whose balance `transaction` changed up to date: the
/// transfer recipients in item order (repeated ones repeatedly), then the sender.
fn refresh_vote_balances(
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Result<(), WalletError> {
    if let Asset::Transfer { transfers } = &transaction.data().asset {
        for item in transfers {
            update_wallet_vote_balance(wallets, &item.recipient)?;
        }
    }
    let sender = Address::from_public_key(&transaction.data().sender_public_key, wallets.network());
    update_wallet_vote_balance(wallets, &sender)
}

/// What a block pays its forger and moves in the totals, from the block's data under the
/// milestone at its height.
struct Payout {
    reward: u64,
    total_fee: u64,
    burned_fee: u64,
    donations: Vec<(Address, u64)>,
    donated: u64,
    burned_amount: u64,
}

impl Payout {
    /// The payout of `block`; `None` when a sum does not fit 64 bits, which no block whose
    /// header adds up can cause.
    fn of(block: &Block, milestones: &Milestones) -> Option<Payout> {
        let header = block.header();
        let donations = block.donations(milestones);
        let donated = donations
            .iter()
            .try_fold(0_u64, |sum, (_, amount)| sum.checked_add(*amount))?;
        let burned_amount = block
            .transactions()
            .iter()
            .filter(|transaction| transaction.kind() == TransactionType::Burn)
            .try_fold(0_u64, |sum, transaction| {
                sum.checked_add(transaction.amount())
            })?;
        Some(Payout {
            reward: header.reward,
            total_fee: header.total_fee,
            burned_fee: u64::try_from(block.burned_fee(milestones)).ok()?,
            donations,
            donated,
            burned_amount,
        })
    }

    /// The change of the forger's balance: the reward less the donations, plus the total fee
    /// less its burned part.
    fn forger_change(&self) -> i128 {
        // Four u64 values cannot leave the range of an i128.
        i128::from(self.reward)
            .saturating_sub(i128::from(self.donated))
            .saturating_add(i128::from(self.total_fee))
            .saturating_sub(i128::from(self.burned_fee))
    }

    /// The balance changes of the payout, in the reference's order: the forger, then each
    /// donation recipient in the order of the milestone.
    fn balance_changes(&self, forger: &Address) -> Vec<(Address, i128)> {
        std::iter::once((*forger, self.forger_change()))
            .chain(
                self.donations
                    .iter()
                    .map(|(address, amount)| (*address, i128::from(*amount))),
            )
            .collect()
    }

    /// The forger's delegate record after the payout of the block `id`.
    fn paid(&self, delegate: &Delegate, id: &BlockId) -> Option<Delegate> {
        let mut paid = delegate.clone();
        paid.produced_blocks = paid.produced_blocks.checked_add(1)?;
        paid.burned_fees = paid.burned_fees.checked_add(self.burned_fee)?;
        paid.forged_fees = paid.forged_fees.checked_add(self.total_fee)?;
        paid.forged_rewards = paid.forged_rewards.checked_add(self.reward)?;
        paid.donations = paid.donations.checked_add(self.donated)?;
        paid.last_block = Some(*id);
        Some(paid)
    }

    /// The forger's delegate record before the payout, whose last block was `last_block`.
    fn unpaid(&self, delegate: &Delegate, last_block: Option<BlockId>) -> Option<Delegate> {
        let mut unpaid = delegate.clone();
        unpaid.produced_blocks = unpaid.produced_blocks.checked_sub(1)?;
        unpaid.burned_fees = unpaid.burned_fees.checked_sub(self.burned_fee)?;
        unpaid.forged_fees = unpaid.forged_fees.checked_sub(self.total_fee)?;
        unpaid.forged_rewards = unpaid.forged_rewards.checked_sub(self.reward)?;
        unpaid.donations = unpaid.donations.checked_sub(self.donated)?;
        unpaid.last_block = last_block;
        Some(unpaid)
    }
}

/// Pay the forger at `forger` for block `id`: its delegate counters, its balance and the
/// donations, each balance followed by its vote-balance update. Returns the forger's previous
/// last block. On an error the wallets are unchanged.
fn pay_forger(
    wallets: &mut WalletRepository,
    forger: &Address,
    payout: &Payout,
    id: &BlockId,
) -> Result<Option<BlockId>, WalletError> {
    let before = delegate_of(wallets, forger)?.clone();
    let paid = payout.paid(&before, id).ok_or_else(|| range(&before))?;
    set_counters(wallets, forger, &paid)?;
    if let Err(error) = change_balances(wallets, &payout.balance_changes(forger)) {
        let _ = set_counters(wallets, forger, &before);
        return Err(error);
    }
    Ok(before.last_block)
}

/// Take back the payout that `undo`'s forger received, restoring its last block from `undo`.
/// On an error the wallets are unchanged.
fn unpay_forger(
    wallets: &mut WalletRepository,
    undo: &ForgerUndo,
    payout: &Payout,
) -> Result<(), WalletError> {
    let before = delegate_of(wallets, &undo.address)?.clone();
    let unpaid = payout
        .unpaid(&before, undo.last_block)
        .ok_or_else(|| range(&before))?;
    let changes: Vec<(Address, i128)> = payout
        .balance_changes(&undo.address)
        .into_iter()
        .map(|(address, change)| (address, change.saturating_neg()))
        .collect();
    set_counters(wallets, &undo.address, &unpaid)?;
    if let Err(error) = change_balances(wallets, &changes) {
        let _ = set_counters(wallets, &undo.address, &before);
        return Err(error);
    }
    Ok(())
}

/// The delegate record of the wallet at `address`.
fn delegate_of<'w>(
    wallets: &'w WalletRepository,
    address: &Address,
) -> Result<&'w Delegate, WalletError> {
    wallets
        .find_by_address(address)
        .ok_or(WalletError::UnknownWallet { address: *address })?
        .delegate()
        .ok_or(WalletError::NotADelegate { address: *address })
}

/// A delegate counter of `delegate` would leave its range.
fn range(delegate: &Delegate) -> WalletError {
    WalletError::DelegateRange {
        username: delegate.username().to_owned(),
    }
}

/// Set the payout counters and the last block of the delegate at `address` to those of `from`.
/// Its name, vote balance, voters, rank and resignation stay as they are.
fn set_counters(
    wallets: &mut WalletRepository,
    address: &Address,
    from: &Delegate,
) -> Result<(), WalletError> {
    let delegate = wallets
        .wallet_mut(address)
        .ok_or(WalletError::UnknownWallet { address: *address })?
        .delegate_mut()
        .ok_or(WalletError::NotADelegate { address: *address })?;
    delegate.produced_blocks = from.produced_blocks;
    delegate.burned_fees = from.burned_fees;
    delegate.forged_fees = from.forged_fees;
    delegate.forged_rewards = from.forged_rewards;
    delegate.donations = from.donations;
    delegate.last_block = from.last_block;
    Ok(())
}

/// Change each balance of `changes` in order, each followed by the update of the wallet's
/// delegates. When one fails, the ones before it are changed back in reverse order.
fn change_balances(
    wallets: &mut WalletRepository,
    changes: &[(Address, i128)],
) -> Result<(), WalletError> {
    for (done, (address, change)) in changes.iter().enumerate() {
        if let Err(error) = change_balance(wallets, address, *change) {
            for (address, change) in changes.iter().take(done).rev() {
                let _ = change_balance(wallets, address, change.saturating_neg());
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Add `change` (which may be negative) to the balance of the wallet at `address`, created if
/// needed, then update its delegates' vote balances. On an error the balance is unchanged.
fn change_balance(
    wallets: &mut WalletRepository,
    address: &Address,
    change: i128,
) -> Result<(), WalletError> {
    let wallet = wallets.get_or_create(address);
    let amount = u64::try_from(change.unsigned_abs());
    match (change >= 0, amount) {
        (true, Ok(amount)) => wallet.credit(amount)?,
        (false, Ok(amount)) => wallet.debit(amount)?,
        (true, Err(_)) => return Err(WalletError::BalanceOverflow { address: *address }),
        (false, Err(_)) => return Err(WalletError::NegativeBalance { address: *address }),
    }
    if let Err(error) = update_wallet_vote_balance(wallets, address) {
        let wallet = wallets.get_or_create(address);
        let _ = match amount {
            Ok(amount) if change >= 0 => wallet.debit(amount),
            Ok(amount) => wallet.credit(amount),
            Err(_) => Ok(()),
        };
        let _ = update_wallet_vote_balance(wallets, address);
        return Err(error);
    }
    Ok(())
}
