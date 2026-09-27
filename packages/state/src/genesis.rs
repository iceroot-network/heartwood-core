//! The state after the genesis block, as the reference builds it at its first start.
//!
//! 1. The block must be at height 1 and verify, except for the rule on future timestamps.
//! 2. What it issues: the items of every transfer whose sender key equals the block's generator
//!    key byte for byte; at most `2^63 − 1` in total.
//! 3. Every transaction is bootstrapped through [`crate::TransactionHandlers::bootstrap`], type by
//!    type in the order the reference registers its handlers (second signatures, delegate
//!    registrations, transfers, resignations, burns, votes) and in block order within a type;
//!    then each sender's nonce is the number of its transactions, and it pays the amounts, the
//!    fees and the items of its non-issuing transfers. All credits come before all debits, so a
//!    debit fails exactly when the built balance would be negative, which refuses the block.
//! 4. The delegates' vote balances are built from the voters; none may be negative.
//! 5. Round 1 starts as after the genesis block; the genesis block is not one of its blocks.
//!
//! The genesis block has no forger: its reward and its fees go to no wallet, so its fees and
//! burned amounts count as burned in the supply totals. Donations at height 1 would have to be
//! taken from a forger that does not exist, and the reference refuses to start with them.

use heartwood_crypto::blocks::VerifyFailure;
use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::Asset;
use heartwood_crypto::{Block, Milestones, TransactionType};
use heartwood_kernel::calculate_round;
use indexmap::IndexMap;

use crate::dpos::build_vote_balances;
use crate::errors::GenesisError;
use crate::handlers::{ApplyContext, TransactionHandlers};
use crate::machine::State;
use crate::round_state::RoundState;
use crate::stores::state::StateStore;
use crate::wallets::WalletRepository;

/// The most a genesis block may issue, so that every balance fits the signed 64-bit field the
/// reference saves it in.
pub const MAX_ISSUANCE: u64 = i64::MAX.unsigned_abs();

/// The order in which the reference's start-up bootstraps the transaction types: the order its
/// handlers are registered in.
const BOOTSTRAP_ORDER: [TransactionType; 6] = [
    TransactionType::SecondSignature,
    TransactionType::DelegateRegistration,
    TransactionType::Transfer,
    TransactionType::DelegateResignation,
    TransactionType::Burn,
    TransactionType::Vote,
];

/// What `genesis` issues: the sum of the items of the transfers sent with the generator's key;
/// `None` when they add up to more than [`MAX_ISSUANCE`].
pub fn issued(genesis: &Block) -> Option<u64> {
    let generator = genesis.header().generator_public_key;
    let mut issued: u64 = 0;
    for transaction in genesis.transactions() {
        let data = transaction.data();
        if let Asset::Transfer { transfers } = &data.asset
            && data.sender_public_key.as_bytes() == generator.as_bytes()
        {
            for item in transfers {
                issued = issued
                    .checked_add(item.amount)
                    .filter(|sum| *sum <= MAX_ISSUANCE)?;
            }
        }
    }
    Some(issued)
}

/// Check `genesis` as the reference does at start: height 1, and every rule of block
/// verification except the one on future timestamps. Every transaction type that decodes is
/// active at height 1.
pub fn validate<E>(genesis: &Block, milestones: &Milestones) -> Result<(), GenesisError<E>> {
    let height = genesis.height();
    if height != 1 {
        return Err(GenesisError::Height { height });
    }
    let failures: Vec<VerifyFailure> = genesis
        .verify(milestones, 0)
        .failures
        .into_iter()
        .filter(|failure| *failure != VerifyFailure::Timestamp)
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(GenesisError::Invalid { failures })
    }
}

/// The state after `genesis` (steps 1 to 5 of the module documentation).
pub fn build<H: TransactionHandlers>(
    genesis: &Block,
    milestones: &Milestones,
    handlers: &H,
) -> Result<State, GenesisError<H::Error>> {
    validate(genesis, milestones)?;
    let issued = issued(genesis).ok_or(GenesisError::Issuance)?;
    let params = milestones.at(1);
    let ctx = ApplyContext {
        height: 1,
        params,
        last_block_height: 0,
    };
    let generator = genesis.header().generator_public_key;
    let mut wallets = WalletRepository::new(params.network());

    // Per sender, in order of first use: its number of transactions and what it pays.
    let mut senders: IndexMap<Address, (u64, u128)> = IndexMap::new();
    let mut fees: u128 = 0;
    let mut burned_amounts: u128 = 0;
    for kind in BOOTSTRAP_ORDER {
        for (index, transaction) in genesis.transactions().iter().enumerate() {
            if transaction.kind() != kind {
                continue;
            }
            let data = transaction.data();
            let issuance = kind == TransactionType::Transfer
                && data.sender_public_key.as_bytes() == generator.as_bytes();
            let owed = handlers
                .bootstrap(&ctx, &mut wallets, transaction, issuance)
                .map_err(|rejected| GenesisError::Transaction {
                    index,
                    id: *transaction.id(),
                    rejected,
                })?;
            let sender = Address::from_public_key(&data.sender_public_key, params.network());
            let (count, pays) = senders.entry(sender).or_default();
            *count = count.saturating_add(1);
            *pays = pays
                .saturating_add(u128::from(owed))
                .saturating_add(u128::from(transaction.amount()))
                .saturating_add(u128::from(transaction.fee()));
            fees = fees.saturating_add(u128::from(transaction.fee()));
            burned_amounts = burned_amounts.saturating_add(u128::from(transaction.amount()));
        }
    }
    for (address, (count, pays)) in senders {
        let wallet = wallets.get_or_create(&address);
        wallet.set_nonce(count);
        let negative = || GenesisError::NegativeBalance { address };
        let pays = u64::try_from(pays).map_err(|_| negative())?;
        wallet.debit(pays).map_err(|_| negative())?;
    }
    if !genesis.donations(milestones).is_empty() {
        return Err(GenesisError::Donations);
    }
    build_vote_balances(&mut wallets)?;
    let round = RoundState::start(&mut wallets, calculate_round(2, milestones))?;

    // Every balance is now at least 0, so the fees and burned amounts, which left the issued
    // amount, add up to at most what was issued.
    let mut store = StateStore::new(genesis.clone(), issued);
    let totals = store.totals_mut();
    totals.burned_fees = u64::try_from(fees).unwrap_or(u64::MAX);
    totals.burned_amounts = u64::try_from(burned_amounts).unwrap_or(u64::MAX);
    Ok(State::from_parts(wallets, round, store))
}
