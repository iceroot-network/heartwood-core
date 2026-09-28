//! The peer routes: `GET /peers` (the `version` filter as a semver range, a bad range 404 "Invalid
//! version range provided"; the `ip` filter; latency order; pagination) and `GET /peers/{ip}` (404
//! "Peer not found").
//!
//! The version filter reads its range with the version gate's npm semver subset: a range outside
//! it (a hyphen range, an operator or a star inside a word) is answered as an invalid one, where
//! the reference would filter by it. The `ip` filter keeps the peers at exactly that address; the
//! reference validates it and ignores it.

use std::cmp::Ordering;
use std::net::IpAddr;
use std::sync::Arc;

use heartwood_crypto::utils::js;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_p2p::utils::npm_semver;
use serde_json::Value;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;
use crate::pagination::{Pagination, envelope};
use crate::plugins::percent_decode;
use crate::resources;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::PeersIndex,
            handler: |ctx, request| Box::pin(index(ctx, request)),
        },
        Route {
            id: RouteId::PeersShow,
            handler: |ctx, request| Box::pin(show(ctx, request)),
        },
    ]
}

/// The peers.
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let (page, total, pagination) = list(ctx.peers.peers(), &request.query)?;
    let data = page.iter().map(resources::peer).collect();
    Ok(ApiReply::ok(envelope(
        &request, pagination, total, false, data,
    )))
}

/// A peer by address.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let ip = request.param("ip").unwrap_or_default();
    let peers = ctx.peers.peers();
    match peers.iter().find(|peer| peer.ip.to_string() == ip) {
        Some(peer) => Ok(ApiReply::data(resources::peer(peer))),
        None => Err(ApiError::not_found("Peer not found")),
    }
}

/// The page of `peers` that the validated `query` asks for, the number of matches, and the page's
/// offset and limit: the filters, then the order (by latency unless `orderBy` names the version,
/// the height or the latency), then the page (`offset` when it is above 0, else `page`).
pub fn list(
    mut peers: Vec<PeerInfo>,
    query: &Value,
) -> Result<(Vec<PeerInfo>, u64, Pagination), ApiError> {
    let version = query
        .get("version")
        .filter(|version| js::is_truthy(version));
    if let Some(version) = version {
        let range = version.as_str().and_then(percent_decode);
        let range = range.ok_or_else(ApiError::internal)?;
        if !npm_semver::valid_range(&range) {
            return Err(ApiError::not_found("Invalid version range provided"));
        }
        let satisfied = |version: &String| npm_semver::satisfies(version, &range, false);
        peers.retain(|peer| peer.version.as_ref().is_some_and(satisfied));
    }
    if let Some(ip) = query.get("ip").and_then(Value::as_str) {
        let wanted = ip.parse::<IpAddr>().ok();
        peers.retain(|peer| Some(peer.ip) == wanted);
    }
    order(&mut peers, query.get("orderBy"))?;
    let number = |key: &str| query.get(key).map_or(0.0, number);
    // `+limit || 100`: NaN and 0 take the default; `offset` wins when it is above 0.
    let limit = Some(number("limit")).filter(|limit| *limit != 0.0 && !limit.is_nan());
    let limit = limit.unwrap_or(100.0);
    let offset = match number("offset") {
        offset if offset <= 0.0 && number("page") > 1.0 => (number("page") - 1.0) * limit,
        offset => offset,
    };
    let (offset, limit) = (whole(offset), whole(limit));
    let total = peers.len() as u64;
    let page = peers
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();
    Ok((page, total, Pagination { offset, limit }))
}

/// Sort `peers` as the reference does for `orderBy`: `version` by semver, `height` and `latency`
/// with unknown values last, in the direction given (`desc` unless it is `asc`, for these two);
/// anything else by latency, ascending. A list of orders is a 500, as in the reference.
fn order(peers: &mut [PeerInfo], order_by: Option<&Value>) -> Result<(), ApiError> {
    let order = match order_by {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(order)) => order.to_lowercase(),
        Some(_) => return Err(ApiError::internal()),
    };
    let (property, direction) = order.split_once(':').unwrap_or((&order, ""));
    let ascending = direction == "asc";
    match property {
        "version" => {
            let version = |peer: &PeerInfo| peer.version.clone().unwrap_or_default();
            if peers.len() > 1
                && peers
                    .iter()
                    .any(|peer| npm_semver::valid(&version(peer)).is_none())
            {
                return Err(ApiError::internal());
            }
            let semver = |a: &PeerInfo, b: &PeerInfo| npm_semver::compare(&version(a), &version(b));
            peers.sort_by(|a, b| {
                let (a, b) = if ascending { (a, b) } else { (b, a) };
                semver(a, b).unwrap_or(Ordering::Equal)
            });
        }
        "height" => peers.sort_by(|a, b| known_last(a.height, b.height, ascending)),
        "latency" => peers.sort_by(|a, b| known_last(a.latency, b.latency, ascending)),
        _ => peers.sort_by(|a, b| known_last(a.latency, b.latency, true)),
    }
    Ok(())
}

/// The order of two known or unknown values: the known ones in `ascending` order or its reverse,
/// the unknown ones last in both.
fn known_last(a: Option<u32>, b: Option<u32>, ascending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) if ascending => a.cmp(&b),
        (Some(a), Some(b)) => b.cmp(&a),
        (a, b) => a.is_none().cmp(&b.is_none()),
    }
}

/// `+value` for a validated query value: a number, or the number a text spells; NaN otherwise.
fn number(value: &Value) -> f64 {
    match value {
        Value::String(text) => text.trim().parse().unwrap_or(f64::NAN),
        value => value.as_f64().unwrap_or(f64::NAN),
    }
}

/// A count from a validated number: NaN and negatives are 0, fractions are cut.
fn whole(value: f64) -> u64 {
    if value > 0.0 { value as u64 } else { 0 }
}
