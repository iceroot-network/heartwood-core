//! The cost of re-indexing wallets, measured (ignored by default; `make pool-bench` runs it in a
//! release build): at 10^4, 10^5 and 3 x 10^5 wallets, the time to re-index a block's 300 parties
//! taken from the front of the address index (the median of 5 blocks, after a first block that
//! grows the copy's tables as the node's own state has grown them long before), and the time to
//! copy the repository (median of 5) and to build it again from its wallets.
//!
//! The reference keeps each index in a JavaScript `Map`, whose delete and set cost the same
//! whatever its size, and its handlers re-index every party of every transaction a block applies
//! or reverts, a pool admits or a forger collates. So a re-index must not cost more with more
//! wallets: the test fails when the block at 3 x 10^5 wallets costs more than 8 times the block
//! at 10^4. The threshold for a follow-up is the block above 20 ms at 3 x 10^5 wallets.

#![allow(
    clippy::disallowed_types,
    reason = "a measurement reads the wall clock"
)]

use std::hint::black_box;
use std::time::Instant;

use heartwood_crypto::KeyPair;
use heartwood_crypto::identities::Address;
use heartwood_state::{Wallet, WalletRepository};

const NETWORK: u8 = 90;

/// The parties of a block of 150 transfers.
const PARTIES: usize = 300;

/// `count` wallets with a balance, every tenth with a bound key, in the order they were created.
fn wallets(count: usize) -> WalletRepository {
    let mut wallets = WalletRepository::new(NETWORK);
    for n in 0..count {
        let address = if n % 10 == 0 {
            let keys = KeyPair::from_passphrase(&format!("reindex cost {n}")).unwrap();
            let address = Address::from_public_key(keys.public_key(), NETWORK);
            wallets.bind_public_key(&address, keys.public_key().clone());
            address
        } else {
            let mut bytes = [0u8; 21];
            bytes[0] = NETWORK;
            bytes[1..9].copy_from_slice(&(n as u64).to_le_bytes());
            Address::from_bytes(bytes)
        };
        wallets.get_or_create(&address).credit(1_000).unwrap();
    }
    wallets
}

/// The median of `values`.
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// The milliseconds `f` takes.
fn timed(f: impl FnOnce()) -> f64 {
    let start = Instant::now();
    f();
    start.elapsed().as_secs_f64() * 1_000.0
}

#[test]
#[ignore = "a measurement, run in a release build by make pool-bench"]
fn reindex_cost() {
    println!("wallets   block of {PARTIES} parties   copy   build from wallets");
    let mut blocks = Vec::new();
    for count in [10_000, 100_000, 300_000] {
        let repository = wallets(count);
        let mut copy = repository.clone();
        let front: Vec<Address> = (copy.by_address_index())
            .take(6 * PARTIES)
            .map(|wallet| *wallet.address())
            .collect();
        let mut blocks_of = front.chunks(PARTIES);
        if let Some(first) = blocks_of.next() {
            copy.reindex_parties(first.iter().copied());
        }
        let block = median(
            blocks_of
                .map(|parties| timed(|| copy.reindex_parties(parties.iter().copied())))
                .collect(),
        );
        let copy = median(
            (0..5)
                .map(|_| timed(|| drop(black_box(repository.clone()))))
                .collect(),
        );
        let rows: Vec<Wallet> = repository.iter().cloned().collect();
        let mut built = None;
        let build = timed(|| built = WalletRepository::from_wallets(NETWORK, rows).ok());
        assert_eq!(built.map(|built| built.len()), Some(count));
        println!("RESULT {count:>7} {block:>12.3} ms {copy:>9.3} ms {build:>9.3} ms");
        blocks.push(block);
    }
    let growth = blocks[2] / blocks[0];
    println!("a block at 300,000 wallets costs {growth:.1} times the block at 10,000");
    assert!(
        growth < 8.0,
        "a re-index grows with the number of wallets: {blocks:?} ms"
    );
}
