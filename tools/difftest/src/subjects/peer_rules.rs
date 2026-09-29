//! The peer rules, answered with `heartwood_p2p::utils` and `heartwood_p2p::rate_limiter`:
//!
//! - `peer.isValid` `{ip, allowPrivate, interfaces}` -> `{valid, processed}`;
//! - `peer.isValidVersion` `{version, minimumVersions?, configured, networkName}` -> `{valid}`,
//!   under the devnet's milestone at the case's height with its `p2p` replaced by
//!   `{minimumVersions}` (or removed when the input has none);
//! - `peer.sameSubnet` `{ip, peers}` -> `{same}`;
//! - `ratelimit.run` `{limiter, events: [{t, ip, route, op}]}` -> `{answers}`, the events at `t`
//!   milliseconds after a start; the server's inbound limiter answers a case whose
//!   `limiter.inbound` is true (the `server` subject).

use std::net::IpAddr;
use std::time::Duration;

use heartwood_crypto::managers::Milestones;
use heartwood_p2p::rate_limiter::RateLimiter;
use heartwood_p2p::utils::{is_valid_peer, is_valid_version, subnet};
use heartwood_p2p_wire::RouteId;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{Env, Outcome, Run, failed, height, text};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the peer rules.
pub(super) const OPS: &[(&str, Run)] = &[
    ("peer.isValid", is_valid),
    ("peer.isValidVersion", is_valid_version),
    ("peer.sameSubnet", same_subnet),
    ("ratelimit.run", rate_limit),
];

/// An address as the host's interfaces or the peer repository hold it.
fn address(value: &Value) -> Outcome<IpAddr> {
    let text = value
        .as_str()
        .ok_or_else(|| failed("an address is not text"))?;
    text.parse()
        .map_err(|_| SubjectError::Unsupported(format!("{text:?} is not an address Rust reads")))
}

fn addresses(value: &Value) -> Outcome<Vec<IpAddr>> {
    let list = value
        .as_array()
        .ok_or_else(|| failed("the addresses are not a list"))?;
    list.iter().map(address).collect()
}

fn flag(input: &Value, key: &str) -> Outcome<bool> {
    input[key]
        .as_bool()
        .ok_or_else(|| failed(format!("input.{key} is not a boolean")))
}

fn is_valid(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let mut ip = text(input, "ip")?.to_owned();
    let interfaces = addresses(&input["interfaces"])?;
    let valid = is_valid_peer::is_valid_peer(&mut ip, flag(input, "allowPrivate")?, &interfaces);
    Ok(Verdict::Output(json!({ "valid": valid, "processed": ip })))
}

fn is_valid_version(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let devnet = env.devnet()?;
    let mut milestone = devnet.milestones.at(height(case)?).as_json().clone();
    milestone.insert("height".into(), json!(1));
    match input.get("minimumVersions") {
        None => milestone.shift_remove("p2p"),
        Some(ranges) => milestone.insert("p2p".into(), json!({ "minimumVersions": ranges })),
    };
    let file = Value::Array(vec![Value::Object(milestone)]).to_string();
    let milestones = Milestones::load(&file, &devnet.network)
        .map_err(|e| failed(format!("the milestone does not load: {e}")))?;
    let configured: Vec<String> = (input["configured"].as_array().into_iter().flatten())
        .map(|range| range.as_str().map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or_else(|| failed("input.configured holds a value that is not text"))?;
    let valid = is_valid_version::is_valid_version(
        Some(text(input, "version")?),
        milestones.at(1),
        &configured,
        text(input, "networkName")?,
    );
    Ok(Verdict::Output(json!({ "valid": valid })))
}

fn same_subnet(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let ip = address(&case.input["ip"])?;
    let peers = case.input["peers"]
        .as_array()
        .ok_or_else(|| failed("input.peers is not a list"))?;
    let mut same = Vec::new();
    for peer in peers {
        if subnet::same_subnet(address(peer)?, ip) {
            same.push(peer.clone());
        }
    }
    Ok(Verdict::Output(json!({ "same": same })))
}

fn rate_limit(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    if input["limiter"]["inbound"].as_bool() == Some(true) {
        return super::server::inbound_rate_limit(env, case);
    }
    let points = |key: &str| {
        (input["limiter"][key].as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| failed(format!("input.limiter.{key} is not a count")))
    };
    let mut limiter =
        RateLimiter::outgoing(points("rateLimit")?, points("rateLimitPostTransactions")?);
    let start = Instant::now();
    let events = input["events"]
        .as_array()
        .ok_or_else(|| failed("input.events is not a list"))?;
    let mut answers = Vec::with_capacity(events.len());
    for event in events {
        let t = event["t"]
            .as_u64()
            .ok_or_else(|| failed("an event's t is not a count"))?;
        let now = start + Duration::from_millis(t);
        let ip = address(&event["ip"])?;
        let name = text(event, "route")?;
        let route = (RouteId::ALL.into_iter().find(|route| route.name() == name))
            .ok_or_else(|| failed(format!("no route named {name}")))?;
        answers.push(match text(event, "op")? {
            "consume" => json!(if limiter.consume(ip, route, now).is_ok() {
                "ok"
            } else {
                "exceeded"
            }),
            "wouldThrottle" => json!(limiter.has_exceeded(ip, route, now)),
            "throttle" if limiter.has_exceeded(ip, route, now) => json!("wait"),
            "throttle" => {
                let _ = limiter.consume(ip, route, now);
                json!("sent")
            }
            other => return Err(failed(format!("no limiter operation {other}"))),
        });
    }
    Ok(Verdict::Output(json!({ "answers": answers })))
}
