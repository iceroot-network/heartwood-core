//! The codecs of the transaction routes: getUnconfirmedTransactions and postTransactions.

use crate::errors::DecodeError;
use crate::list;
use crate::messages::shared::Headers;
use crate::messages::transactions::{
    GetUnconfirmedTransactionsRequest, GetUnconfirmedTransactionsResponse, PostTransactionsRequest,
    PostTransactionsResponse,
};
use crate::messages::{Proto, WireOrder};

/// The field number of `GetUnconfirmedTransactionsRequest.exclude`.
const EXCLUDE: u32 = 2;

/// A decoded getUnconfirmedTransactions request: `{...decoded, exclude}`.
///
/// The message's `exclude` holds the bytes as they came (none where it was absent, as it is
/// always present in a decoded request), and is noted in the wire order;
/// [`UnconfirmedRequest::exclude`] is the list the reference reads from them. The encoder takes an
/// [`UnconfirmedRequestInput`], which holds each value once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnconfirmedRequest {
    /// The message. When encoding, its `exclude` is not used.
    pub request: GetUnconfirmedTransactionsRequest,
    /// The excluded transaction ids: empty unless the bytes are a multiple of 32 long, and at
    /// most 15,001 ids.
    pub exclude: Vec<[u8; 32]>,
}

/// What the getUnconfirmedTransactions request encoder writes: each field once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnconfirmedRequestInput {
    /// `countOnly`, written when present. The reference's client never writes it.
    pub count_only: Option<bool>,
    /// The excluded transaction ids, written concatenated as `exclude`, even when there is none.
    pub exclude: Vec<[u8; 32]>,
    /// The client's headers, written when present.
    pub headers: Option<Headers>,
}

/// What the postTransactions request encoder writes: each field once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostTransactionsInput {
    /// The serialised transactions, written as a list, even when there is none.
    pub transactions: Vec<Vec<u8>>,
    /// The client's headers, written when present.
    pub headers: Option<Headers>,
}

/// A message whose `transactions` field is a list of transactions: `{...decoded, transactions}`.
///
/// The message's `transactions` holds the bytes as they came (none where it was absent, as it is
/// always present in a decoded message), and is noted in the wire order;
/// [`WithTransactions::transactions`] is the list the reference reads from them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WithTransactions<M> {
    /// The message. When encoding, its `transactions` is not used.
    pub message: M,
    /// The serialised transactions, at most 1,001.
    pub transactions: Vec<Vec<u8>>,
}

/// What the server writes in a getUnconfirmedTransactions response: the pool's size and the
/// collated transactions, each once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnconfirmedListing {
    /// The number of transactions in the pool.
    pub pool_size: u32,
    /// The serialised transactions, in the collator's order.
    pub transactions: Vec<Vec<u8>>,
}

/// The getUnconfirmedTransactions request: the ids concatenated as `exclude`, written even when
/// there is none.
pub fn get_unconfirmed_transactions_encode_request(request: &UnconfirmedRequestInput) -> Vec<u8> {
    GetUnconfirmedTransactionsRequest {
        count_only: request.count_only,
        exclude: Some(request.exclude.as_flattened().to_vec()),
        headers: request.headers.clone(),
        ..GetUnconfirmedTransactionsRequest::default()
    }
    .encode()
}

/// The getUnconfirmedTransactions request, with its exclude list read.
pub fn get_unconfirmed_transactions_decode_request(
    bytes: &[u8],
) -> Result<UnconfirmedRequest, DecodeError> {
    let mut request = GetUnconfirmedTransactionsRequest::decode(bytes)?;
    let exclude = list::decode_exclude(request.exclude.get_or_insert_default());
    request.wire_order.note(EXCLUDE);
    Ok(UnconfirmedRequest { request, exclude })
}

/// The getUnconfirmedTransactions response: the transactions as a list, always written.
pub fn get_unconfirmed_transactions_encode_response(
    response: &WithTransactions<GetUnconfirmedTransactionsResponse>,
) -> Vec<u8> {
    encode_listed(response)
}

/// The getUnconfirmedTransactions response as the server writes it from `listing`: the bytes
/// that [`get_unconfirmed_transactions_encode_response`] writes for the same values.
pub fn get_unconfirmed_transactions_encode_listing(listing: &UnconfirmedListing) -> Vec<u8> {
    let _ = listing;
    todo!("the getUnconfirmedTransactions response from the server's listing")
}

/// The getUnconfirmedTransactions response, with its transaction list read.
pub fn get_unconfirmed_transactions_decode_response(
    bytes: &[u8],
) -> Result<WithTransactions<GetUnconfirmedTransactionsResponse>, DecodeError> {
    decode_listed(bytes)
}

/// The postTransactions request: the transactions as a list, always written.
pub fn post_transactions_encode_request(request: &PostTransactionsInput) -> Vec<u8> {
    PostTransactionsRequest {
        transactions: Some(list::encode(&request.transactions)),
        headers: request.headers.clone(),
        ..PostTransactionsRequest::default()
    }
    .encode()
}

/// The postTransactions request, with its transaction list read.
pub fn post_transactions_decode_request(
    bytes: &[u8],
) -> Result<WithTransactions<PostTransactionsRequest>, DecodeError> {
    decode_listed(bytes)
}

/// The postTransactions response: `{accept}`.
pub fn post_transactions_encode_response(accept: &[String]) -> Vec<u8> {
    PostTransactionsResponse {
        accept: accept.to_vec(),
        ..PostTransactionsResponse::default()
    }
    .encode()
}

/// The postTransactions response: the decoded message's `accept`.
pub fn post_transactions_decode_response(bytes: &[u8]) -> Result<Vec<String>, DecodeError> {
    PostTransactionsResponse::decode(bytes).map(|response| response.accept)
}

/// A message with a `transactions` field that holds a list.
trait Listed: Proto + Clone {
    /// The field number of `transactions`.
    const TRANSACTIONS: u32;

    /// The `transactions` field and the wire order.
    fn listed(&mut self) -> (&mut Option<Vec<u8>>, &mut WireOrder);
}

impl Listed for GetUnconfirmedTransactionsResponse {
    const TRANSACTIONS: u32 = 2;

    fn listed(&mut self) -> (&mut Option<Vec<u8>>, &mut WireOrder) {
        (&mut self.transactions, &mut self.wire_order)
    }
}

impl Listed for PostTransactionsRequest {
    const TRANSACTIONS: u32 = 1;

    fn listed(&mut self) -> (&mut Option<Vec<u8>>, &mut WireOrder) {
        (&mut self.transactions, &mut self.wire_order)
    }
}

/// `{...value, transactions: list}`, encoded.
fn encode_listed<M: Listed>(value: &WithTransactions<M>) -> Vec<u8> {
    let mut message = value.message.clone();
    *message.listed().0 = Some(list::encode(&value.transactions));
    message.encode()
}

/// `{...decoded, transactions}`, with the list read from the field's bytes (none when absent).
fn decode_listed<M: Listed>(bytes: &[u8]) -> Result<WithTransactions<M>, DecodeError> {
    let mut message = M::decode(bytes)?;
    let (field, wire_order) = message.listed();
    let transactions = list::decode(field.get_or_insert_default(), list::MAX_TRANSACTIONS);
    wire_order.note(M::TRANSACTIONS);
    Ok(WithTransactions {
        message,
        transactions,
    })
}
