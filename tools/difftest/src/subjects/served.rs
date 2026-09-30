//! `p2p.controller`: a public route's controller of the server over a chain, a pool and a peer
//! repository, its answer encoded with the route's codec and its log lines, as the oracle runs the
//! reference's controller headless.
//!
//! The scripted node is `heartwood_p2p::testing::served`; this subject adds the log lines the
//! controller wrote (the warnings of the server's start excepted) and reads a 500 the way the
//! reference's server does: a controller that throws is `{thrown: true}`, except a block whose
//! transaction has a memo that is not valid UTF-8, which the reference lets through at the receipt
//! (`Documented::NonUtf8Memo`).

use std::sync::PoisonError;

use heartwood_crypto::Block;
use heartwood_p2p::testing::served::{Served, controller as serve};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::post_block_decode_request;
use serde_json::{Map, Value, json};
use tracing_subscriber::layer::SubscriberExt;

use super::forger_decide::Lines;
use super::{Env, Outcome, Run, block_refused, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("p2p.controller", controller)];

/// The oracle's answer for a controller that threw.
fn thrown() -> Value {
    json!({ "thrown": true })
}

fn controller(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let devnet = env.devnet()?;
    let milestones = env.milestones(case)?.into_owned();
    let height = u32::try_from(case.height).map_err(|_| failed("the height is out of range"))?;
    let lines = Lines::default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| failed(format!("no tokio runtime: {e}")))?;
    let served = runtime.block_on(async {
        let _capture =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(lines.clone()));
        serve(
            case.input,
            height,
            milestones.clone(),
            devnet.network.clone(),
        )
        .await
    });
    let Served {
        mut output,
        refusal,
        setup_lines,
    } = served.map_err(failed)?;
    if let Some(refusal) = &refusal {
        if refusal.status != 500 {
            return Ok(Verdict::Rejected {
                class: None,
                detail: format!("{} {}", refusal.status, refusal.message),
            });
        }
        // The block whose transaction the reference lets through at the receipt.
        if let Some(bytes) = posted_block(case.input)
            && let Err(error) = Block::decode(&bytes, &milestones)
            && let verdict @ Verdict::RefusedByDesign { .. } = block_refused(&error, &bytes)
        {
            return Ok(verdict);
        }
        output = thrown();
    }
    let lines = lines.0.lock().unwrap_or_else(PoisonError::into_inner);
    let written: Vec<Value> = (lines.iter().skip(setup_lines))
        .map(|(level, line)| json!([level, line]))
        .collect();
    Ok(Verdict::Output(in_oracle_order(
        case.input,
        output,
        Value::Array(written),
    )))
}

/// The output with the log lines where the oracle writes them: after the `thrown` flag, before the
/// answer, except for getStatus, whose answer comes first.
fn in_oracle_order(input: &Value, output: Value, lines: Value) -> Value {
    let Value::Object(fields) = output else {
        return output;
    };
    let status = input["route"].as_str() == Some(RouteId::GetStatus.id());
    let mut ordered = Map::new();
    let (first, rest): (Vec<_>, Vec<_>) = fields
        .into_iter()
        .partition(|(name, _)| name == "thrown" || status);
    ordered.extend(first);
    ordered.insert("lines".to_owned(), lines);
    ordered.extend(rest);
    Value::Object(ordered)
}

/// The bytes of the block a postBlock case posts.
fn posted_block(input: &Value) -> Option<Vec<u8>> {
    if input["route"].as_str()? != RouteId::PostBlock.id() {
        return None;
    }
    let payload = heartwood_crypto::utils::hex::decode(input["request"].as_str()?).ok()?;
    post_block_decode_request(&payload).ok()?.block
}
