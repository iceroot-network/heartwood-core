//! The response shapes of the served routes, as the reference's resources write them, keys in the
//! reference's order. A key whose value the reference leaves `undefined` is left out, as
//! `JSON.stringify` leaves it out; amounts are decimal strings.
//!
//! The listings' resources are in their own modules: [`delegate`], [`wallet_search`],
//! [`missed_block`], [`round`] and [`fee_statistics`].

pub mod delegate;
pub mod fee_statistics;
pub mod missed_block;
pub mod round;
pub mod wallet_search;

use heartwood_crypto::transactions::types::Asset;
use heartwood_crypto::utils::burn::burned_fee;
use heartwood_crypto::{BlockId, Milestones, Transaction};
use heartwood_database::BlockRow;
use heartwood_kernel::timestamp::format_timestamp;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_pool::PoolView;
use heartwood_state::{Resignation, StateView, Wallet};
use serde_json::{Value, json};

use crate::context::{Hundredths, ProductivityTable};
use crate::search::{big_number, plain};

/// Where a confirmed transaction is, as its resource shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionBlock {
    /// The block's id.
    pub id: BlockId,
    /// The block's height.
    pub height: u32,
    /// The block's timestamp; `None` leaves `timestamp` out (a block's transactions route).
    pub timestamp: Option<u32>,
    /// The confirmations shown.
    pub confirmations: u32,
}

/// `value` without its top-level members that are `null`: the resources write a JavaScript
/// `undefined` as `null` here, and `JSON.stringify` leaves such a key out.
pub(crate) fn defined(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.retain(|_, member| !member.is_null());
    }
    value
}

/// A block, from its stored row, with `transferred` (the sum of its transfer items, which the
/// reference adds to `forged.amount`) and the tip's height for its confirmations. The generator's
/// name is the row's: the reference finds the same one in its wallets for every stored block (the
/// genesis block has none).
pub fn block(row: &BlockRow, transferred: u128, tip: u32, milestones: &Milestones) -> Value {
    let donations: Value = serde_json::from_str(&row.donations).unwrap_or_else(|_| json!({}));
    let amount = |value: &Value| {
        value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned)
    };
    let amounts = donations.as_object().into_iter().flatten();
    let donated: i128 = amounts
        .map(|(_, value)| amount(value).parse().unwrap_or(0))
        .sum();
    let total = i128::from(row.reward) - donated + i128::from(row.total_fee);
    let total = total - i128::from(row.burned_fee);
    let amount = u128::from(row.total_amount) + transferred;
    defined(json!({
        "id": row.id.to_hex(), "version": row.version, "height": row.height,
        "previous": row.previous_block.as_ref().map(BlockId::to_hex),
        "forged": {
            "reward": row.reward.to_string(), "donations": donations,
            "fee": row.total_fee.to_string(), "burnedFee": row.burned_fee.to_string(),
            "amount": amount.to_string(), "total": total.to_string(),
        },
        "payload": { "hash": row.payload_hash, "length": row.payload_length },
        "generator": defined(json!({
            "username": row.username, "publicKey": row.generator_public_key,
        })),
        "signature": row.block_signature,
        "confirmations": i64::from(tip) - i64::from(row.height),
        "transactions": row.number_of_transactions,
        "timestamp": timestamp(row.timestamp, milestones),
    }))
}

/// A transaction's data as the reference holds it after reading its bytes, which its routes show
/// untransformed (`transform=false`): the transaction's JSON with its `typeGroup` kept.
pub fn raw_transaction(transaction: &Transaction, milestones: &Milestones) -> Value {
    let mut json = transaction.to_json(milestones.at(1));
    if let Value::Object(map) = &mut json {
        let type_group = json!(transaction.kind().type_group());
        if !map.contains_key("typeGroup") {
            let network = map.keys().position(|key| key == "network");
            map.shift_insert(
                network.map_or(0, |at| at + 1),
                "typeGroup".into(),
                type_group,
            );
        }
    }
    json
}

/// A transaction, confirmed in `block` or in the pool (`None`), as the reference's transaction
/// resource transforms it: a pool transaction has no block, no burned fee and no confirmations.
pub fn transaction(
    transaction: &Transaction,
    block: Option<&TransactionBlock>,
    milestones: &Milestones,
) -> Value {
    let data = transaction.data();
    let json = transaction.to_json(milestones.at(block.map_or(1, |block| block.height)));
    let field = |key: &str| json.get(key).cloned();
    let amount = match &data.asset {
        Asset::Transfer { transfers } => {
            let sum: u128 = transfers.iter().map(|item| u128::from(item.amount)).sum();
            Some(sum.to_string())
        }
        Asset::Burn { amount } if *amount != 0 => Some(amount.to_string()),
        _ => None,
    };
    let burned = block.map(|block| burned_fee(data.fee, milestones.at(block.height)).to_string());
    let stamp = block.and_then(|block| block.timestamp);
    defined(json!({
        "id": field("id"), "blockHeight": block.map(|block| block.height),
        "blockId": block.map(|block| block.id.to_hex()), "version": field("version"),
        "type": field("type"), "typeGroup": transaction.kind().type_group(), "amount": amount,
        "fee": field("fee"), "burnedFee": burned, "sender": field("senderId"),
        "senderPublicKey": field("senderPublicKey"), "signature": field("signature"),
        "signSignature": field("secondSignature"), "memo": field("memo"), "asset": field("asset"),
        "confirmations": block.map_or(0, |block| block.confirmations),
        "timestamp": stamp.map(|stamp| timestamp(stamp, milestones)), "nonce": field("nonce"),
    }))
}

/// A wallet, as the reference's basic wallet shows it; the delegate's version comes from the
/// online record of `pool`.
///
/// The attributes are those the reference's wallet holds (`delegate` and `secondPublicKey`), in
/// the order of a wallet registered as a delegate before it registered a second key. A delegate's
/// keys are in the order the reference's wallets have them after it builds its state from its
/// store (as it does for the genesis block's delegates): the rank before the voters, then the
/// last block, the version (the text alone) and the resignation (`permanent` or `temporary`);
/// its round is left out. The reference's order depends on each wallet's history (a key forgotten
/// and set again moves last), which the state does not keep.
pub fn wallet(wallet: &Wallet, state: &StateView, pool: &PoolView) -> Value {
    let _ = state;
    wallet_with(wallet, pool, &ProductivityTable::default())
}

/// [`wallet`] with the delegate's missed blocks and productivity, which the reference sets on the
/// wallet's delegate attribute after the resignation, from `table`.
pub fn wallet_with(wallet: &Wallet, pool: &PoolView, table: &ProductivityTable) -> Value {
    plain(wallet_searchable(wallet, pool, table))
}

/// [`wallet_with`] as the listings search and sort it: each BigNumber a
/// [`big_number`](crate::search::big_number) (the balance, the nonce, the amounts of the delegate
/// attribute and the votes of `votingFor`), which [`plain`] writes out as text.
pub fn wallet_searchable(wallet: &Wallet, pool: &PoolView, table: &ProductivityTable) -> Value {
    let key = wallet.public_key().map(|key| key.to_compressed());
    let delegate = wallet.delegate().map(|delegate| {
        let version = key.as_ref().and_then(|key| pool.version(key));
        let resigned = delegate.resigned.map(|resigned| match resigned {
            Resignation::Permanent => "permanent",
            Resignation::Temporary => "temporary",
        });
        let production = table.by_username.get(delegate.username());
        let missed = production.and_then(|production| production.missed);
        let productivity = production.and_then(|production| production.productivity);
        defined(json!({
            "username": delegate.username(), "voteBalance": big_number(delegate.vote_balance),
            "forgedFees": big_number(delegate.forged_fees),
            "burnedFees": big_number(delegate.burned_fees),
            "forgedRewards": big_number(delegate.forged_rewards),
            "donations": big_number(delegate.donations),
            "producedBlocks": delegate.produced_blocks, "rank": delegate.rank,
            "voters": delegate.voters,
            "lastBlock": delegate.last_block.as_ref().map(BlockId::to_hex),
            "version": version, "resigned": resigned,
            "missedBlocks": missed, "productivity": productivity.map(hundredths),
        }))
    });
    let second = wallet.second_public_key().map(|key| key.to_hex());
    let attributes = defined(json!({ "delegate": delegate, "secondPublicKey": second }));
    let shares = wallet.vote_shares();
    let voting_for = wallet.votes().iter().map(|(name, points)| {
        let percent = hundredths(Hundredths(u32::from(*points)));
        let votes = shares.get(name).map(big_number);
        (
            name.clone(),
            defined(json!({ "percent": percent, "votes": votes })),
        )
    });
    defined(json!({
        "address": wallet.address().to_base58(), "publicKey": key.map(|key| key.to_hex()),
        "balance": big_number(wallet.balance()), "nonce": big_number(wallet.nonce()),
        "attributes": attributes, "votingFor": Value::Object(voting_for.collect()),
    }))
}

/// A count of hundredths as a JavaScript number: whole when it is, else the fraction.
pub(crate) fn hundredths(count: Hundredths) -> Value {
    match count.0 % 100 {
        0 => json!(count.0 / 100),
        _ => json!(f64::from(count.0) / 100.0),
    }
}

/// A peer: its address, ports, version, height, latency and plugins.
pub fn peer(peer: &PeerInfo) -> Value {
    let plugins = peer.plugins.iter().map(|(name, plugin)| {
        let plugin = plugin.map(|plugin| {
            let options = (plugin.port, plugin.enabled, plugin.estimate_total_count);
            defined(
                json!({ "port": options.0, "enabled": options.1, "estimateTotalCount": options.2 }),
            )
        });
        (name.to_owned(), plugin.unwrap_or(Value::Null))
    });
    let ports = peer.ports.iter();
    let ports = ports.map(|(name, port)| (name.clone(), json!(port)));
    defined(json!({
        "ip": peer.ip.to_string(), "port": peer.port, "ports": Value::Object(ports.collect()),
        "version": peer.version, "height": peer.height, "latency": peer.latency,
        "plugins": Value::Object(plugins.collect()),
    }))
}

/// An epoch timestamp as `{epoch, unix, human}`; `null` for a time a JavaScript date cannot hold.
pub fn timestamp(epoch_stamp: u32, milestones: &Milestones) -> Value {
    match format_timestamp(i64::from(epoch_stamp), milestones.at(1).epoch()) {
        Ok(stamp) => json!({ "epoch": stamp.epoch, "unix": stamp.unix, "human": stamp.human }),
        Err(_) => Value::Null,
    }
}
