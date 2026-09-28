//! The peer-to-peer server's reads on the read-only connections: the blocks of a download, with
//! or without their transactions, and the stored blocks among a peer's ids.

mod common;

use heartwood_crypto::BlockId;
use heartwood_database::Readers;

use common::{block_id, block_row, chain, transaction_row};

#[test]
fn a_download_reads_the_range_in_height_order_with_the_transactions_by_sequence() {
    let (_database, settings) = chain("download-range", 7);
    let readers = Readers::open(&settings, 1).unwrap();
    let reader = readers.get();
    let blocks = reader.blocks_for_download(3, 3, false).unwrap();
    let heights: Vec<u32> = blocks.iter().map(|block| block.row.height).collect();
    assert_eq!(heights, [3, 4, 5]);
    assert_eq!(blocks[0].row, block_row(3, 0));
    assert_eq!(blocks[0].transactions, Some(Vec::new()));
    let five: Vec<Vec<u8>> = (0..2).map(|s| transaction_row(5, s).serialised).collect();
    assert_eq!(blocks[2].transactions.as_ref(), Some(&five));
    let headers = reader.blocks_for_download(3, 3, true).unwrap();
    assert!(headers.iter().all(|block| block.transactions.is_none()));
    assert_eq!(headers.len(), 3);
    // Nothing above the tip; a limit of zero reads nothing.
    let tail = reader.blocks_for_download(6, 400, false).unwrap();
    assert_eq!(
        tail.iter().map(|b| b.row.height).collect::<Vec<_>>(),
        [6, 7]
    );
    assert!(reader.blocks_for_download(3, 0, false).unwrap().is_empty());
    assert!(reader.blocks_for_download(8, 10, true).unwrap().is_empty());
    assert_eq!(
        reader
            .blocks_for_download(u32::MAX, u32::MAX, true)
            .unwrap(),
        []
    );
}

#[test]
fn the_blocks_among_ids_are_the_stored_ones_each_once_in_height_order() {
    let (_database, settings) = chain("download-ids", 6);
    let readers = Readers::open(&settings, 1).unwrap();
    let reader = readers.get();
    let unknown = BlockId::from_array([0x42; 32]);
    let ids = [block_id(5), unknown, block_id(2), block_id(5)];
    let rows = reader.blocks_by_ids(&ids).unwrap();
    assert_eq!(rows, [block_row(2, 2), block_row(5, 2)]);
    assert!(reader.blocks_by_ids(&[]).unwrap().is_empty());
    assert!(reader.blocks_by_ids(&[unknown]).unwrap().is_empty());
}
