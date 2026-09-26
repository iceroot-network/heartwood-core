//! The codecs of the transaction routes: getUnconfirmedTransactions and postTransactions.

use crate::errors::DecodeError;
use crate::messages::Proto;
use crate::messages::transactions::{
    GetUnconfirmedTransactionsRequest, GetUnconfirmedTransactionsResponse, PostTransactionsRequest,
    PostTransactionsResponse,
};

/// A getUnconfirmedTransactions request: `{...decoded, exclude}`.
///
/// The message's `exclude` holds the bytes as they came, and is noted in the wire order;
/// [`UnconfirmedRequest::exclude`] is the list the reference reads from them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnconfirmedRequest {
    /// The message. When encoding, its `exclude` is not used.
    pub request: GetUnconfirmedTransactionsRequest,
    /// The excluded transaction ids: empty unless the bytes are a multiple of 32 long, and at
    /// most 15,001 ids.
    pub exclude: Vec<[u8; 32]>,
}

/// A message whose `transactions` field is a list of transactions: `{...decoded, transactions}`.
///
/// The message's `transactions` holds the bytes as they came, and is noted in the wire order;
/// [`WithTransactions::transactions`] is the list the reference reads from them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WithTransactions<M> {
    /// The message. When encoding, its `transactions` is not used.
    pub message: M,
    /// The serialised transactions, at most 1,001.
    pub transactions: Vec<Vec<u8>>,
}

/// The getUnconfirmedTransactions request: the ids concatenated as `exclude`, written even when
/// there is none.
pub fn get_unconfirmed_transactions_encode_request(request: &UnconfirmedRequest) -> Vec<u8> {
    let _ = request;
    todo!()
}

/// The getUnconfirmedTransactions request, with its exclude list read.
pub fn get_unconfirmed_transactions_decode_request(
    bytes: &[u8],
) -> Result<UnconfirmedRequest, DecodeError> {
    let _ = bytes;
    todo!()
}

/// The getUnconfirmedTransactions response: the transactions as a list, always written.
pub fn get_unconfirmed_transactions_encode_response(
    response: &WithTransactions<GetUnconfirmedTransactionsResponse>,
) -> Vec<u8> {
    let _ = response;
    todo!()
}

/// The getUnconfirmedTransactions response, with its transaction list read.
pub fn get_unconfirmed_transactions_decode_response(
    bytes: &[u8],
) -> Result<WithTransactions<GetUnconfirmedTransactionsResponse>, DecodeError> {
    let _ = bytes;
    todo!()
}

/// The postTransactions request: the transactions as a list, always written.
pub fn post_transactions_encode_request(
    request: &WithTransactions<PostTransactionsRequest>,
) -> Vec<u8> {
    let _ = request;
    todo!()
}

/// The postTransactions request, with its transaction list read.
pub fn post_transactions_decode_request(
    bytes: &[u8],
) -> Result<WithTransactions<PostTransactionsRequest>, DecodeError> {
    let _ = bytes;
    todo!()
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
