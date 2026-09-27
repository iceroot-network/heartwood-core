//! The route decoders on arbitrary bytes: the first byte selects one of the 14 codecs
//! (`Codec::select`) and the rest is its payload. No input may panic a decoder, and a value it
//! accepts encodes to bytes that decode to the same value, with the changes the reference's own
//! encoders make: a list loses a final empty item (its decoder never reads one), and the fields
//! that keep the text and bytes as they came hold what the encoder wrote.

#![no_main]

use std::fmt::Debug;

use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::transactions::{self, UnconfirmedRequest, WithTransactions};
use heartwood_p2p_wire::codecs::{Amount, peer};
use heartwood_p2p_wire::status::{self, ReceivedStatus};
use heartwood_p2p_wire::{Codec, DecodeError, list};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let (first, payload) = bytes.split_first().map_or((0, &[][..]), |(b, p)| (*b, p));
    match Codec::select(first) {
        Codec::GetPeersRequest => check(
            payload,
            peer::get_peers_decode_request,
            peer::get_peers_encode_request,
            unchanged,
        ),
        Codec::GetPeersResponse => check(
            payload,
            peer::get_peers_decode_response,
            |peers| peer::get_peers_encode_response(peers),
            unchanged,
        ),
        Codec::GetCommonBlocksRequest => check(
            payload,
            peer::get_common_blocks_decode_request,
            peer::get_common_blocks_encode_request,
            unchanged,
        ),
        Codec::GetCommonBlocksResponse => check(
            payload,
            peer::get_common_blocks_decode_response,
            peer::get_common_blocks_encode_response,
            unchanged,
        ),
        Codec::GetStatusRequest => check(
            payload,
            peer::get_status_decode_request,
            peer::get_status_encode_request,
            unchanged,
        ),
        Codec::GetStatusResponse => check(
            payload,
            status::decode_response,
            status::encode_response,
            status_written,
        ),
        Codec::GetBlocksRequest => check(
            payload,
            blocks::get_blocks_decode_request,
            blocks::get_blocks_encode_request,
            unchanged,
        ),
        Codec::GetBlocksResponse => check(
            payload,
            blocks::get_blocks_decode_response,
            |listed| blocks::get_blocks_encode_response(listed),
            |listed: &mut Vec<ListedBlock>| listed.iter_mut().for_each(block_written),
        ),
        Codec::PostBlockRequest => check(
            payload,
            blocks::post_block_decode_request,
            blocks::post_block_encode_request,
            unchanged,
        ),
        Codec::PostBlockResponse => check(
            payload,
            blocks::post_block_decode_response,
            blocks::post_block_encode_response,
            unchanged,
        ),
        Codec::GetUnconfirmedTransactionsRequest => check(
            payload,
            transactions::get_unconfirmed_transactions_decode_request,
            transactions::get_unconfirmed_transactions_encode_request,
            |request: &mut UnconfirmedRequest| {
                request.request.exclude = Some(request.exclude.as_flattened().to_vec());
            },
        ),
        Codec::GetUnconfirmedTransactionsResponse => check(
            payload,
            transactions::get_unconfirmed_transactions_decode_response,
            transactions::get_unconfirmed_transactions_encode_response,
            |response: &mut WithTransactions<_>| {
                response.message.transactions = Some(list::encode(&response.transactions));
                drop_final_empty(&mut response.transactions);
            },
        ),
        Codec::PostTransactionsRequest => check(
            payload,
            transactions::post_transactions_decode_request,
            transactions::post_transactions_encode_request,
            |request: &mut WithTransactions<_>| {
                request.message.transactions = Some(list::encode(&request.transactions));
                drop_final_empty(&mut request.transactions);
            },
        ),
        Codec::PostTransactionsResponse => check(
            payload,
            transactions::post_transactions_decode_response,
            |accept| transactions::post_transactions_encode_response(accept),
            unchanged,
        ),
    }
});

/// Decode `payload`; when the decoder accepts it, encode the value and decode that again, which
/// must give the value as `written` changes it.
fn check<T: PartialEq + Debug>(
    payload: &[u8],
    decode: impl Fn(&[u8]) -> Result<T, DecodeError>,
    encode: impl Fn(&T) -> Vec<u8>,
    written: impl Fn(&mut T),
) {
    let Ok(mut value) = decode(payload) else {
        return;
    };
    let again = decode(&encode(&value)).expect("an encoded value decodes");
    written(&mut value);
    assert_eq!(again, value);
}

/// A value whose encoding decodes to it unchanged.
fn unchanged<T>(_: &mut T) {}

/// A block of a getBlocks response as its encoding decodes: the amounts as decimal text, the
/// transactions as the list written, and that list's final empty item gone.
fn block_written(block: &mut ListedBlock) {
    let header = &mut block.header;
    header.total_amount = Some(block.total_amount.to_string());
    header.total_fee = Some(block.total_fee.to_string());
    header.reward = Some(block.reward.to_string());
    header.transactions = Some(list::encode(&block.transactions));
    drop_final_empty(&mut block.transactions);
}

/// A status as its encoding decodes: the header's amounts as decimal text.
fn status_written(status: &mut ReceivedStatus) {
    let amounts: [Amount; 3] = [status.total_amount, status.total_fee, status.reward];
    if let Some(header) = status
        .message
        .state
        .as_mut()
        .and_then(|state| state.header.as_mut())
    {
        let [total_amount, total_fee, reward] = amounts.map(|amount| Some(amount.to_string()));
        header.total_amount = total_amount;
        header.total_fee = total_fee;
        header.reward = reward;
    }
}

/// A list as its encoding decodes: the reference's decoder stops before a final empty item.
fn drop_final_empty(items: &mut Vec<Vec<u8>>) {
    if items.last().is_some_and(Vec::is_empty) {
        items.pop();
    }
}
