//! getUnconfirmedTransactions and postTransactions: the messages and transaction lists from the
//! oracle's JSON, and the decoded values in its JSON (transactions and excluded ids as hex).

use heartwood_crypto::utils::hex;
use heartwood_p2p_wire::Codec;
use heartwood_p2p_wire::codecs::transactions::{self, UnconfirmedRequest, WithTransactions};
use heartwood_p2p_wire::messages::transactions::{
    GetUnconfirmedTransactionsRequest, GetUnconfirmedTransactionsResponse, PostTransactionsRequest,
};
use serde_json::{Value, json};

use super::blocks::{
    as_string, as_u32, field, headers, headers_value, hex_list, hex_list_value, unknown, verdict,
};
use super::object;
use crate::subject::Verdict;
use crate::subjects::{Outcome, failed};

/// The payload of `value` for `codec`.
pub(super) fn encode(codec: Codec, value: &Value) -> Outcome<Vec<u8>> {
    Ok(match codec {
        Codec::GetUnconfirmedTransactionsRequest => {
            let exclude = field(value, "exclude", |ids| {
                ids.as_array()?
                    .iter()
                    .map(|id| hex::decode_array(id.as_str()?).ok())
                    .collect()
            })?;
            transactions::get_unconfirmed_transactions_encode_request(&UnconfirmedRequest {
                request: GetUnconfirmedTransactionsRequest {
                    count_only: field(value, "countOnly", Value::as_bool)?,
                    headers: headers(value)?,
                    ..GetUnconfirmedTransactionsRequest::default()
                },
                exclude: exclude.unwrap_or_default(),
            })
        }
        Codec::GetUnconfirmedTransactionsResponse => {
            transactions::get_unconfirmed_transactions_encode_response(&WithTransactions {
                message: GetUnconfirmedTransactionsResponse {
                    pool_size: field(value, "poolSize", as_u32)?,
                    headers: headers(value)?,
                    ..GetUnconfirmedTransactionsResponse::default()
                },
                transactions: listed(value)?,
            })
        }
        Codec::PostTransactionsRequest => {
            transactions::post_transactions_encode_request(&WithTransactions {
                message: PostTransactionsRequest {
                    headers: headers(value)?,
                    ..PostTransactionsRequest::default()
                },
                transactions: listed(value)?,
            })
        }
        Codec::PostTransactionsResponse => {
            let accept = value
                .as_array()
                .and_then(|ids| ids.iter().map(as_string).collect::<Option<Vec<_>>>())
                .ok_or_else(|| failed("value: the accepted ids are not an array of text"))?;
            transactions::post_transactions_encode_response(&accept)
        }
        other => return Err(failed(format!("{other:?} is not a transaction codec"))),
    })
}

/// The verdict on `bytes` decoded with `codec`; its output is the decoded value.
pub(super) fn decode(codec: Codec, bytes: &[u8]) -> Outcome<Verdict> {
    match codec {
        Codec::GetUnconfirmedTransactionsRequest => verdict(
            transactions::get_unconfirmed_transactions_decode_request(bytes),
            |decoded| {
                let request = &decoded.request;
                object(request, |key| match key {
                    "countOnly" => Ok(json!(request.count_only)),
                    "exclude" => Ok(hex_list_value(&decoded.exclude)),
                    "headers" => headers_value(request.headers.as_ref()),
                    other => Err(unknown(other)),
                })
            },
        ),
        Codec::GetUnconfirmedTransactionsResponse => verdict(
            transactions::get_unconfirmed_transactions_decode_response(bytes),
            |decoded| {
                let response = &decoded.message;
                object(response, |key| match key {
                    "poolSize" => Ok(json!(response.pool_size)),
                    "transactions" => Ok(hex_list_value(&decoded.transactions)),
                    "headers" => headers_value(response.headers.as_ref()),
                    other => Err(unknown(other)),
                })
            },
        ),
        Codec::PostTransactionsRequest => verdict(
            transactions::post_transactions_decode_request(bytes),
            |decoded| {
                object(&decoded.message, |key| match key {
                    "transactions" => Ok(hex_list_value(&decoded.transactions)),
                    "headers" => headers_value(decoded.message.headers.as_ref()),
                    other => Err(unknown(other)),
                })
            },
        ),
        Codec::PostTransactionsResponse => verdict(
            transactions::post_transactions_decode_response(bytes),
            |accept| Ok(json!(accept)),
        ),
        other => Err(failed(format!("{other:?} is not a transaction codec"))),
    }
}

/// The transactions of a message that carries a list: required, as the reference's encoder reads
/// their lengths.
fn listed(value: &Value) -> Outcome<Vec<Vec<u8>>> {
    hex_list(value, "transactions")?
        .ok_or_else(|| failed("value.transactions is missing or not a list of hex"))
}
