//! Wallets, delegates and votes.
//!
//! - [`wallet`]: [`Wallet`] and its [`Delegate`] record.
//! - [`wallet_repository`]: [`WalletRepository`], every wallet by address, created on first use.
//! - [`indexes`]: the public-key and username indexes of the repository.
//! - [`vote_balances`]: how a voter's balance splits over its delegates, and the delegates' vote
//!   balances.

pub mod indexes;
pub mod vote_balances;
pub mod wallet;
pub mod wallet_repository;

pub use self::indexes::Indexes;
pub use self::wallet::{Delegate, Resignation, Wallet};
pub use self::wallet_repository::WalletRepository;
