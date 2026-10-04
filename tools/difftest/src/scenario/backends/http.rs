//! The API calls of the live backends, over the route corpus's HTTP client.

use std::time::Duration;

use heartwood_crypto::utils::hex;
use heartwood_crypto::{Milestones, Network, Transaction};
use serde_json::{Value, json};

use super::super::backend::{Answer, BackendError, Submitted};
use super::super::format::CompiledTx;
use crate::api::http::{Node, Reply, Request};

/// The chain's rules, which turn a compiled transaction into the JSON form the API takes.
#[derive(Debug, Default)]
pub struct Rules(Option<Milestones>);

impl Rules {
    /// The rules of the crypto files `crypto` holds (`network` and `milestones`).
    pub fn new(crypto: &Value) -> Result<Rules, BackendError> {
        let read = |name: &str| {
            crypto
                .get(name)
                .map(Value::to_string)
                .ok_or_else(|| failed("read the chain's rules", format!("no {name} file")))
        };
        let network =
            Network::load(&read("network")?).map_err(|e| failed("read the chain's rules", e))?;
        let milestones = Milestones::load(&read("milestones")?, &network)
            .map_err(|e| failed("read the chain's rules", e))?;
        Ok(Rules(Some(milestones)))
    }

    /// The API's form of the transaction `hex_text` writes, by the rules of the height it was
    /// planned for.
    fn json(&self, hex_text: &str, height: u32) -> Result<Value, BackendError> {
        let Some(milestones) = &self.0 else {
            return Err(failed("submit", "the runner gave no chain rules"));
        };
        let params = milestones.at(height);
        let bytes = hex::decode(hex_text).map_err(|e| failed("submit", e))?;
        let tx = Transaction::decode_trusted(&bytes, params).map_err(|e| failed("submit", e))?;
        Ok(tx.to_json(params))
    }
}

fn failed(action: &'static str, message: impl ToString) -> BackendError {
    BackendError::Failed {
        action,
        message: message.to_string(),
    }
}

/// The pauses before each new try of a request the API's rate limit refused (429): about a minute
/// in all, the reference limiter's window (100 requests a minute from one address by default).
const RATE_LIMITED: [Duration; 9] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(16),
    Duration::from_secs(16),
];

/// Send `request` to the API at `base` and wait for the answer; a request the rate limit refused
/// is sent again after a pause, until the limiter's window has passed.
fn send(base: &str, request: Request) -> Result<Answer, BackendError> {
    let node = Node::parse(base).map_err(|e| failed("call the API", e))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| failed("call the API", e))?;
    let mut pauses = RATE_LIMITED.iter();
    loop {
        let answer = match runtime.block_on(node.send(&request, None)) {
            Reply::Answer { status, body, .. } => Answer { status, body },
            Reply::Failed { error } => {
                return Err(failed("call the API", format!("{base}: {error}")));
            }
        };
        match pauses.next() {
            Some(pause) if answer.status == 429 => std::thread::sleep(*pause),
            _ => return Ok(answer),
        }
    }
}

/// `GET path` at `base`; `path` may carry a query.
pub fn get(base: &str, path: &str) -> Result<Answer, BackendError> {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    send(
        base,
        Request {
            method: "GET".into(),
            path: path.into(),
            query: query.into(),
            headers: Vec::new(),
            body: None,
        },
    )
}

/// `POST /api/transactions` of the JSON transactions `transactions` at `base`.
pub fn post_transactions(base: &str, transactions: &[Value]) -> Result<Answer, BackendError> {
    send(
        base,
        Request {
            method: "POST".into(),
            path: "/api/transactions".into(),
            query: String::new(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: Some(json!({ "transactions": transactions }).to_string()),
        },
    )
}

/// A node's tip from its API: `GET /api/blockchain`.
pub fn tip(base: &str) -> Result<super::super::backend::Tip, BackendError> {
    let answer = get(base, "/api/blockchain")?;
    let block = &answer.body["data"]["block"];
    match (block["height"].as_u64(), block["id"].as_str()) {
        (Some(height), Some(id)) if answer.status == 200 => Ok(super::super::backend::Tip {
            height: u32::try_from(height).map_err(|e| failed("read the tip", e))?,
            id: id.to_owned(),
        }),
        _ => Err(failed(
            "read the tip",
            format!("the API answers {}", answer.status),
        )),
    }
}

/// The end of the message of a vote, or a revoke, for a delegate that is not operating a node.
const OFFLINE: &str = "is not operating a node on the network";

/// The text before the reason in a refusal's message: the reference's pool wraps every refusal of
/// a handler as `<transaction> cannot be applied: <reason>`.
const APPLY: &str = "cannot be applied: ";

/// A refusal's code and its message without the transaction's own text. The reference's pool
/// reports every refusal of a handler as `ERR_APPLY`, the handler's own reason in the message
/// only: one whose reason is a delegate that is not operating a node is `ERR_OFFLINE`, the
/// handler's code, as the lockstep backend names it too.
pub fn refusal(error: &Value) -> (Option<String>, Option<String>) {
    let message = error["message"].as_str().map(|m| {
        m.split_once(APPLY)
            .map_or(m, |(_, reason)| reason)
            .to_owned()
    });
    let code = match error["type"].as_str() {
        Some("ERR_APPLY") if message.as_deref().is_some_and(|m| m.ends_with(OFFLINE)) => {
            Some("ERR_OFFLINE".to_owned())
        }
        other => other.map(str::to_owned),
    };
    (code, message)
}

/// What a `POST /api/transactions` answer says of the transaction `id`: whether it was admitted,
/// and the refusal's code and message ([`refusal`]); `None` when the answer does not name it.
pub fn verdict(body: &Value, id: &str) -> Option<(bool, Option<String>, Option<String>)> {
    let listed = |list: &str| {
        body["data"][list]
            .as_array()
            .is_some_and(|l| l.iter().any(|i| i == id))
    };
    let accepted = listed("accept") || listed("broadcast");
    let error = &body["errors"][id];
    let named = accepted || listed("invalid") || listed("excess") || !error.is_null();
    if !named {
        return None;
    }
    let error = error.get(0).unwrap_or(error);
    let (code, message) = refusal(error);
    Some(if accepted {
        (true, None, None)
    } else {
        (false, code, message)
    })
}

/// Submit `transactions` over `POST /api/transactions` at `base`: each one's answer, in order.
pub fn submit(
    base: &str,
    rules: &Rules,
    transactions: &[CompiledTx],
) -> Result<Vec<Submitted>, BackendError> {
    let wire = transactions
        .iter()
        .map(|t| rules.json(&t.bytes, t.height))
        .collect::<Result<Vec<_>, _>>()?;
    let answer = post_transactions(base, &wire)?;
    if !matches!(answer.status, 200 | 201 | 422) {
        return Err(failed(
            "submit",
            format!("the API answers {}", answer.status),
        ));
    }
    transactions
        .iter()
        .map(|t| {
            let (accepted, code, message) = verdict(&answer.body, &t.id).ok_or_else(|| {
                failed(
                    "submit",
                    format!("the answer does not name {} ({})", t.reference, t.id),
                )
            })?;
            Ok(Submitted {
                reference: t.reference.clone(),
                accepted,
                code,
                message,
            })
        })
        .collect()
}
