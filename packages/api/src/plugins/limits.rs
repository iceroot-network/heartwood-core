//! The rate limit and the semaphore.
//!
//! The rate limit keeps a fixed window per client address, as the reference's memory limiter does:
//! a window starts at the first point charged and lasts its duration (a window of 0 s never ends);
//! a point charged at or after its end starts a new one; points beyond the limit still count.
//!
//! The semaphore bounds the requests that the store's routes serve at once (the reference's
//! `database` settings; the `memory` ones concern routes a relay does not serve), at two levels: a
//! request that asks for more work than the first level allows (a large offset or page, an order
//! or a criterion the route's indexes do not serve) takes the second. A request finds a free place
//! or waits in its level's queue; with the queue full it is refused with 429.
//! The reference's semaphore never applies (its options are read from a path that does not
//! exist); this one applies the reference's settings as they are meant.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{Semaphore as Places, SemaphorePermit};
use tokio::time::Instant;

use crate::context::RouteId;
use crate::errors::ApiError;
use crate::server::{Semaphore, SemaphoreLevel};

/// The windows kept before the ended ones are dropped; the bound doubles with the live ones.
const PRUNE_AT: usize = 4_096;

/// The fixed-window rate limit of the API, per client address.
#[derive(Debug)]
pub(crate) struct RateLimiter {
    points: u32,
    duration: Option<Duration>,
    windows: HashMap<String, (Option<Instant>, u64)>,
    prune_at: usize,
}

impl RateLimiter {
    /// `points` per window of `duration_s` seconds.
    pub(crate) fn new(points: u32, duration_s: u32) -> RateLimiter {
        let duration = (duration_s > 0).then(|| Duration::from_secs(duration_s.into()));
        RateLimiter {
            points,
            duration,
            windows: HashMap::new(),
            prune_at: PRUNE_AT,
        }
    }

    /// Charge a point to `key` at `now`: whether it is within the limit.
    pub(crate) fn consume(&mut self, key: &str, now: Instant) -> bool {
        if self.windows.len() >= self.prune_at {
            self.windows
                .retain(|_, (ends, _)| ends.is_none_or(|ends| now < ends));
            self.prune_at = PRUNE_AT.max(self.windows.len().saturating_mul(2));
        }
        let fresh = (self.duration.map(|duration| now + duration), 0);
        let window = self.windows.entry(key.to_owned()).or_insert(fresh);
        if window.0.is_some_and(|ends| now >= ends) {
            *window = fresh;
        }
        window.1 = window.1.saturating_add(1);
        window.1 <= u64::from(self.points)
    }
}

/// Whether the semaphore applies to `route`, and if so whether its levels follow the transaction
/// indexes (otherwise only the offset decides). Every served route that the reference gives a
/// semaphore reads the store.
fn route_options(route: RouteId) -> Option<bool> {
    match route {
        RouteId::BlocksShow | RouteId::BlocksTransactions | RouteId::TransactionsShow => {
            Some(false)
        }
        RouteId::TransactionsIndex | RouteId::WalletsTransactions => Some(true),
        _ => None,
    }
}

/// One level: its places, and its tickets (the places and the queue), which a request holds from
/// its arrival to its answer.
#[derive(Debug)]
struct Gate {
    places: Places,
    tickets: Places,
}

/// A place at a gate: a request's ticket and place.
pub(crate) type Permit<'g> = (SemaphorePermit<'g>, SemaphorePermit<'g>);

impl Gate {
    fn new(level: &SemaphoreLevel) -> Gate {
        let count = |n: u32| usize::try_from(n).unwrap_or(usize::MAX);
        let queue = count(level.concurrency).saturating_add(count(level.queue_limit));
        let (places, tickets) = (Places::new(count(level.concurrency)), Places::new(queue));
        Gate { places, tickets }
    }

    /// A place, at once or after the queue; `None` when the queue is full.
    async fn enter(&self) -> Option<Permit<'_>> {
        let ticket = self.tickets.try_acquire().ok()?;
        Some((ticket, self.places.acquire().await.ok()?))
    }
}

/// The semaphore's gates of the store's routes, at their two levels.
#[derive(Debug)]
pub(crate) struct Gates {
    options: Semaphore,
    database: [Gate; 2],
}

impl Gates {
    pub(crate) fn new(options: &Semaphore) -> Gates {
        let database = options.database.each_ref().map(Gate::new);
        Gates {
            options: *options,
            database,
        }
    }

    /// A place for a request of `route` with the validated `query`, when the semaphore applies to
    /// it; 429 when its level's queue is full.
    pub(crate) async fn enter(
        &self,
        route: RouteId,
        query: &Value,
    ) -> Result<Option<Permit<'_>>, ApiError> {
        let Some(indexed) = route_options(route).filter(|_| self.options.enabled) else {
            return Ok(None);
        };
        let [first, _] = &self.options.database;
        let max_offset = first.max_offset.unwrap_or(u64::MAX);
        let heavy = match indexed {
            true => heavy_listing(query, max_offset),
            false => over(query, max_offset),
        };
        let [light_gate, heavy_gate] = &self.database;
        let gate = if heavy { heavy_gate } else { light_gate };
        let permit = gate.enter().await;
        permit
            .map(Some)
            .ok_or_else(|| ApiError::too_many_requests("Too Many Requests"))
    }
}

/// The transaction index of `property`: whether it serves an ascending and a descending order,
/// whether a second order may follow it, and whether it is diverse (a value of it makes a request
/// light).
fn index(property: &str) -> Option<[bool; 4]> {
    Some(match property {
        "version" | "typeGroup" | "nonce" => [true, true, false, false],
        "timestamp" => [true; 4],
        "type" | "amount" | "fee" => [true, false, false, false],
        "id" | "blockId" | "senderPublicKey" | "recipientId" => [false, false, false, true],
        _ => return None,
    })
}

/// Whether a transaction listing asks for more than the first level: unless it gives one value of
/// a diverse index, when its first order is not served by an index in its direction (or a second
/// one follows an index that allows none), its page is beyond `max_offset`, or it gives a
/// criterion that no index serves.
fn heavy_listing(query: &Value, max_offset: u64) -> bool {
    const RESERVED: [&str; 5] = ["page", "limit", "transform", "offset", "orderBy"];
    let fields: Vec<(&String, &Value)> = query.as_object().into_iter().flatten().collect();
    let single = |value: &Value| value.is_number() || value.is_string();
    let diverse = |key: &str| index(key).is_some_and(|[.., diverse]| diverse);
    if fields
        .iter()
        .any(|(key, value)| single(value) && diverse(key))
    {
        return false;
    }
    let orders: Vec<(String, String)> = match query.get("orderBy") {
        Some(Value::Array(items)) => items.iter().filter_map(order).collect(),
        item => item.and_then(order).into_iter().collect(),
    };
    let served = |(property, direction): &(String, String)| match index(property) {
        Some([asc, desc, second, _]) => {
            let direction = (direction == "asc" && asc) || (direction == "desc" && desc);
            direction && (second || orders.len() == 1)
        }
        None => false,
    };
    let criteria = fields.iter().map(|(key, _)| key.as_str());
    let unindexed = |key: &str| !RESERVED.contains(&key) && index(key).is_none();
    let unserved = !orders.first().is_none_or(served);
    unserved || over(query, max_offset) || criteria.into_iter().any(unindexed)
}

/// An order as the query holds it: `property:direction`, or `{property, direction}`.
fn order(item: &Value) -> Option<(String, String)> {
    let text = |value: Option<&Value>| Some(value?.as_str()?.to_owned());
    match item {
        Value::String(order) => {
            let (property, direction) = order.split_once(':').unwrap_or((order, ""));
            Some((property.to_owned(), direction.to_owned()))
        }
        Value::Object(map) => Some((text(map.get("property"))?, text(map.get("direction"))?)),
        _ => None,
    }
}

/// Whether the offset, or the page times the limit, is beyond `max_offset`.
fn over(query: &Value, max_offset: u64) -> bool {
    let number = |key: &str| match query.get(key) {
        Some(Value::String(text)) => text.trim().parse().unwrap_or(0),
        value => value.and_then(Value::as_u64).unwrap_or(0),
    };
    number("offset") > max_offset || number("page").saturating_mul(number("limit")) > max_offset
}
