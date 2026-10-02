//! The delegate routes over the state: `GET /delegates` (the delegate search over the username
//! index), `GET /delegates/{id}` (one delegate's resource) and `GET /delegates/{id}/voters` (the
//! wallets that vote for the delegate, over the public-key index). As on the reference, a route of
//! one delegate answers the 500 of a resource that cannot be made even where it shows only the
//! delegate's name ([`named_delegate`]).

use std::sync::Arc;

use heartwood_pool::PoolView;
use heartwood_state::{StateView, Wallet, WalletRepository};
use serde_json::{Map, Value, json};

use crate::context::{ApiContext, ApiReply, ApiRequest, ProductivityTable};
use crate::errors::ApiError;
use crate::pagination::{Pagination, envelope};
use crate::resources::delegate::{check_resource, delegate, supply};
use crate::search::criteria::{of_query, sorting_of};
use crate::search::{Criteria, Direction, ResultsPage, Sorting, plain};

/// What pages a listing: the page of the collected resources in an order, with the count.
pub trait Pager:
    FnOnce(Pagination, &Sorting, std::vec::IntoIter<Value>) -> Result<ResultsPage, ApiError>
{
}

impl<F> Pager for F where
    F: FnOnce(Pagination, &Sorting, std::vec::IntoIter<Value>) -> Result<ResultsPage, ApiError>
{
}

/// The delegates' missed blocks and productivity at the tip.
pub(crate) fn table(
    ctx: &ApiContext,
    state: &StateView,
) -> Result<Arc<ProductivityTable>, ApiError> {
    ctx.productivity.table(&state.last_block)
}

/// The page of the delegates among `wallets` (in the username index's order) that match every
/// one of `criteria`, sorted by `sorting`, then by rank, then by name.
pub fn delegates_page<'a>(
    wallets: impl Iterator<Item = &'a Wallet>,
    supply: u128,
    pool: &PoolView,
    table: &ProductivityTable,
    (pagination, sorting, criteria): (Pagination, Sorting, &[Criteria]),
    pager: impl Pager,
) -> Result<ResultsPage, ApiError> {
    let mut found = Vec::new();
    for wallet in wallets {
        let resource = delegate(wallet, supply, pool, table)?;
        if crate::search::matches(&resource, criteria)? {
            found.push(resource);
        }
    }
    let tail = [("rank", Direction::Asc), ("username", Direction::Asc)];
    let sorting = Sorting(
        sorting
            .0
            .into_iter()
            .chain(tail.map(|(p, d)| (p.to_owned(), d)))
            .collect(),
    );
    pager(pagination, &sorting, found.into_iter())
}

/// `GET /delegates`: the delegates matching the criteria, sorted and paginated.
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let state = ctx.state();
    let table = table(&ctx, &state)?;
    let query = &request.query;
    let pagination = Pagination::of(query);
    let parts = (pagination, sorting_of(query, &[]), &[of_query(query)][..]);
    let supply = supply(&state.wallets);
    let wallets = state.wallets.by_username_index();
    let page = delegates_page(
        wallets,
        supply,
        &ctx.pool(),
        &table,
        parts,
        crate::search::page,
    )?;
    Ok(listing(&request, pagination, page))
}

/// The envelope of `page` with its resources written out.
pub fn listing(request: &ApiRequest, pagination: Pagination, page: ResultsPage) -> ApiReply {
    let data = page.results.into_iter().map(plain).collect();
    let total = (page.total_count, page.total_count_is_estimate);
    ApiReply::ok(envelope(request, pagination, total.0, total.1, data))
}

/// The delegate that `id` names, as the reference's wallet search finds it: by address, public
/// key or name, with the wallet's resource; 404 "Delegate not found" for any other.
pub fn find_delegate(wallets: &WalletRepository, id: &str) -> Result<Wallet, ApiError> {
    let not_found = || ApiError::not_found("Delegate not found");
    let wallet = super::wallets::find_in(wallets, id).map_err(|_| not_found())?;
    match wallets.find_by_address(wallet.address()) {
        Some(known) if known.delegate().is_some() => Ok(known.clone()),
        _ => Err(not_found()),
    }
}

/// The name of the delegate that `id` names, for the routes of one delegate that use only its
/// name (its voters, its blocks and its missed blocks): [`find_delegate`]'s wallet. The reference
/// builds that delegate's resource at the supply of every wallet before any of these routes
/// answers (`getDelegate`), so where the resource cannot be made ([`check_resource`]) they answer
/// its 500.
pub fn named_delegate(wallets: &WalletRepository, id: &str) -> Result<String, ApiError> {
    let wallet = find_delegate(wallets, id)?;
    check_resource(&wallet, supply(wallets))?;
    let name = wallet.delegate().map(|delegate| delegate.username());
    Ok(name.unwrap_or_default().to_owned())
}

/// `GET /delegates/{id}`: one delegate, by username, address or public key.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let state = ctx.state();
    let wallet = find_delegate(&state.wallets, request.param("id").unwrap_or_default())?;
    let table = table(&ctx, &state)?;
    let resource = delegate(&wallet, supply(&state.wallets), &ctx.pool(), &table)?;
    Ok(ApiReply::data(plain(resource)))
}

/// `GET /delegates/{id}/voters`: the wallets voting for a delegate.
pub async fn voters(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let state = ctx.state();
    let name = named_delegate(&state.wallets, request.param("id").unwrap_or_default())?;
    let table = table(&ctx, &state)?;
    let mut voting = Map::new();
    voting.insert("votingFor".to_owned(), json!([{ name: "*" }]));
    let query = &request.query;
    let pagination = Pagination::of(query);
    let criteria = [of_query(query), Criteria(voting)];
    let parts = (
        pagination,
        sorting_of(query, &[("balance", Direction::Desc)]),
        &criteria[..],
    );
    let wallets = state.wallets.by_public_key_index();
    let page = super::wallets_list::wallets_page(
        wallets,
        &ctx.pool(),
        &table,
        parts,
        crate::search::page,
    )?;
    Ok(listing(&request, pagination, page))
}
