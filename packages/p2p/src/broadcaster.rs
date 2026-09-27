//! The transaction broadcast: batches of `maxTransactionsPerRequest` (40) to every peer, each
//! through the peer's own postTransactions queue.

use heartwood_crypto::Transaction;

use crate::peer::SharedPeer;
use crate::seams::PeerClient;

/// Broadcast `transactions` to `peers` through `client`, in batches of `batch_size`; a warning
/// and nothing else for no transaction.
pub fn broadcast_transactions<C: PeerClient>(
    client: &C,
    peers: &[SharedPeer],
    transactions: &[Transaction],
    batch_size: usize,
) {
    let _ = (client, peers, transactions, batch_size);
    todo!("the batches")
}
