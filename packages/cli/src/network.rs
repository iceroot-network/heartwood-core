//! `network generate`: the files of a new network, from a profile and a seed.
//!
//! - The four crypto files of `heartwood_crypto::genesis::build`, byte for byte as the devnet
//!   generator writes them; `genesis-wallet.json` and `delegates.json` with every delegate's key,
//!   mode 0600.
//! - Unless `--chain-only`, a single-node instance: `.env` with loopback hosts and the testnet
//!   ports, `app.json`, `peers.json` naming the node itself, and `config.json`.
//! - The same seed and flags give the same bytes; the epoch is the clock's current second when
//!   none is given. A directory that is not empty is refused without `--force`.
//! - The devnet profile, which the genesis vectors record, is built in; `--profile FILE` names
//!   another.

use heartwood_kernel::Clock;

use crate::errors::NetworkError;
use crate::seams::{ChainGenerator, GenerateOptions, Generated};

/// Build and write the network of `options`, the epoch from `clock` when the options give none.
pub fn generate(options: &GenerateOptions, clock: &dyn Clock) -> Result<Generated, NetworkError> {
    let _ = (options, clock);
    todo!("network generate")
}

/// The [`ChainGenerator`] over [`generate`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkGenerator;

impl ChainGenerator for NetworkGenerator {
    fn generate(
        &self,
        options: &GenerateOptions,
        clock: &dyn Clock,
    ) -> Result<Generated, NetworkError> {
        generate(options, clock)
    }
}
