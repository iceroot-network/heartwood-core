//! The named projections of the public API that a step's `api` check compares between nodes: each
//! is a route's JSON with the fields that may differ between two honest nodes removed. The names
//! and what they remove are the API compatibility design's list; `wallets`, the scenario's own
//! wallets (genesis and every actor, balance and nonce), is the one two runs of a suite compare
//! with each other ([`super::record::compare`]).
//!
//! | Projection | Route | Removed |
//! |---|---|---|
//! | `wallets` | `/wallets/{id}` of genesis and each actor | all but `balance` and `nonce` |
//! | `wallet(name)` | `/wallets/{id}` | `attributes.delegate.version`, `.productivity`, `.missedBlocks` |
//! | `wallet_history(name)` | `/wallets/{id}/transactions`, all pages | `confirmations` |
//! | `tx(ref)` | `/transactions/{id}` | `confirmations` |
//! | `block(h)` | `/blocks/{h}` | `confirmations` |
//! | `block_txs(h)` | `/blocks/{h}/transactions`, all pages | nothing |
//! | `delegates` | `/delegates`, all pages | `version`, `blocks.productivity`, `blocks.missed` |
//! | `voters(name)` | `/delegates/{name}/voters`, all pages | nothing |
//! | `round(r)` | `/rounds/{r}/delegates` | nothing; sorted by votes (descending), then public key |
//! | `missed(from, to)` | `/blocks/missed` over those heights, all pages | nothing |
//! | `pool` | `/transactions/unconfirmed`, all pages | all but the ids, as a sorted set |
//! | `config` | `/node/configuration` | `core.version` |
//! | `crypto` | `/node/configuration/crypto` | nothing |
//! | `fees` | `/transactions/types` and `/transactions/fees` | nothing |
//! | `supply` | `/blockchain` | nothing |
//! | `status` | `/node/syncing` | `blocks` |
//!
//! Productivity and missed counts are removed at every height (the design removes them unless the
//! height is quiescent, which a running network's never is for sure). A name is a wallet of the
//! scenario (an actor, `genesis`, a delegate's username or a key's role) or an address; a `ref` is
//! a transaction of the scenario or an id.

use std::cmp::Ordering;

use serde_json::{Map, Value, json};

use super::backend::{Backend, BackendError};
use super::chain::Chain;
use super::format::CompiledTx;

/// The most pages a projection reads of one list.
const MAX_PAGES: u32 = 1000;

/// The rows a page of a list holds.
const PAGE: u32 = 100;

/// What the names of a projection resolve against.
pub struct Names<'a> {
    /// The scenario's chain and wallets.
    pub chain: &'a Chain,
    /// The scenario's actors, in name order.
    pub actors: Vec<&'a str>,
    /// The scenario's compiled transactions.
    pub transactions: &'a [CompiledTx],
}

fn failed(message: String) -> BackendError {
    BackendError::Failed {
        action: "project the API",
        message,
    }
}

/// `text` split into its name and arguments: `delegates`, `wallet(alice)`, `missed(2, 9)`.
pub fn parse(text: &str) -> Result<(&str, Vec<&str>), BackendError> {
    let text = text.trim();
    let Some((name, rest)) = text.split_once('(') else {
        return Ok((text, Vec::new()));
    };
    let inner = rest
        .strip_suffix(')')
        .ok_or_else(|| failed(format!("the projection {text} does not end its arguments")))?;
    let args: Vec<&str> = inner.split(',').map(str::trim).collect();
    if args.iter().any(|a| a.is_empty()) {
        return Err(failed(format!("the projection {text} has an empty argument")));
    }
    Ok((name.trim(), args))
}

/// Remove the field at `path` (keys separated by dots) from `value`, when it is there.
fn remove(value: &mut Value, path: &str) {
    let mut at = value;
    let mut keys = path.split('.').peekable();
    while let Some(key) = keys.next() {
        if keys.peek().is_none() {
            if let Some(object) = at.as_object_mut() {
                object.remove(key);
            }
            return;
        }
        match at.get_mut(key) {
            Some(next) => at = next,
            None => return,
        }
    }
}

/// A route's answer as a projection's value: its body, or its status and body when it is not 200.
fn answer(backend: &dyn Backend, node: u32, path: &str) -> Result<Value, BackendError> {
    let answer = backend.get(node, path)?;
    if answer.status >= 500 {
        return Err(failed(format!("{path} answers {}", answer.status)));
    }
    Ok(if answer.status == 200 {
        answer.body
    } else {
        json!({"status": answer.status, "body": answer.body})
    })
}

/// Every row of the list at `path` (which may carry a query), page after page.
fn all_pages(backend: &dyn Backend, node: u32, path: &str) -> Result<Vec<Value>, BackendError> {
    let join = if path.contains('?') { '&' } else { '?' };
    let mut rows = Vec::new();
    for page in 1..=MAX_PAGES {
        let url = format!("{path}{join}page={page}&limit={PAGE}");
        let answer = backend.get(node, &url)?;
        if answer.status != 200 {
            return Err(failed(format!("{url} answers {}", answer.status)));
        }
        let Some(list) = answer.body["data"].as_array() else {
            return Err(failed(format!("{url} answers no list")));
        };
        rows.extend(list.iter().cloned());
        let more = answer.body["meta"]["next"].is_string();
        if list.is_empty() || !more {
            return Ok(rows);
        }
    }
    Err(failed(format!("{path} has more than {MAX_PAGES} pages")))
}

fn without(mut rows: Vec<Value>, paths: &[&str]) -> Value {
    for row in &mut rows {
        for path in paths {
            remove(row, path);
        }
    }
    Value::Array(rows)
}

impl Names<'_> {
    /// The address of the wallet `name` names, or `name` itself.
    fn address(&self, name: &str) -> String {
        self.chain
            .person(name)
            .map_or_else(|_| name.to_owned(), |p| p.address.clone())
    }

    /// The id of the transaction `reference` names, or `reference` itself.
    fn id(&self, reference: &str) -> String {
        self.transactions
            .iter()
            .find(|t| t.reference == reference)
            .map_or_else(|| reference.to_owned(), |t| t.id.clone())
    }
}

/// A decimal amount's order: shorter is smaller, then by its digits.
fn amount_order(a: &Value, b: &Value) -> Ordering {
    let text = |v: &Value| {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
    };
    let (a, b) = (text(a), text(b));
    a.len().cmp(&b.len()).then(a.cmp(&b))
}

/// The projection `text` of node `node`'s API.
pub fn read(
    backend: &dyn Backend,
    names: &Names<'_>,
    node: u32,
    text: &str,
) -> Result<Value, BackendError> {
    let (name, args) = parse(text)?;
    let one = || match args.as_slice() {
        [arg] => Ok(*arg),
        _ => Err(failed(format!("the projection {name} takes one argument"))),
    };
    if !matches!(
        name,
        "wallet" | "wallet_history" | "tx" | "block" | "block_txs" | "voters" | "round" | "missed"
    ) && !args.is_empty()
    {
        return Err(failed(format!("the projection {name} takes no argument")));
    }
    let get = |path: &str| answer(backend, node, path);
    match name {
        "wallets" => {
            let mut out = Map::new();
            for actor in std::iter::once("genesis").chain(names.actors.iter().copied()) {
                let body = get(&format!("/api/wallets/{}", names.address(actor)))?;
                let data = &body["data"];
                out.insert(
                    actor.to_owned(),
                    json!({"balance": data["balance"], "nonce": data["nonce"]}),
                );
            }
            Ok(Value::Object(out))
        }
        "wallet" => {
            let mut body = get(&format!("/api/wallets/{}", names.address(one()?)))?;
            for field in ["version", "productivity", "missedBlocks"] {
                remove(&mut body, &format!("data.attributes.delegate.{field}"));
            }
            Ok(body)
        }
        "wallet_history" => {
            let path = format!("/api/wallets/{}/transactions", names.address(one()?));
            Ok(without(all_pages(backend, node, &path)?, &["confirmations"]))
        }
        "tx" => {
            let mut body = get(&format!("/api/transactions/{}", names.id(one()?)))?;
            remove(&mut body, "data.confirmations");
            Ok(body)
        }
        "block" => {
            let mut body = get(&format!("/api/blocks/{}", one()?))?;
            remove(&mut body, "data.confirmations");
            Ok(body)
        }
        "block_txs" => {
            let path = format!("/api/blocks/{}/transactions", one()?);
            Ok(Value::Array(all_pages(backend, node, &path)?))
        }
        "delegates" => {
            let rows = all_pages(backend, node, "/api/delegates")?;
            Ok(without(
                rows,
                &["version", "blocks.productivity", "blocks.missed"],
            ))
        }
        "voters" => {
            let path = format!("/api/delegates/{}/voters", one()?);
            Ok(Value::Array(all_pages(backend, node, &path)?))
        }
        "round" => {
            let mut body = get(&format!("/api/rounds/{}/delegates", one()?))?;
            if let Some(rows) = body["data"].as_array_mut() {
                rows.sort_by(|a, b| {
                    amount_order(&b["votes"], &a["votes"]).then_with(|| {
                        a["publicKey"]
                            .as_str()
                            .unwrap_or_default()
                            .cmp(b["publicKey"].as_str().unwrap_or_default())
                    })
                });
            }
            Ok(body)
        }
        "missed" => {
            let [from, to] = args.as_slice() else {
                return Err(failed("the projection missed takes two heights".into()));
            };
            let path = format!("/api/blocks/missed?height.from={from}&height.to={to}");
            Ok(Value::Array(all_pages(backend, node, &path)?))
        }
        "pool" => {
            let rows = all_pages(backend, node, "/api/transactions/unconfirmed")?;
            let mut ids: Vec<String> = rows
                .iter()
                .filter_map(|t| t["id"].as_str().map(str::to_owned))
                .collect();
            ids.sort();
            Ok(json!(ids))
        }
        "config" => {
            let mut body = get("/api/node/configuration")?;
            remove(&mut body, "data.core.version");
            Ok(body)
        }
        "crypto" => get("/api/node/configuration/crypto"),
        "fees" => Ok(json!({
            "types": get("/api/transactions/types")?,
            "fees": get("/api/transactions/fees")?,
        })),
        "supply" => get("/api/blockchain"),
        "status" => {
            let mut body = get("/api/node/syncing")?;
            remove(&mut body, "data.blocks");
            Ok(body)
        }
        other => Err(failed(format!(
            "no projection is named {other}: wallets, wallet, wallet_history, tx, block, \
             block_txs, delegates, voters, round, missed, pool, config, crypto, fees, supply or \
             status"
        ))),
    }
}
