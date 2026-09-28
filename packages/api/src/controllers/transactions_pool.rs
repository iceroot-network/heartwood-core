//! The pool routes and the static transaction routes: `POST /transactions` (at most the request
//! limit of entries, to the API channel's processor; `errors` absent when empty; 422 on a failed
//! validation), `GET /transactions/unconfirmed` (the pool from the highest priority, paginated,
//! `burnedFee` left out), `GET /transactions/unconfirmed/{id}` (raw by default, transformed for any
//! non-empty `transform`), `GET /transactions/types` (the six kept types) and
//! `GET /transactions/fees` (the static fees of the tip's milestone).

use std::sync::Arc;

use heartwood_crypto::managers::Params;
use heartwood_crypto::transactions::builders::static_fee;
use heartwood_crypto::utils::js;
use heartwood_crypto::{Milestones, Transaction, TransactionType};
use heartwood_pool::{Entry, ProcessorResult};
use serde_json::{Map, Value, json};

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;
use crate::pagination::{Pagination, envelope};
use crate::resources;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::TransactionsStore,
            handler: |ctx, request| Box::pin(store(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsUnconfirmed,
            handler: |ctx, request| Box::pin(unconfirmed(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsShowUnconfirmed,
            handler: |ctx, request| Box::pin(show_unconfirmed(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsTypes,
            handler: |ctx, request| Box::pin(types(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsFees,
            handler: |ctx, request| Box::pin(fees(ctx, request)),
        },
    ]
}

/// Submit transactions to the pool.
pub async fn store(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let payload = request.payload.as_ref();
    let transactions = (payload.and_then(|payload| payload.get("transactions")))
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::unprocessable("\"transactions\" is required"))?;
    let most = ctx.pool().options.max_transactions_per_request;
    if transactions.len() > most {
        let message = format!("\"transactions\" must contain less than or equal to {most} items");
        return Err(ApiError::unprocessable(message));
    }
    let entries = transactions.iter().cloned().map(Entry::Data).collect();
    let result = ctx
        .submit
        .submit(entries)
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(ApiReply::ok(store_body(&result)))
}

/// The answer of a submission: `data` with the admitted, broadcast, excess and refused entries'
/// ids, and `errors` when an entry was refused.
pub fn store_body(result: &ProcessorResult) -> Value {
    let (accept, broadcast) = (&result.accept, &result.broadcast);
    let (excess, invalid) = (&result.excess, &result.invalid);
    let data =
        json!({ "accept": accept, "broadcast": broadcast, "excess": excess, "invalid": invalid });
    let errors = result.to_json().get("errors").cloned();
    resources::defined(json!({ "data": data, "errors": errors }))
}

/// The pool's transactions.
pub async fn unconfirmed(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let pool = ctx.pool();
    let pagination = Pagination::of(&request.query);
    let transform = request.query.get("transform").is_some_and(js::is_truthy);
    let milestones = &ctx.node.crypto.milestones;
    let page = pool.by_priority.iter().skip(pagination.offset as usize);
    let page = page.take(pagination.limit as usize);
    let data = page
        .map(|transaction| shown(transaction, transform, milestones))
        .collect();
    let body = envelope(&request, pagination, pool.len() as u64, false, data);
    Ok(ApiReply::ok(body))
}

/// A pool transaction by id.
pub async fn show_unconfirmed(
    ctx: Arc<ApiContext>,
    request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    let id = request.param("id").unwrap_or_default();
    let pool = ctx.pool();
    let found = pool
        .by_priority
        .iter()
        .find(|transaction| transaction.id().to_hex() == id);
    let transaction = found.ok_or_else(|| ApiError::not_found("Transaction not found"))?;
    // The route has no query schema: any truthy `transform`, `false` included, transforms.
    let transform = request.query.get("transform").is_some_and(js::is_truthy);
    let milestones = &ctx.node.crypto.milestones;
    Ok(ApiReply::data(shown(transaction, transform, milestones)))
}

/// A pool transaction, transformed or as its data (the burned fee left out either way).
fn shown(transaction: &Transaction, transform: bool, milestones: &Milestones) -> Value {
    match transform {
        true => resources::transaction(transaction, None, milestones),
        false => resources::raw_transaction(transaction, milestones),
    }
}

/// The transaction types.
pub async fn types(ctx: Arc<ApiContext>, _request: ApiRequest) -> Result<ApiReply, ApiError> {
    let params = ctx.node.crypto.milestones.at(ctx.state().height());
    Ok(ApiReply::data(types_data(params)))
}

/// The static fees.
pub async fn fees(ctx: Arc<ApiContext>, _request: ApiRequest) -> Result<ApiReply, ApiError> {
    let params = ctx.node.crypto.milestones.at(ctx.state().height());
    Ok(ApiReply::data(fees_data(params)?))
}

/// The types active under `params`, in the reference's registration order: every kept type, the
/// burn only when the milestone sets `burn.txAmount` to a number.
fn active(params: &Params) -> impl Iterator<Item = TransactionType> {
    let burn = params
        .as_json()
        .get("burn")
        .map(|burn| burn.get("txAmount"));
    let burn_active = burn.flatten().is_some_and(Value::is_number);
    (TransactionType::ALL.into_iter())
        .filter(move |kind| *kind != TransactionType::Burn || burn_active)
}

/// `/transactions/types` under `params`: by type group, each type's key with a capital first
/// letter to its number.
pub fn types_data(params: &Params) -> Value {
    let capital = |key: &str| {
        let (first, rest) = key.split_at_checked(1).unwrap_or_default();
        first.to_uppercase() + rest
    };
    let types = by_group(params, |kind| {
        Ok((capital(kind.key()), json!(kind.type_id())))
    });
    types.unwrap_or_default()
}

/// `/transactions/fees` under `params`: by type group, each type's key to its static fee.
pub fn fees_data(params: &Params) -> Result<Value, ApiError> {
    by_group(params, |kind| {
        let fee = static_fee(kind, params).map_err(|_| ApiError::internal())?;
        Ok((kind.key().to_owned(), json!(fee.to_string())))
    })
}

/// The types active under `params`, by type group, each as `entry` names it and gives its value.
fn by_group(
    params: &Params,
    entry: impl Fn(TransactionType) -> Result<(String, Value), ApiError>,
) -> Result<Value, ApiError> {
    let mut groups = Map::new();
    for kind in active(params) {
        let (name, value) = entry(kind)?;
        let group = groups
            .entry(kind.type_group().to_string())
            .or_insert_with(|| json!({}));
        group.as_object_mut().map(|group| group.insert(name, value));
    }
    Ok(Value::Object(groups))
}
