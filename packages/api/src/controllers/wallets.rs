//! The wallet routes: `GET /wallets/{id}` (by address, public key or username; a valid unknown
//! address is a zero wallet; another unknown 34-character id is 422 "Wallet not valid"; anything
//! else unknown 404; the basic wallet's attributes, the delegate's version from the online record)
//! and `GET /wallets/{id}/transactions` (the `address` criteria of the wallet, through the history
//! queries).

use std::sync::Arc;

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_state::{StateView, Wallet, WalletRepository};
use serde_json::Value;

use heartwood_database::TransactionCriteria;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::controllers::transactions::{Shape, criteria, listing};
use crate::errors::ApiError;
use crate::pagination::{Pagination, envelope};

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::WalletsShow,
            handler: |ctx, request| Box::pin(show(ctx, request)),
        },
        Route {
            id: RouteId::WalletsTransactions,
            handler: |ctx, request| Box::pin(transactions(ctx, request)),
        },
    ]
}

/// A wallet; a delegate's missed blocks and productivity from the node's table at the tip.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let state = ctx.state();
    let wallet = find(&state, request.param("id").unwrap_or_default())?;
    let table = match wallet.delegate() {
        Some(_) => super::delegates::table(&ctx, &state)?,
        None => Arc::default(),
    };
    Ok(ApiReply::data(crate::resources::wallet_with(
        &wallet,
        &ctx.pool(),
        &table,
    )))
}

/// A wallet's transactions: the confirmed transactions route's listing with the wallet's address
/// as its `address` criterion, in the envelope of this request (whose links keep the query as
/// given).
pub async fn transactions(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let wallet = find(&ctx.state(), request.param("id").unwrap_or_default())?;
    let mut listing = request.clone();
    if let Value::Object(query) = &mut listing.query {
        query.insert("address".into(), wallet.address().to_base58().into());
    }
    let reply = super::transactions::index(Arc::clone(&ctx), listing).await?;
    let field = |path: &str| reply.body.pointer(path).cloned().unwrap_or_default();
    let total = field("/meta/totalCount").as_u64().unwrap_or(0);
    let estimate = field("/meta/totalCountIsEstimate").as_bool() == Some(true);
    let data = field("/data").as_array().cloned().unwrap_or_default();
    let pagination = Pagination::of(&request.query);
    Ok(ApiReply::ok(envelope(
        &request, pagination, total, estimate, data,
    )))
}

/// The wallet that `id` names, as the reference's wallet search finds it: by address, by public
/// key, by delegate name, else a new wallet for a valid address of the network; 422 "Wallet not
/// valid" for another id of 34 characters, 404 "Wallet not found" for anything else.
pub fn find(state: &StateView, id: &str) -> Result<Wallet, ApiError> {
    find_in(&state.wallets, id)
}

/// [`find`] over the wallets alone.
pub fn find_in(wallets: &WalletRepository, id: &str) -> Result<Wallet, ApiError> {
    let network = wallets.network();
    // Each index is keyed by the canonical text: an address, a lowercase key, a name.
    let address = Address::from_base58_for(id, network).ok();
    let indexed = address.as_ref().filter(|address| address.to_base58() == id);
    let key = PublicKeyBytes::from_hex(id).ok();
    let key = key.filter(|key| key.to_hex() == id);
    let found = indexed.and_then(|address| wallets.find_by_address(address));
    let found = found.or_else(|| key.and_then(|key| wallets.find_by_public_key(&key)));
    let found = found.or_else(|| wallets.find_by_username(id));
    if let Some(wallet) = found {
        return Ok(wallet.clone());
    }
    match address {
        Some(address) => Ok(Wallet::new(address)),
        _ if id.encode_utf16().count() == 34 => Err(ApiError::unprocessable("Wallet not valid")),
        _ => Err(ApiError::not_found("Wallet not found")),
    }
}

/// `GET /wallets/{id}/transactions/sent`: the transactions a wallet sent, as the block-joined
/// resource; a wallet without a public key has sent none, and the answer is an empty page.
pub async fn sent(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let wallet = find(&ctx.state(), request.param("id").unwrap_or_default())?;
    if wallet.public_key().is_none() {
        return Ok(empty_page(&request));
    }
    let query = without(&request.query, "senderId");
    let sender_id = vec![wallet.address().to_base58()];
    let criteria = criteria(&query)?.map(|criteria| TransactionCriteria {
        sender_id,
        ..criteria
    });
    listing(&ctx, &request, criteria, Shape::WithBlock).await
}

/// `GET /wallets/{id}/transactions/received`: the transactions sent to a wallet. Unlike the sent
/// ones, a wallet without a public key has its page.
pub async fn received(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let wallet = find(&ctx.state(), request.param("id").unwrap_or_default())?;
    let query = without(&request.query, "recipientId");
    let recipient_id = vec![wallet.address().to_base58()];
    let criteria = criteria(&query)?.map(|criteria| TransactionCriteria {
        recipient_id,
        ..criteria
    });
    listing(&ctx, &request, criteria, Shape::WithBlock).await
}

/// `GET /wallets/{id}/votes`: the vote transactions a wallet sent; an empty page for a wallet
/// without a public key.
pub async fn votes(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let wallet = find(&ctx.state(), request.param("id").unwrap_or_default())?;
    if wallet.public_key().is_none() {
        return Ok(empty_page(&request));
    }
    let sender = Some(wallet.address().to_base58());
    super::votes::votes(&ctx, &request, sender, Shape::WithBlock).await
}

/// The query without `name`, which the route sets itself.
fn without(query: &Value, name: &str) -> Value {
    let mut query = query.clone();
    if let Value::Object(map) = &mut query {
        map.shift_remove(name);
    }
    query
}

/// The page of a wallet with nothing to list: no data, a count of 0 and no estimate.
fn empty_page(request: &ApiRequest) -> ApiReply {
    let pagination = Pagination::of(&request.query);
    ApiReply::ok(envelope(request, pagination, 0, false, Vec::new()))
}
