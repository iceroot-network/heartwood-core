//! The reply checks of the routes other than getStatus on arbitrary bytes: the first byte selects
//! the route (`byte % 6` over `heartwood_p2p_wire::replies::ROUTES`) and the rest is the reply. No
//! input may panic the codec or the check, and an accepted reply encodes to bytes that decode and
//! check to the same value, less a final empty transaction of a list, which the reference's list
//! reader never reads (it stops when four bytes are left).

#![no_main]

use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::{peer, transactions};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::replies::{self, ROUTES};
use libfuzzer_sys::fuzz_target;

/// The largest `block.maxTransactions` of the devnet milestones.
const MAX_TRANSACTIONS: usize = 150;

/// `list` less a final empty item, which a written list does not read back.
fn read_back(list: &[Vec<u8>]) -> &[Vec<u8>] {
    match list.split_last() {
        Some((last, rest)) if last.is_empty() => rest,
        _ => list,
    }
}

/// A block without the fields its encoder writes from elsewhere: the amounts' text and the
/// transactions' bytes, which read back as the amounts and the list.
fn without_written_fields(blocks: &[ListedBlock]) -> Vec<ListedBlock> {
    let mut blocks = blocks.to_vec();
    for block in &mut blocks {
        let header = &mut block.header;
        (
            header.total_amount,
            header.total_fee,
            header.reward,
            header.transactions,
        ) = (None, None, None, None);
    }
    blocks
}

fuzz_target!(|bytes: &[u8]| {
    let (first, reply) = bytes.split_first().map_or((0, &[][..]), |(b, r)| (*b, r));
    match ROUTES[usize::from(first) % ROUTES.len()] {
        RouteId::GetPeers => {
            let Ok(peers) = peer::get_peers_decode_response(reply) else {
                return;
            };
            let Ok(checked) = replies::check_peers(&peers) else {
                return;
            };
            let written = peer::get_peers_encode_response(&peers);
            let again = peer::get_peers_decode_response(&written).expect("written peers read back");
            assert_eq!(replies::check_peers(&again), Ok(checked));
        }
        RouteId::GetCommonBlocks => {
            let Ok(decoded) = peer::get_common_blocks_decode_response(reply) else {
                return;
            };
            let Ok(common) = replies::check_common_blocks(&decoded) else {
                return;
            };
            let again = peer::get_common_blocks_decode_response(&decoded.encode())
                .expect("a written common block reads back");
            assert_eq!(replies::check_common_blocks(&again), Ok(common));
        }
        RouteId::GetBlocks => {
            let Ok(mut listed) = blocks::get_blocks_decode_response(reply) else {
                return;
            };
            if replies::check_blocks(&mut listed).is_err() {
                return;
            }
            let written = blocks::get_blocks_encode_response(&listed);
            let mut again =
                blocks::get_blocks_decode_response(&written).expect("written blocks read back");
            assert_eq!(replies::check_blocks(&mut again), Ok(()));
            let mut expected = without_written_fields(&listed);
            for block in &mut expected {
                block.transactions = read_back(&block.transactions).to_vec();
            }
            assert_eq!(without_written_fields(&again), expected);
        }
        RouteId::PostBlock => {
            let Ok(decoded) = blocks::post_block_decode_response(reply) else {
                return;
            };
            let Ok(height) = replies::check_post_block(&decoded) else {
                return;
            };
            let again = blocks::post_block_decode_response(&decoded.encode())
                .expect("a written reply reads back");
            assert_eq!(replies::check_post_block(&again), Ok(height));
            assert_eq!(again.status, decoded.status);
        }
        RouteId::GetUnconfirmedTransactions => {
            let Ok(decoded) = transactions::get_unconfirmed_transactions_decode_response(reply)
            else {
                return;
            };
            if replies::check_unconfirmed(&decoded, MAX_TRANSACTIONS).is_err() {
                return;
            }
            let written = transactions::get_unconfirmed_transactions_encode_response(&decoded);
            let again = transactions::get_unconfirmed_transactions_decode_response(&written)
                .expect("a written reply reads back");
            assert_eq!(replies::check_unconfirmed(&again, MAX_TRANSACTIONS), Ok(()));
            assert_eq!(again.transactions, read_back(&decoded.transactions));
            assert_eq!(again.message.pool_size, decoded.message.pool_size);
        }
        _ => {
            let Ok(accept) = transactions::post_transactions_decode_response(reply) else {
                return;
            };
            if replies::check_post_transactions(&accept).is_err() {
                return;
            }
            let written = transactions::post_transactions_encode_response(&accept);
            let again = transactions::post_transactions_decode_response(&written)
                .expect("written ids read back");
            assert_eq!(again, accept);
        }
    }
});
