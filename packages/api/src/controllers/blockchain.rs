//! The chain routes: `GET /blockchain` (the tip, the burned totals and the supply) and
//! `GET /blockchain/search/{id}` (the wallets, blocks and transactions that the input starts).

use std::sync::Arc;

use heartwood_database::{
    BlockCriteria, BlockFilter, BlockRow, CountFlag, Direction, Order, Page, TransactionCriteria,
    TransactionFilter,
};
use serde_json::{Map, Value, json};

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::controllers::transactions::{Shape, present, read};
use crate::errors::ApiError;
use crate::resources::delegate::supply;
use crate::search::wallets_like;

/// How many blocks and transactions a search lists.
const LIMIT: u64 = 100;

/// `GET /blockchain`: the tip, the burned totals and the supply, all as decimal strings. The supply
/// is the sum of the balances, as the reference's calculator gives it, not the store's sums, which
/// count a genesis fee as partly burned where the state burns all of it.
pub async fn index(ctx: Arc<ApiContext>, _request: ApiRequest) -> Result<ApiReply, ApiError> {
    let state = ctx.state();
    let tip = Arc::clone(&state.last_block);
    let productivity = Arc::clone(&ctx.productivity);
    let totals = tokio::task::spawn_blocking(move || productivity.totals(&tip))
        .await
        .map_err(|_| ApiError::internal())??;
    let burned = totals
        .burned_fees
        .saturating_add(totals.burned_transactions);
    Ok(ApiReply::data(json!({
        "block": { "height": state.height(), "id": state.last_block.id().to_hex() },
        "burned": {
            "fees": totals.burned_fees.to_string(),
            "transactions": totals.burned_transactions.to_string(),
            "total": burned.to_string(),
        },
        "supply": supply(&state.wallets).to_string(),
    })))
}

/// `GET /blockchain/search/{id}`: the wallets that a delegate name, a public key or an address
/// prefix names, and the blocks and transactions whose id starts with the input (by id, 100 at
/// most each). The input is lower-cased once, and each part runs when its pattern admits it.
pub async fn search(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let input = request.param("id").unwrap_or_default().to_ascii_lowercase();
    let character = ctx.node.crypto.network.address_character.clone();
    let wallets = if names_wallet(&input, character.as_deref()) {
        wallets_like(&ctx.state(), &input)
    } else {
        Vec::new()
    };
    let (mut blocks, mut transactions) = (Vec::new(), Vec::new());
    if names_id(&input) {
        let (found, rows) = read(&ctx, move |reader| {
            let order = [Order {
                property: "id".into(),
                direction: Direction::Asc,
            }];
            let page = Page {
                offset: 0,
                limit: LIMIT,
            };
            let block_criteria = BlockCriteria {
                id: vec![input.clone()],
                ..BlockCriteria::default()
            };
            let found = reader.blocks(
                &BlockFilter::of(block_criteria),
                &order,
                page,
                false,
                CountFlag::Exact,
            )?;
            let criteria = TransactionCriteria {
                id: vec![input],
                ..TransactionCriteria::default()
            };
            let rows = reader.transactions(&TransactionFilter::of(criteria), &order, page)?;
            Ok((found.rows, rows.rows))
        })
        .await?;
        blocks.extend(found.iter().map(|listed| block_hit(&listed.row)));
        for record in &rows {
            let data = present(&record.row, Shape::Plain, false, 0, &ctx)?;
            transactions.push(transaction_hit(&data));
        }
    }
    Ok(ApiReply::data(
        json!({ "blocks": blocks, "transactions": transactions, "wallets": wallets }),
    ))
}

/// A block as the search lists it; a block without a generator name (the genesis block) has no
/// `username`.
fn block_hit(row: &BlockRow) -> Value {
    let mut hit = Map::new();
    hit.insert("height".into(), Value::from(row.height));
    hit.insert("id".into(), Value::from(row.id.to_hex()));
    hit.insert("timestamp".into(), Value::from(row.timestamp));
    hit.insert(
        "transactions".into(),
        Value::from(row.number_of_transactions),
    );
    if let Some(name) = &row.username {
        hit.insert("username".into(), Value::from(name.as_str()));
    }
    Value::Object(hit)
}

/// A transaction as the search lists it, from its raw data: the amount only when it is above
/// zero, and the recipient and asset only when the transaction has them.
fn transaction_hit(data: &Value) -> Value {
    let mut hit = Map::new();
    let copy = |hit: &mut Map<String, Value>, from: &str, to: &str| {
        if let Some(value) = data.get(from) {
            hit.insert(to.into(), value.clone());
        }
    };
    if data
        .get("amount")
        .and_then(Value::as_str)
        .is_some_and(|amount| amount.bytes().any(|digit| digit != b'0'))
    {
        copy(&mut hit, "amount", "amount");
    }
    copy(&mut hit, "asset", "asset");
    copy(&mut hit, "id", "id");
    copy(&mut hit, "recipientId", "recipient");
    copy(&mut hit, "senderId", "sender");
    copy(&mut hit, "type", "type");
    copy(&mut hit, "typeGroup", "typeGroup");
    Value::Object(hit)
}

/// Whether the wallet search runs for the lower-cased `input`: it reads as a delegate name, a
/// public key or an address of the network (`address_character` is the network's first address
/// character; a network without one takes the reference's literal `undefined`).
fn names_wallet(input: &str, address_character: Option<&str>) -> bool {
    is_delegate_name(input)
        || is_public_key_prefix(input)
        || is_address_prefix(input, address_character.unwrap_or("undefined"))
}

/// The reference's `^(?!_)(?=.*[a-z!@$&_.])([a-z0-9!@$&_.]?){1,20}$`: one to twenty characters of
/// the name alphabet, not starting with an underscore, and not only digits.
fn is_delegate_name(input: &str) -> bool {
    let allowed =
        |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"!@$&_.".contains(&byte);
    (1..=20).contains(&input.len())
        && input.bytes().all(allowed)
        && !input.starts_with('_')
        && input.bytes().any(|byte| !byte.is_ascii_digit())
}

/// The reference's `^0([23]){1}([a-z0-9]){19,64}$`.
fn is_public_key_prefix(input: &str) -> bool {
    let rest = input
        .strip_prefix("02")
        .or_else(|| input.strip_prefix("03"));
    rest.is_some_and(|rest| {
        (19..=64).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// The reference's ``^(${addressCharacter})([a-z1-9]{20,33})$``, the character read literally.
fn is_address_prefix(input: &str, character: &str) -> bool {
    let character = character.to_ascii_lowercase();
    input.strip_prefix(character.as_str()).is_some_and(|rest| {
        (20..=33).contains(&rest.len())
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || (b'1'..=b'9').contains(&byte))
    })
}

/// Whether the block and transaction scans run: the reference's `^([a-z0-9]){21,64}$`.
fn names_id(input: &str) -> bool {
    (21..=64).contains(&input.len()) && input.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delegate_name_has_one_to_twenty_characters_and_a_non_digit() {
        assert!(is_delegate_name("genesis_1"));
        assert!(is_delegate_name("a"));
        assert!(is_delegate_name("!"));
        assert!(!is_delegate_name("12345"));
        assert!(!is_delegate_name("_abc"));
        assert!(!is_delegate_name(""));
        assert!(!is_delegate_name("abcdefghijklmnopqrstu"));
        assert!(is_delegate_name("abcdefghijklmnopqrst"));
        assert!(!is_delegate_name("bad name"));
    }

    #[test]
    fn a_public_key_prefix_starts_02_or_03_and_has_21_to_66_characters() {
        let key = format!("02{}", "a".repeat(19));
        assert!(is_public_key_prefix(&key));
        assert!(is_public_key_prefix(&format!("03{}", "b".repeat(64))));
        assert!(!is_public_key_prefix(&format!("04{}", "a".repeat(19))));
        assert!(!is_public_key_prefix(&format!("02{}", "a".repeat(18))));
        assert!(!is_public_key_prefix(&format!("02{}", "a".repeat(65))));
    }

    #[test]
    fn an_address_prefix_follows_the_networks_character() {
        let address = format!("d{}", "q".repeat(20));
        assert!(is_address_prefix(&address, "D"));
        assert!(!is_address_prefix(&address, "S"));
        assert!(!is_address_prefix(&format!("d{}0", "q".repeat(19)), "d"));
        assert!(!is_address_prefix(&format!("d{}", "q".repeat(34)), "d"));
        assert!(names_wallet(&format!("undefined{}", "q".repeat(20)), None));
    }

    #[test]
    fn an_id_scan_takes_21_to_64_alphanumeric_characters() {
        assert!(names_id(&"a".repeat(21)));
        assert!(names_id(&"f".repeat(64)));
        assert!(!names_id(&"a".repeat(20)));
        assert!(!names_id(&"a".repeat(65)));
        assert!(!names_id(&format!("{}_", "a".repeat(21))));
    }

    #[test]
    fn a_transaction_hit_keeps_a_positive_amount_only() {
        let data = json!({
            "amount": "0", "id": "ab", "senderId": "D1", "type": 6, "typeGroup": 1,
            "nonce": "1",
        });
        assert_eq!(
            transaction_hit(&data),
            json!({ "id": "ab", "sender": "D1", "type": 6, "typeGroup": 1 })
        );
        let burn = json!({ "amount": "5", "id": "ab", "type": 6, "typeGroup": 1 });
        assert_eq!(transaction_hit(&burn)["amount"], "5");
    }
}
