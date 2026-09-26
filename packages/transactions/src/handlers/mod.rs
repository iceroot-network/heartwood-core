//! The transaction handlers.
//!
//! - [`transaction`]: what every type shares: the base checks, which run after the type's own
//!   checks (cold wallet, fee floor, nonce, balance, sender key, second signature), the sender's
//!   side of apply and revert, and the dynamic fee floor.
//! - [`core`]: the types of type group 1: transfer, second-signature registration, delegate
//!   registration and delegate resignation.
//! - [`group2`]: the types of type group 2: burn and vote.
//! - [`registry`]: [`registry::Handlers`], which dispatches on the transaction type.
//!
//! Each type's module has the same functions, which take the context, the wallets, the
//! transaction and, where the type has one, the part of the asset they read: `check` (the type's
//! checks, then the base checks), `apply` (check, then change the wallets, returning the undo
//! record), `revert` (with that record) and `bootstrap` (the genesis path); the types with
//! pool-entry rules also have `throw_if_cannot_enter_pool`.

pub mod core;
pub mod group2;
pub mod registry;
pub mod transaction;
