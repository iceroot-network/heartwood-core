//! The transaction broadcast: batches of `maxTransactionsPerRequest` (40) to every peer, each
//! through the peer's own postTransactions queue.

use std::sync::Arc;

use heartwood_crypto::Transaction;
use heartwood_kernel::{debug, warning};

use crate::peer::SharedPeer;
use crate::seams::PeerClient;

/// Broadcast `transactions` to `peers` through `client`, in batches of `batch_size` (at least
/// one); a warning and nothing else for no transaction. Every peer's queue gets the batches in
/// order, each batch as the transactions' bytes.
pub fn broadcast_transactions<C: PeerClient>(
    client: &C,
    peers: &[SharedPeer],
    transactions: &[Transaction],
    batch_size: usize,
) {
    if transactions.is_empty() {
        warning!("Broadcasting 0 transactions");
        return;
    }
    let peer_count = plural("peer", peers.len());
    for batch in transactions.chunks(batch_size.max(1)) {
        debug!(
            "Broadcasting {} to {peer_count} :moneybag:",
            plural("transaction", batch.len())
        );
        let bytes: Arc<Vec<Vec<u8>>> = Arc::new(
            batch
                .iter()
                .map(|transaction| transaction.bytes().to_vec())
                .collect(),
        );
        for peer in peers {
            client.post_transactions(peer, Arc::clone(&bytes));
        }
    }
}

/// `count word` or `count words`, with the count grouped by thousands, as the reference's
/// `pluralise(word, count, true)` writes it.
pub(crate) fn plural(word: &str, count: usize) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{} {word}{suffix}", grouped(count as u64))
}

/// `value` with its thousands separated by commas, as the reference's `toLocaleString()` writes
/// an integer.
pub(crate) fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len().saturating_add(digits.len() / 3));
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len().saturating_sub(index)) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_written_as_the_reference_writes_them() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(plural("peer", 1), "1 peer");
        assert_eq!(plural("peer", 0), "0 peers");
        assert_eq!(plural("block", 5_000), "5,000 blocks");
    }
}
