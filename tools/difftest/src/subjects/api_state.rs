//! The listings over the state: `api.delegatesPage` and `api.walletsPage` (a route's answer over a
//! wallet fixture) and `api.delegateResource` (one delegate's resource).
//!
//! A listing runs the crate's services as the routes run them, over the fixture's wallets in the
//! index orders the case gives (`orders`, permutations of fixture positions; the fixture's own
//! order by default), with the delegates' rank, missed blocks, productivity and version from
//! `extras`, and paged by the crate's pagination service, as the routes page them.

use std::net::{IpAddr, Ipv4Addr};

use heartwood_api::controllers::delegates::{delegates_page, find_delegate, listing};
use heartwood_api::controllers::wallets_list::wallets_page;
use heartwood_api::pagination::Pagination;
use heartwood_api::resources::delegate::{delegate, supply};
use heartwood_api::schemas::{parse_query, validate_params, validate_query};
use heartwood_api::search::criteria::{of_query, sorting_of};
use heartwood_api::search::{self, Criteria, Direction, plain};
use heartwood_api::{
    ApiError, ApiRequest, DelegateProduction, Hundredths, ProductivityTable, RouteId,
};
use heartwood_crypto::BlockId;
use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_pool::{OnlineEntry, PoolView, PoolViewOptions};
use heartwood_state::{Resignation, Wallet, WalletRepository};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed};
use crate::fixture::Fixture;
use crate::subject::{ApiDifference, Case, Documented, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("api.delegatesPage", delegates_route),
    ("api.walletsPage", wallets_route),
    ("api.delegateResource", delegate_resource),
];

/// The answer of a route that refused: its status, and the message of a 404.
fn refusal(error: &ApiError) -> Verdict {
    let mut answer = json!({ "status": error.status });
    if error.status == 404 {
        answer["message"] = json!(error.message);
    }
    Verdict::Output(answer)
}

// The fixture and what it states beside the wallets.

/// One listing's wallets: the fixture's, the delegates' extras, and the index orders.
struct State {
    wallets: WalletRepository,
    table: ProductivityTable,
    pool: PoolView,
    addresses: Vec<Address>,
    usernames: Vec<Address>,
    public_keys: Vec<Address>,
}

/// `input.orders.name` as a permutation of `wanted` (fixture positions), or `wanted` itself.
fn permutation(input: &Value, name: &str, wanted: Vec<usize>) -> Outcome<Vec<usize>> {
    let Some(order) = input.get("orders").and_then(|orders| orders.get(name)) else {
        return Ok(wanted);
    };
    let order: Vec<usize> = order
        .as_array()
        .and_then(|list| {
            list.iter()
                .map(|n| usize::try_from(n.as_u64()?).ok())
                .collect()
        })
        .ok_or_else(|| failed(format!("input.orders.{name} is not a list of positions")))?;
    let (mut sorted, mut expected) = (order.clone(), wanted);
    sorted.sort_unstable();
    expected.sort_unstable();
    if sorted != expected {
        return Err(failed(format!("input.orders.{name} is not a permutation")));
    }
    Ok(order)
}

/// The version an `extras` entry gives, as the pool's online record holds it.
fn online(wallets: &WalletRepository, extras: &Map<String, Value>) -> Outcome<Vec<OnlineEntry>> {
    let mut entries = Vec::new();
    for (username, extra) in extras {
        let Some(version) = extra.get("version").and_then(Value::as_str) else {
            continue;
        };
        let key = wallets
            .find_by_username(username)
            .and_then(Wallet::public_key)
            .ok_or_else(|| failed(format!("extras.{username}.version needs a delegate's key")))?;
        entries.push(OnlineEntry {
            public_key: key.to_compressed(),
            round: 5,
            version: version.to_owned(),
        });
    }
    entries.sort_by_key(|entry| entry.public_key);
    Ok(entries)
}

fn pool_of(online: Vec<OnlineEntry>) -> PoolView {
    let options = heartwood_pool::Options::defaults(&|_| None, std::path::Path::new("/none"), 4);
    PoolView {
        by_priority: Vec::new(),
        online,
        options: PoolViewOptions::of(&options),
    }
}

impl State {
    fn of(env: &Env<'_>, case: &Case<'_>) -> Outcome<State> {
        let input = case.input;
        let network = env.devnet()?.network.pub_key_hash;
        let fixture = Fixture::from_json(&input["fixture"])
            .map_err(|message| failed(format!("input.fixture: {message}")))?;
        let mut wallets = fixture
            .to_wallets(network)
            .map_err(|message| failed(format!("input.fixture: {message}")))?;
        let empty = Map::new();
        let extras = match input.get("extras") {
            None | Some(Value::Null) => &empty,
            Some(extras) => extras
                .as_object()
                .ok_or_else(|| failed("input.extras is not an object"))?,
        };
        let mut table = ProductivityTable::default();
        for (username, extra) in extras {
            let number = |key: &str| {
                extra
                    .get(key)
                    .map(|n| n.as_u64().and_then(|n| u32::try_from(n).ok()))
                    .map(|n| {
                        n.ok_or_else(|| failed(format!("extras.{username}.{key} is not a count")))
                    })
                    .transpose()
            };
            let address = *wallets
                .find_by_username(username)
                .ok_or_else(|| failed(format!("extras.{username}: no such delegate")))?
                .address();
            if let Some(rank) = number("rank")? {
                let record = wallets.wallet_mut(&address).and_then(Wallet::delegate_mut);
                record
                    .ok_or_else(|| failed("a delegate has a record"))?
                    .rank = Some(rank);
            }
            let (missed, points) = (number("missed")?, number("productivity")?);
            if missed.is_some() || points.is_some() {
                let production = DelegateProduction {
                    missed,
                    productivity: points.map(Hundredths),
                };
                table.by_username.insert(username.clone(), production);
            }
        }
        let pool = pool_of(online(&wallets, extras)?);
        let by_address = |positions: Vec<usize>| -> Outcome<Vec<Address>> {
            positions
                .into_iter()
                .map(|at| {
                    fixture
                        .wallets
                        .get(at)
                        .map(|wallet| wallet.address)
                        .ok_or_else(|| failed("a position is past the fixture"))
                })
                .collect()
        };
        let positions = |keep: &dyn Fn(&crate::fixture::FixtureWallet) -> bool| -> Vec<usize> {
            (0..fixture.wallets.len())
                .filter(|at| fixture.wallets.get(*at).is_some_and(keep))
                .collect()
        };
        let all = positions(&|_| true);
        let named = positions(&|wallet| wallet.delegate.is_some());
        let keyed = positions(&|wallet| wallet.public_key.is_some());
        Ok(State {
            addresses: by_address(permutation(input, "address", all)?)?,
            usernames: by_address(permutation(input, "username", named)?)?,
            public_keys: by_address(permutation(input, "publicKey", keyed)?)?,
            wallets,
            table,
            pool,
        })
    }

    /// The wallets at `addresses`, in that order.
    fn in_order<'a>(&'a self, addresses: &'a [Address]) -> impl Iterator<Item = &'a Wallet> {
        addresses
            .iter()
            .filter_map(|address| self.wallets.find_by_address(address))
    }
}

// The routes.

/// A route's request: its validated query and its parameters, or the refusal of the validation.
fn request(
    case: &Case<'_>,
    kinds: &[RouteId],
    default: &str,
) -> Outcome<Result<ApiRequest, Verdict>> {
    let path = case.input["path"].as_str().unwrap_or(default);
    let raw = super::text(case.input, "query")?;
    let path = format!("/api{path}");
    let route = crate::api::route_of("GET", &path)
        .filter(|route| kinds.contains(route))
        .ok_or_else(|| failed(format!("{path} is not a route of this operation")))?;
    let params = crate::api::params(route.path(), &path);
    let parsed = parse_query(raw);
    let status = |status: u16| Ok(Err(Verdict::Output(json!({ "status": status }))));
    if parsed.is_null() {
        return status(500);
    }
    if let Err(error) = validate_params(route, &params) {
        return status(error.status);
    }
    let query = match validate_query(route, &parsed) {
        Ok(query) => query,
        Err(error) => return status(error.status),
    };
    Ok(Ok(ApiRequest {
        route,
        params,
        path: path.strip_prefix("/api").unwrap_or(&path).to_owned(),
        query,
        payload: None,
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
    }))
}

/// A route's answer: the status and body of a reply, or the refusal.
fn answer(reply: Result<Value, ApiError>) -> Verdict {
    match reply {
        Ok(body) => Verdict::Output(json!({ "status": 200, "body": body })),
        Err(error) => refusal(&error),
    }
}

/// A wallet listing's answer: [`answer`], which differs from the reference's where a wallet's
/// second key came before its delegate record ([`ApiDifference::AttributeOrder`]; the oracle notes
/// its answer with the delegate first).
fn wallets_answer(reply: Result<Value, ApiError>) -> Verdict {
    Verdict::DiffersWhereShown {
        difference: Documented::Api(ApiDifference::AttributeOrder),
        verdict: Box::new(answer(reply)),
    }
}

fn delegates_route(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    const KINDS: [RouteId; 3] = [
        RouteId::DelegatesIndex,
        RouteId::DelegatesShow,
        RouteId::DelegatesVoters,
    ];
    let request = match request(case, &KINDS, "/delegates")? {
        Ok(request) => request,
        Err(verdict) => return Ok(verdict),
    };
    let state = State::of(env, case)?;
    let supply = supply(&state.wallets);
    let query = &request.query;
    let pagination = Pagination::of(query);
    if request.route == RouteId::DelegatesIndex {
        let parts = (pagination, sorting_of(query, &[]), &[of_query(query)][..]);
        let wallets = state.in_order(&state.usernames);
        let page = delegates_page(
            wallets,
            supply,
            &state.pool,
            &state.table,
            parts,
            search::page,
        );
        return Ok(answer(
            page.map(|page| listing(&request, pagination, page).body),
        ));
    }
    let id = request.param("id").unwrap_or_default();
    let wallet = match find_delegate(&state.wallets, id) {
        Ok(wallet) => wallet,
        Err(error) => return Ok(refusal(&error)),
    };
    if request.route == RouteId::DelegatesShow {
        let resource = delegate(&wallet, supply, &state.pool, &state.table);
        return Ok(answer(
            resource.map(|resource| json!({ "data": plain(resource) })),
        ));
    }
    let name = wallet.delegate().map(|record| record.username().to_owned());
    let mut voting = Map::new();
    voting.insert(
        "votingFor".to_owned(),
        json!([{ name.unwrap_or_default(): "*" }]),
    );
    let criteria = [of_query(query), Criteria(voting)];
    let tail = [("balance", Direction::Desc)];
    let parts = (pagination, sorting_of(query, &tail), &criteria[..]);
    let wallets = state.in_order(&state.public_keys);
    let page = wallets_page(wallets, &state.pool, &state.table, parts, search::page);
    Ok(wallets_answer(
        page.map(|page| listing(&request, pagination, page).body),
    ))
}

fn wallets_route(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let request = match request(
        case,
        &[RouteId::WalletsIndex, RouteId::WalletsTop],
        "/wallets",
    )? {
        Ok(request) => request,
        Err(verdict) => return Ok(verdict),
    };
    let state = State::of(env, case)?;
    let query = &request.query;
    let pagination = Pagination::of(query);
    let tail = [("balance", Direction::Desc)];
    let parts = (pagination, sorting_of(query, &tail), &[of_query(query)][..]);
    let wallets = state.in_order(&state.addresses);
    let page = wallets_page(wallets, &state.pool, &state.table, parts, search::page);
    Ok(wallets_answer(
        page.map(|page| listing(&request, pagination, page).body),
    ))
}

// One delegate's resource.

/// A decimal-string amount of `wallet.key`, 0 when absent.
fn amount_of(wallet: &Value, key: &str) -> Outcome<u64> {
    match wallet.get(key) {
        None => Ok(0),
        Some(_) => super::amount(wallet, key),
    }
}

fn delegate_resource(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let network = env.devnet()?.network.pub_key_hash;
    let w = &input["wallet"];
    let supply: u128 = super::text(input, "supply")?
        .parse()
        .map_err(|_| failed("input.supply is not an amount"))?;
    let address = Address::from_base58(super::text(w, "a")?)
        .map_err(|e| failed(format!("input.wallet.a: {e}")))?;
    let mut wallets = WalletRepository::new(network);
    wallets.get_or_create(&address);
    let mut key = None;
    if let Some(pk) = w.get("pk").and_then(Value::as_str) {
        let pk = PublicKey::from_hex(pk).map_err(|e| failed(format!("input.wallet.pk: {e}")))?;
        key = Some(pk.to_compressed());
        wallets.bind_public_key(&address, pk);
    }
    let count = |key: &str| w.get(key).and_then(Value::as_u64);
    let small = |key: &str| count(key).and_then(|n| u32::try_from(n).ok());
    let record = wallets
        .register_delegate(&address, super::text(w, "u")?.to_owned())
        .map_err(|e| failed(format!("input.wallet.u: {e}")))?;
    record.vote_balance = amount_of(w, "vb")?;
    record.voters = count("voters").unwrap_or(0);
    record.rank = small("rk");
    record.resigned = match count("rs") {
        None => None,
        Some(0) => Some(Resignation::Temporary),
        Some(1) => Some(Resignation::Permanent),
        Some(other) => return Err(failed(format!("input.wallet.rs is {other}"))),
    };
    record.produced_blocks = count("pb").unwrap_or(0);
    record.forged_fees = amount_of(w, "ff")?;
    record.forged_rewards = amount_of(w, "fr")?;
    record.burned_fees = amount_of(w, "bf")?;
    record.donations = amount_of(w, "dn")?;
    record.last_block = match w.get("lb").and_then(Value::as_str) {
        None => None,
        Some(id) => {
            Some(BlockId::from_hex(id).map_err(|e| failed(format!("input.wallet.lb: {e}")))?)
        }
    };
    let mut table = ProductivityTable::default();
    let (missed, points) = (small("missed"), small("productivity"));
    if missed.is_some() || points.is_some() {
        let production = DelegateProduction {
            missed,
            productivity: points.map(Hundredths),
        };
        table
            .by_username
            .insert(super::text(w, "u")?.to_owned(), production);
    }
    let mut versions = Vec::new();
    if let (Some(key), Some(version)) = (key, w.get("version").and_then(Value::as_str)) {
        let key: PublicKeyBytes = key;
        versions.push(OnlineEntry {
            public_key: key,
            round: 5,
            version: version.to_owned(),
        });
    }
    let pool = pool_of(versions);
    let wallet = wallets
        .find_by_address(&address)
        .ok_or_else(|| failed("the wallet is in its repository"))?;
    Ok(match delegate(wallet, supply, &pool, &table) {
        Ok(resource) => Verdict::Output(json!({ "resource": plain(resource) })),
        Err(error) => Verdict::Output(json!({ "status": error.status })),
    })
}
