//! Vector class V25 (the peer rules), compared with the functions of this module.
//!
//! The class is read from `tests/vectors` of the crate, or from the directory in `P2P_VECTORS`
//! (a relative path is taken from the workspace root), such as a local oracle run in
//! `target/vectors/local`. The crate's directory must hold the class, where `make vectors-install`
//! puts it, so a missing class fails the test; a directory named in the environment may lack it,
//! and the test then says so and passes.

use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use heartwood_crypto::managers::{Milestones, Network};
use heartwood_p2p_wire::RouteId;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{is_valid_peer, is_valid_version, subnet};
use crate::rate_limiter::RateLimiter;

const CLASS: &str = "V25-peer-rules";

/// The class's records without its meta record, or `None` when the directory `P2P_VECTORS` names
/// lacks it. The crate's own directory must hold it.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("P2P_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        // A directory named in the environment may lack the class; the crate's own must hold it.
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    Some(lines.collect())
}

fn address(value: &Value) -> IpAddr {
    value.as_str().unwrap().parse().unwrap()
}

/// A network and a first milestone with `p2p` when given, the keys the loader needs and no more.
fn milestones(p2p: Option<&Value>) -> Milestones {
    let network = Network::load(
        &json!({
            "name": "devnet", "messagePrefix": "devnet message:\n", "pubKeyHash": 90,
            "bip32": { "public": 70_617_039, "private": 70_615_956 }, "nethash": "00".repeat(32),
            "wif": 252, "slip44": 1, "client": { "token": "dROOT", "symbol": "dRT", "explorer": "" },
        })
        .to_string(),
    )
    .unwrap();
    let mut milestone = json!({
        "height": 1, "activeDelegates": 53, "blockTime": 8, "epoch": "2026-01-01T00:00:00.000Z",
        "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 }, "reward": 0,
        "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 }, "donations": {},
        "dynamicReward": { "enabled": false }, "transfer": { "maximum": 256, "minimum": 1 },
    });
    if let Some(ranges) = p2p {
        milestone["p2p"] = json!({ "minimumVersions": ranges });
    }
    Milestones::load(&json!([milestone]).to_string(), &network).unwrap()
}

fn check(record: &Value) {
    let (input, output, name) = (&record["input"], &record["output"], &record["name"]);
    let actual = match record["op"].as_str().unwrap() {
        "peer.isValid" => {
            let mut ip = input["ip"].as_str().unwrap().to_owned();
            let interfaces: Vec<IpAddr> = input["interfaces"]
                .as_array()
                .unwrap()
                .iter()
                .map(address)
                .collect();
            let allow = input["allowPrivate"].as_bool().unwrap();
            let valid = is_valid_peer::is_valid_peer(&mut ip, allow, &interfaces);
            json!({ "valid": valid, "processed": ip })
        }
        "peer.isValidVersion" => {
            let milestones = milestones(input.get("minimumVersions"));
            let configured: Vec<String> = (input["configured"].as_array().unwrap().iter())
                .map(|range| range.as_str().unwrap().to_owned())
                .collect();
            let valid = is_valid_version::is_valid_version(
                input["version"].as_str(),
                milestones.at(1),
                &configured,
                input["networkName"].as_str().unwrap(),
            );
            json!({ "valid": valid })
        }
        "peer.sameSubnet" => {
            let ip = address(&input["ip"]);
            let peers = input["peers"].as_array().unwrap();
            let same: Vec<&Value> = peers
                .iter()
                .filter(|peer| subnet::same_subnet(address(peer), ip))
                .collect();
            json!({ "same": same })
        }
        "ratelimit.run" => {
            let limit = |key: &str| u32::try_from(input["limiter"][key].as_u64().unwrap()).unwrap();
            let mut limiter =
                RateLimiter::outgoing(limit("rateLimit"), limit("rateLimitPostTransactions"));
            let start = Instant::now();
            let answers: Vec<Value> = (input["events"].as_array().unwrap().iter())
                .map(|event| {
                    let now = start + Duration::from_millis(event["t"].as_u64().unwrap());
                    let ip = address(&event["ip"]);
                    let route = RouteId::ALL
                        .into_iter()
                        .find(|r| r.name() == event["route"])
                        .unwrap();
                    match event["op"].as_str().unwrap() {
                        "consume" => json!(if limiter.consume(ip, route, now).is_ok() {
                            "ok"
                        } else {
                            "exceeded"
                        }),
                        "wouldThrottle" => json!(limiter.has_exceeded(ip, route, now)),
                        _ if limiter.has_exceeded(ip, route, now) => json!("wait"),
                        _ => {
                            let _ = limiter.consume(ip, route, now);
                            json!("sent")
                        }
                    }
                })
                .collect();
            json!({ "answers": answers })
        }
        other => panic!("{name}: an operation outside the class: {other}"),
    };
    assert_eq!(&actual, output, "{name}");
}

#[test]
fn the_peer_rules_match_the_reference() {
    let Some(records) = records() else {
        eprintln!("{CLASS} is not in the directory P2P_VECTORS names");
        return;
    };
    assert!(!records.is_empty());
    for record in &records {
        check(record);
    }
}
