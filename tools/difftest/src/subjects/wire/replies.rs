//! `wire.checkReply`: a reply of one of the six routes other than getStatus, decoded and checked
//! with its reply schema. The input is `{route, hex}`, or `{hex}` whose first byte selects the
//! route (`byte % 6` over `heartwood_p2p_wire::replies::ROUTES`) and whose rest is the reply, and
//! for getUnconfirmedTransactions an optional `maxTransactions` (the devnet milestones' largest
//! `block.maxTransactions` otherwise). The output is `{valid: true, value}` with the value after
//! the schema's changes, or `{valid: false, refused: part}`; a reply the codec refuses is the
//! codec's error class.
//!
//! The value is what the client reads: the addresses of getPeers, the common block, the blocks
//! with their generator keys lowercased, the postBlock status and height, the pool's reply
//! without the server's headers (the schema deletes them), and the accepted ids.

use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::{peer, transactions};
use heartwood_p2p_wire::replies::{self, ROUTES};
use heartwood_p2p_wire::{ReplyRefused, RouteId};
use serde_json::{Value, json};

use super::blocks::{hex_list_value, unknown, verdict};
use super::object;
use crate::subject::{Case, Verdict};
use crate::subjects::{Env, Outcome, failed, hex_bytes, rejected, text};

/// `wire.checkReply`.
pub(super) fn check_reply(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let Some(bytes) = hex_bytes(input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    let (route, bytes) = match input.get("route") {
        None | Some(Value::Null) => {
            let (first, rest) = bytes.split_first().map_or((0, &[][..]), |(b, r)| (*b, r));
            (ROUTES[usize::from(first) % ROUTES.len()], rest.to_vec())
        }
        Some(_) => {
            let name = text(input, "route")?;
            let route = (ROUTES.into_iter().find(|route| route.name() == name))
                .ok_or_else(|| failed(format!("input.route: {name} has no reply check")))?;
            (route, bytes)
        }
    };
    let checked = |result: Result<Value, ReplyRefused>| match result {
        Ok(value) => json!({ "valid": true, "value": value }),
        Err(part) => json!({ "valid": false, "refused": part_name(part) }),
    };
    match route {
        RouteId::GetPeers => verdict(peer::get_peers_decode_response(&bytes), |peers| {
            Ok(checked(replies::check_peers(peers).map(|addresses| {
                (addresses.iter())
                    .map(|address| json!({ "ip": address.ip, "port": address.port }))
                    .collect()
            })))
        }),
        RouteId::GetCommonBlocks => {
            verdict(peer::get_common_blocks_decode_response(&bytes), |reply| {
                Ok(checked(replies::check_common_blocks(reply).map(|common| {
                    let common = common.map(|c| json!({ "height": c.height, "id": c.id }));
                    json!({ "common": common })
                })))
            })
        }
        RouteId::GetBlocks => verdict(blocks::get_blocks_decode_response(&bytes), |listed| {
            let mut listed = listed.clone();
            Ok(match replies::check_blocks(&mut listed) {
                Ok(()) => checked(Ok(listed
                    .iter()
                    .map(listed_value)
                    .collect::<Outcome<_>>()?)),
                Err(part) => checked(Err(part)),
            })
        }),
        RouteId::PostBlock => verdict(blocks::post_block_decode_response(&bytes), |reply| {
            Ok(checked(replies::check_post_block(reply).map(
                |height| json!({ "status": reply.status.unwrap_or(false), "height": height }),
            )))
        }),
        RouteId::GetUnconfirmedTransactions => {
            let max = match input.get("maxTransactions") {
                Some(max) => max.as_u64().and_then(|n| usize::try_from(n).ok()),
                None => largest_max_transactions(env)?,
            }
            .ok_or_else(|| failed("input.maxTransactions is not a count"))?;
            let decoded = transactions::get_unconfirmed_transactions_decode_response(&bytes);
            verdict(decoded, |reply| {
                let message = &reply.message;
                let value = object(message, |key| match key {
                    "poolSize" => Ok(json!(message.pool_size)),
                    "transactions" => Ok(hex_list_value(&reply.transactions)),
                    "headers" => Ok(Value::Null),
                    other => Err(unknown(other)),
                })?;
                let value = match value {
                    Value::Object(mut fields) => {
                        fields.shift_remove("headers");
                        Value::Object(fields)
                    }
                    other => other,
                };
                Ok(checked(
                    replies::check_unconfirmed(reply, max).map(|()| value),
                ))
            })
        }
        RouteId::PostTransactions => {
            let decoded = transactions::post_transactions_decode_response(&bytes);
            verdict(decoded, |accept| {
                Ok(checked(
                    replies::check_post_transactions(accept).map(|()| json!(accept)),
                ))
            })
        }
        other => Err(failed(format!("{other:?} has no reply check"))),
    }
}

/// The largest `block.maxTransactions` of the devnet milestones, as the reference's schema module
/// computes it; `None` when a milestone has none.
fn largest_max_transactions(env: &Env<'_>) -> Outcome<Option<usize>> {
    let all = env.devnet()?.milestones.all();
    let each: Option<Vec<u32>> = all.iter().map(|params| params.max_transactions()).collect();
    Ok(each
        .and_then(|each| each.into_iter().max())
        .and_then(|n| usize::try_from(n).ok()))
}

/// A checked block: `{...header, totalAmount, totalFee, reward, transactions}`, own keys in the
/// reference's order.
fn listed_value(block: &ListedBlock) -> Outcome<Value> {
    let header = &block.header;
    object(header, |key| match key {
        "id" => Ok(json!(header.id)),
        "version" => Ok(json!(header.version)),
        "timestamp" => Ok(json!(header.timestamp)),
        "previousBlock" => Ok(json!(header.previous_block)),
        "height" => Ok(json!(header.height)),
        "numberOfTransactions" => Ok(json!(header.number_of_transactions)),
        "totalAmount" => Ok(json!(block.total_amount.to_string())),
        "totalFee" => Ok(json!(block.total_fee.to_string())),
        "reward" => Ok(json!(block.reward.to_string())),
        "payloadLength" => Ok(json!(header.payload_length)),
        "payloadHash" => Ok(json!(header.payload_hash)),
        "generatorPublicKey" => Ok(json!(header.generator_public_key)),
        "blockSignature" => Ok(json!(header.block_signature)),
        "transactions" => Ok(hex_list_value(&block.transactions)),
        other => Err(unknown(other)),
    })
}

/// The name of the reply schema's part that the oracle reports.
pub(super) fn part_name(part: ReplyRefused) -> &'static str {
    match part {
        ReplyRefused::Root => "root",
        ReplyRefused::State => "state",
        ReplyRefused::Header => "header",
        ReplyRefused::Config => "config",
        ReplyRefused::Network => "network",
        ReplyRefused::Plugins => "plugins",
        ReplyRefused::PublicKeys => "publicKeys",
        ReplyRefused::Signatures => "signatures",
        ReplyRefused::Peer => "peer",
        ReplyRefused::Common => "common",
        ReplyRefused::PostBlock => "postBlock",
        ReplyRefused::Unconfirmed => "unconfirmed",
    }
}
