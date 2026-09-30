//! The API calls of the live backends, over the route corpus's HTTP client.

use serde_json::{Value, json};

use super::super::backend::{Answer, BackendError, Submitted};
use super::super::format::CompiledTx;
use crate::api::http::{Node, Reply, Request};

fn failed(action: &'static str, message: impl ToString) -> BackendError {
    BackendError::Failed {
        action,
        message: message.to_string(),
    }
}

/// Send `request` to the API at `base` and wait for the answer.
fn send(base: &str, request: Request) -> Result<Answer, BackendError> {
    let node = Node::parse(base).map_err(|e| failed("call the API", e))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| failed("call the API", e))?;
    match runtime.block_on(node.send(&request, None)) {
        Reply::Answer { status, body, .. } => Ok(Answer { status, body }),
        Reply::Failed { error } => Err(failed("call the API", format!("{base}: {error}"))),
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

/// `POST /api/transactions` of the hex transactions `hexes` at `base`.
pub fn post_transactions(base: &str, hexes: &[String]) -> Result<Answer, BackendError> {
    send(
        base,
        Request {
            method: "POST".into(),
            path: "/api/transactions".into(),
            query: String::new(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: Some(json!({ "transactions": hexes }).to_string()),
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

/// What a `POST /api/transactions` answer says of the transaction `id`: whether it was admitted
/// and the refusal's code.
pub fn verdict(body: &Value, id: &str) -> (bool, Option<String>) {
    let listed = |list: &str| {
        body["data"][list]
            .as_array()
            .is_some_and(|l| l.iter().any(|i| i == id))
    };
    let accepted = listed("accept") || listed("broadcast");
    let error = &body["errors"][id];
    let error = error.get(0).unwrap_or(error);
    let code = error["type"].as_str().map(str::to_owned);
    (accepted, if accepted { None } else { code })
}

/// Submit `transactions` over `POST /api/transactions` at `base`: each one's answer, in order.
pub fn submit(base: &str, transactions: &[CompiledTx]) -> Result<Vec<Submitted>, BackendError> {
    let hexes: Vec<String> = transactions.iter().map(|t| t.bytes.clone()).collect();
    let answer = post_transactions(base, &hexes)?;
    if !matches!(answer.status, 200 | 201 | 422) {
        return Err(failed(
            "submit",
            format!("the API answers {}", answer.status),
        ));
    }
    Ok(transactions
        .iter()
        .map(|t| {
            let (accepted, code) = verdict(&answer.body, &t.id);
            Submitted {
                reference: t.reference.clone(),
                accepted,
                code,
            }
        })
        .collect())
}
