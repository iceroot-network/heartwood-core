//! The cases of the crypto files: the devnet's milestone and network files mutated (keys added at
//! random paths, `null` values, a height twice, value types changed, removed features turned on,
//! files left out), for `config.load`.

use std::sync::Arc;

use proptest::strategy::BoxedStrategy;

use crate::strategies::kernel::OpCase;
use crate::strategies::transactions::TxContext;

/// The cases of `config.load` over the devnet files of `context`.
pub fn config_cases(context: Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    todo!("the configuration cases")
}
