//! The wallet listings over the state: `GET /wallets` and `GET /wallets/top` (one handler, over
//! the address index in first-use order).

use std::sync::Arc;

use heartwood_pool::PoolView;
use heartwood_state::Wallet;

use super::delegates::{Pager, listing, table};
use crate::context::{ApiContext, ApiReply, ApiRequest, ProductivityTable};
use crate::errors::ApiError;
use crate::pagination::Pagination;
use crate::resources::wallet_searchable;
use crate::search::criteria::{of_query, sorting_of};
use crate::search::{Criteria, Direction, ResultsPage, Sorting};

/// The page of the wallets in `wallets`' order that match every one of `criteria`, sorted by
/// `sorting`, then by balance, highest first.
pub fn wallets_page<'a>(
    wallets: impl Iterator<Item = &'a Wallet>,
    pool: &PoolView,
    table: &ProductivityTable,
    (pagination, sorting, criteria): (Pagination, Sorting, &[Criteria]),
    pager: impl Pager,
) -> Result<ResultsPage, ApiError> {
    let mut found = Vec::new();
    for wallet in wallets {
        let resource = wallet_searchable(wallet, pool, table);
        if crate::search::matches(&resource, criteria)? {
            found.push(resource);
        }
    }
    pager(pagination, &sorting, found.into_iter())
}

/// The listing of `GET /wallets` and `GET /wallets/top`.
async fn all(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let state = ctx.state();
    let delegates = state
        .wallets
        .iter()
        .any(|wallet| wallet.delegate().is_some());
    let table = if delegates {
        table(&ctx, &state)?
    } else {
        Arc::default()
    };
    let query = &request.query;
    let pagination = Pagination::of(query);
    let parts = (
        pagination,
        sorting_of(query, &[("balance", Direction::Desc)]),
        &[of_query(query)][..],
    );
    let wallets = state.wallets.by_address_index();
    let page = wallets_page(wallets, &ctx.pool(), &table, parts, crate::search::page)?;
    Ok(listing(&request, pagination, page))
}

/// `GET /wallets`: the wallets matching the criteria, sorted and paginated.
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    all(ctx, request).await
}

/// `GET /wallets/top`: the reference's duplicate of `GET /wallets`.
pub async fn top(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    all(ctx, request).await
}
