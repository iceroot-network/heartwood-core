//! The messages of `transactions.proto`: getUnconfirmedTransactions and postTransactions.

use super::shared::Headers;
use crate::proto::message;

message! {
    /// The getUnconfirmedTransactions request.
    GetUnconfirmedTransactionsRequest {
        /// Whether only the pool's size is wanted.
        1 count_only "countOnly": bool,
        /// The ids of transactions the client has, concatenated (32 bytes each).
        2 exclude "exclude": bytes,
        /// The client's headers.
        3 headers "headers": message(Headers),
    }
}

message! {
    /// The getUnconfirmedTransactions response.
    GetUnconfirmedTransactionsResponse {
        /// The number of transactions in the pool.
        1 pool_size "poolSize": u32,
        /// The transactions as a length-prefixed list (see [`crate::list`]).
        2 transactions "transactions": bytes,
        /// The server's headers.
        3 headers "headers": message(Headers),
    }
}

message! {
    /// The postTransactions request.
    PostTransactionsRequest {
        /// The transactions as a length-prefixed list (see [`crate::list`]).
        1 transactions "transactions": bytes,
        /// The client's headers.
        2 headers "headers": message(Headers),
    }
}

message! {
    /// The postTransactions response.
    PostTransactionsResponse {
        /// The ids of the transactions the server accepted, in hex.
        1 accept "accept": strings,
    }
}
