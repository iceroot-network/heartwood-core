//! The node routes: `GET /node/status` (`synced`, `now`, `blocksCount`, `timestamp`),
//! `GET /node/syncing` (`syncing`, `blocks`, `height`, `id`), `GET /node/configuration` (the
//! network's values, the tip's milestone as `constants`, the pool's options, `ports: {}` and the
//! node's version) and `GET /node/configuration/crypto` (the loaded crypto files, the milestones
//! merged cumulatively, `exceptions: {}`).

use std::sync::Arc;

use heartwood_crypto::crypto::slots;
use heartwood_crypto::managers::Params;
use heartwood_crypto::utils::js;
use heartwood_kernel::{Clock, SystemClock};
use heartwood_pool::PoolViewOptions;
use heartwood_state::StateView;
use serde_json::{Map, Value, json};

use crate::context::{ApiContext, ApiReply, ApiRequest, NodeInfo, NodeStatus, Route, RouteId};
use crate::controllers::transactions::read;
use crate::errors::ApiError;
use crate::resources::fee_statistics;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::NodeStatus,
            handler: |ctx, request| Box::pin(status(ctx, request)),
        },
        Route {
            id: RouteId::NodeSyncing,
            handler: |ctx, request| Box::pin(syncing(ctx, request)),
        },
        Route {
            id: RouteId::NodeConfiguration,
            handler: |ctx, request| Box::pin(configuration(ctx, request)),
        },
        Route {
            id: RouteId::NodeConfigurationCrypto,
            handler: |ctx, request| Box::pin(configuration_crypto(ctx, request)),
        },
    ]
}

/// The node's status.
pub async fn status(ctx: Arc<ApiContext>, _request: ApiRequest) -> Result<ApiReply, ApiError> {
    let data = status_data(&ctx.state(), ctx.status(), &ctx.node, SystemClock.now_ms());
    Ok(ApiReply::data(data))
}

/// The node's sync.
pub async fn syncing(ctx: Arc<ApiContext>, _request: ApiRequest) -> Result<ApiReply, ApiError> {
    Ok(ApiReply::data(syncing_data(&ctx.state(), ctx.status())))
}

/// The node's configuration.
pub async fn configuration(
    ctx: Arc<ApiContext>,
    _request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    Ok(ApiReply::data(configuration_data(
        &ctx.state(),
        &ctx.pool().options,
        &ctx.node,
    )))
}

/// The crypto configuration.
pub async fn configuration_crypto(
    ctx: Arc<ApiContext>,
    _request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    Ok(ApiReply::data(crypto_data(&ctx.node)))
}

/// The tip's height less the network's: 0 when the network's height is not known.
fn behind(state: &StateView, status: NodeStatus) -> i64 {
    match status.network_height {
        0 => 0,
        network => i64::from(network) - i64::from(state.height()),
    }
}

/// `/node/status` at `now_ms` (the time, in seconds since the network's epoch, is volatile).
pub fn status_data(state: &StateView, status: NodeStatus, node: &NodeInfo, now_ms: i64) -> Value {
    let epoch = node.crypto.milestones.at(1).epoch();
    let time = slots::epoch_unix_ms(epoch).map(|epoch_ms| slots::epoch_time(now_ms, epoch_ms));
    json!({
        "synced": status.synced, "now": state.height(),
        "blocksCount": behind(state, status), "timestamp": time,
    })
}

/// `/node/syncing`.
pub fn syncing_data(state: &StateView, status: NodeStatus) -> Value {
    let id = state.last_block.id().to_hex();
    json!({
        "syncing": !status.synced, "blocks": behind(state, status), "height": state.height(),
        "id": id,
    })
}

/// `/node/configuration`: the network's values, the milestone at the tip as `constants` (without
/// its `dynamicFees`), and the pool's options, whose dynamic fees are the milestone's when the
/// milestone turns them on. Every `false` is left out at any depth, as the reference's
/// `removeFalsy` does.
pub fn configuration_data(state: &StateView, pool: &PoolViewOptions, node: &NodeInfo) -> Value {
    let network = &node.crypto.network;
    let params: &Params = node.crypto.milestones.at(state.height());
    let mut constants = params.as_json().clone();
    let milestone_fees = constants.shift_remove("dynamicFees");
    let dynamic_fees = match milestone_fees {
        Some(Value::Object(mut fees)) if fees.get("enabled").is_some_and(js::is_truthy) => {
            if let Some(min_fee) = fees.shift_remove("minFee") {
                // The reference spreads the fees, sets both floors to minFee, then deletes it.
                fees.insert("minFeeBroadcast".into(), min_fee.clone());
                fees.insert("minFeePool".into(), min_fee);
            }
            Value::Object(fees)
        }
        _ if pool.dynamic_fees.enabled => {
            let fees = &pool.dynamic_fees;
            let addon = fees.addon_bytes.iter();
            let addon = addon.map(|(key, bytes)| (key.clone(), json!(bytes)));
            json!({
                "enabled": true, "minFeePool": fees.min_fee_pool,
                "minFeeBroadcast": fees.min_fee_broadcast,
                "addonBytes": addon.collect::<Map<_, _>>(),
            })
        }
        _ => json!({ "enabled": false }),
    };
    let mut data = json!({
        "core": { "version": node.version }, "nethash": network.nethash, "slip44": network.slip44,
        "wif": network.wif, "token": network.token, "symbol": network.symbol,
        "explorer": network.explorer, "version": network.pub_key_hash, "ports": {},
        "constants": constants,
        "pool": {
            "dynamicFees": dynamic_fees, "maxTransactionsInPool": pool.max_transactions_in_pool,
            "maxTransactionsPerSender": pool.max_transactions_per_sender,
            "maxTransactionsPerRequest": pool.max_transactions_per_request,
            "maxTransactionAge": pool.max_transaction_age,
            "maxTransactionBytes": pool.max_transaction_bytes,
        },
    });
    remove_false(&mut data);
    data
}

/// Remove every `false` member of the objects in `value`; an array's `false` becomes a hole, which
/// JSON writes as `null`.
fn remove_false(value: &mut Value) {
    if let Value::Object(map) = value {
        map.retain(|_, member| *member != false);
    }
    let members: Vec<&mut Value> = match value {
        Value::Object(map) => map.values_mut().collect(),
        Value::Array(items) => items.iter_mut().collect(),
        _ => Vec::new(),
    };
    for member in members {
        match *member == false {
            true => *member = Value::Null,
            false => remove_false(member),
        }
    }
}

/// `/node/configuration/crypto`: what the reference's crypto configuration holds, the network as
/// loaded, `exceptions` empty, the milestones merged cumulatively in height order and the genesis
/// block as the reference holds it once it has booted ([`genesis_as_held`]).
pub fn crypto_data(node: &NodeInfo) -> Value {
    let milestones = node.crypto.milestones.all().iter();
    let milestones = milestones.map(|params| Value::Object(params.as_json().clone()));
    json!({
        "network": node.crypto.network.to_json(), "exceptions": {},
        "milestones": milestones.collect::<Vec<_>>(),
        "genesisBlock": genesis_as_held(&node.crypto.genesis),
    })
}

/// The genesis block `genesis` as loaded, as the reference's configuration holds it once it has
/// booted: building the block turns each transaction's `amount`, `fee` and `nonce`, and the
/// amounts of its transfers, into the decimal strings of big numbers, and serialises the
/// transaction, which sets on its own object, in this order, `version` to 3 and `headerType` to 0
/// when they are not truthy, and `typeGroup` to 1 when it is missing. A key it adds comes last; a
/// key it sets keeps its place. A key that the transaction's schema does not list is kept, as the
/// reference keeps it.
pub fn genesis_as_held(genesis: &Value) -> Value {
    let mut genesis = genesis.clone();
    let transactions = genesis
        .get_mut("transactions")
        .and_then(Value::as_array_mut);
    for transaction in transactions.into_iter().flatten() {
        let Some(transaction) = transaction.as_object_mut() else {
            continue;
        };
        for key in ["amount", "fee", "nonce"] {
            as_text(transaction.get_mut(key));
        }
        let transfers = transaction
            .get_mut("asset")
            .and_then(|asset| asset.get_mut("transfers"));
        for transfer in transfers
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            as_text(transfer.get_mut("amount"));
        }
        for (key, default) in [("version", 3), ("headerType", 0)] {
            if !transaction.get(key).is_some_and(js::is_truthy) {
                transaction.insert(key.to_owned(), Value::from(default));
            }
        }
        if !transaction.contains_key("typeGroup") {
            transaction.insert("typeGroup".to_owned(), Value::from(1));
        }
    }
    genesis
}

/// A whole number that is not negative, written as a JSON number, becomes its decimal string, as
/// a big number is served.
fn as_text(value: Option<&mut Value>) {
    if let Some(value) = value
        && let Some(number) = value.as_u64()
    {
        *value = Value::from(number.to_string());
    }
}

/// `GET /node/fees`: the fee statistics of the kept types, over the last `days` days when the
/// query gives them and over each type's last 20 transactions when it does not.
pub async fn fees(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    fees_at(&ctx, &request, SystemClock.now_ms()).await
}

/// [`fees`] at `now_ms` (Unix milliseconds), the time the window of `days` counts back from; a
/// test gives its own clock here.
pub async fn fees_at(
    ctx: &Arc<ApiContext>,
    request: &ApiRequest,
    now_ms: i64,
) -> Result<ApiReply, ApiError> {
    let days = request.query.get("days").and_then(Value::as_i64);
    let window = match days {
        Some(days) => {
            let epoch = slots::epoch_unix_ms(ctx.node.crypto.milestones.at(1).epoch())
                .ok_or_else(ApiError::internal)?;
            Some(fee_statistics::window(days, now_ms, epoch).ok_or_else(ApiError::internal)?)
        }
        None => None,
    };
    let rows = read(ctx, move |reader| reader.fee_statistics(window)).await?;
    let rows = match days {
        Some(_) => rows,
        None => fee_statistics::in_registry_order(rows),
    };
    Ok(ApiReply::ok(fee_statistics::response(days, &rows)))
}
