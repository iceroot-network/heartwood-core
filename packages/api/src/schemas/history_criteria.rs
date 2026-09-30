//! The schemas of the store's history routes: the block criteria and sort keys (`GET /blocks`,
//! `GET /delegates/{id}/blocks` without `username` and `generatorPublicKey`), the missed-block
//! criteria and sort keys (`GET /blocks/missed`, `GET /delegates/{id}/blocks/missed`), the round's
//! parameter, the vote criteria (`GET /votes`, `GET /votes/{id}`), `GET /blocks/first`, and a
//! wallet's sent, received and vote transactions.
//!
//! Each route's query is checked as the reference's Joi schema checks it: an unknown key is 422,
//! a value is converted from its text, the defaults are added (`orderBy` of the blocks
//! `height:desc`, of the missed slots `timestamp:desc`), and the sort keys must name a criterion of
//! the route in lowercase direction. The two block keys with patterns of their own, `username`
//! (not starting with `_`, not only digits) and `payloadHash` (hexadecimal of any length), are
//! checked after the generic rules.

use serde_json::{Map, Value};

use super::{
    BEYOND, Key, LIMIT, MIN_0, MIN_1, OFFSET, PAGE, PARSED, Rule, SAFE, TRANSACTION_QUERY,
    TRANSFORM, Text, any, check, invalid, key, validate_keys,
};
use crate::context::RouteId;
use crate::errors::ApiError;

const HEX_64: Rule = Rule::Text(Text::Hex(64));

/// The block criteria, in the order of the reference's schema.
const BLOCK_CRITERIA: [Key; 15] = [
    key("id", any(&HEX_64), None),
    key("version", any(&MIN_0), None),
    key("timestamp", any(&Rule::Range(&MIN_0)), None),
    key("previousBlock", any(&HEX_64), None),
    key("height", any(&Rule::Range(&MIN_1)), None),
    key("numberOfTransactions", any(&Rule::Range(&MIN_0)), None),
    key("totalAmount", any(&Rule::Range(&MIN_0)), None),
    key("totalFee", any(&Rule::Range(&MIN_0)), None),
    key("burnedFee", any(&Rule::Range(&MIN_0)), None),
    key("reward", any(&Rule::Range(&MIN_0)), None),
    key("payloadLength", any(&Rule::Range(&MIN_0)), None),
    key("payloadHash", any(&Rule::Text(Text::Any)), None),
    key("generatorPublicKey", any(&Rule::Text(Text::Hex(66))), None),
    key("username", any(&Rule::Text(Text::Any)), None),
    key("blockSignature", any(&Rule::Text(Text::Hex(128))), None),
];

/// The missed-slot criteria of `GET /blocks/missed`.
const MISSED_CRITERIA: [Key; 3] = [BLOCK_CRITERIA[2], BLOCK_CRITERIA[4], BLOCK_CRITERIA[13]];

/// `orderBy`: the pattern here, the known properties and the default in [`Sorting`].
const ORDER_BY: Key = key("orderBy", Rule::OrderBy(false), None);

/// The wallet that owns a history: an address, a public key or a delegate name.
const WALLET: [Rule; 3] = [
    Rule::Text(Text::Address),
    Rule::Text(Text::Hex(66)),
    Rule::Text(Text::Username),
];

/// The sort keys of a block route, and the order it sorts by without `orderBy`.
struct Sorting {
    known: &'static [&'static str],
    default: &'static str,
}

const BLOCK_KEYS: [&str; 15] = [
    "id",
    "version",
    "timestamp",
    "previousBlock",
    "height",
    "numberOfTransactions",
    "totalAmount",
    "totalFee",
    "burnedFee",
    "reward",
    "payloadLength",
    "payloadHash",
    "generatorPublicKey",
    "username",
    "blockSignature",
];

/// The block keys of a delegate's blocks: all but `generatorPublicKey` and `username`.
const DELEGATE_KEYS: [&str; 13] = [
    "id",
    "version",
    "timestamp",
    "previousBlock",
    "height",
    "numberOfTransactions",
    "totalAmount",
    "totalFee",
    "burnedFee",
    "reward",
    "payloadLength",
    "payloadHash",
    "blockSignature",
];

const MISSED_KEYS: [&str; 3] = ["timestamp", "height", "username"];

const BLOCKS: Sorting = Sorting {
    known: &BLOCK_KEYS,
    default: "height:desc",
};
const DELEGATE_BLOCKS: Sorting = Sorting {
    known: &DELEGATE_KEYS,
    default: "height:desc",
};
const MISSED: Sorting = Sorting {
    known: &MISSED_KEYS,
    default: "timestamp:desc",
};

/// The query of `route`, validated: defaults added and values coerced, or 422.
pub fn validate_query(route: RouteId, query: &Value) -> Result<Value, ApiError> {
    let Value::Object(input) = query else {
        return Err(ApiError::internal());
    };
    let mut input = input.clone();
    let beyond = input.shift_remove(BEYOND).is_some();
    let tail = [ORDER_BY, TRANSFORM, PAGE, OFFSET, LIMIT];
    let (schema, paginates, sorting): (Vec<Key>, bool, Option<&Sorting>) = match route {
        RouteId::BlocksIndex => ([&BLOCK_CRITERIA[..], &tail].concat(), true, Some(&BLOCKS)),
        RouteId::DelegatesBlocks => {
            let criteria = BLOCK_CRITERIA
                .iter()
                .filter(|k| DELEGATE_KEYS.contains(&k.name));
            let keys = criteria.copied().chain(tail).collect();
            (keys, true, Some(&DELEGATE_BLOCKS))
        }
        RouteId::BlocksMissed => (
            [&MISSED_CRITERIA[..], &tail[0..1], &tail[2..]].concat(),
            true,
            Some(&MISSED),
        ),
        RouteId::DelegatesMissedBlocks => {
            let criteria = BLOCK_CRITERIA
                .iter()
                .filter(|k| DELEGATE_KEYS.contains(&k.name));
            let keys = criteria
                .copied()
                .chain(tail)
                .filter(|k| k.name != "transform");
            (keys.collect(), true, Some(&MISSED))
        }
        RouteId::BlocksFirst | RouteId::VotesShow => (vec![TRANSFORM], false, None),
        RouteId::VotesIndex
        | RouteId::WalletsTransactionsSent
        | RouteId::WalletsTransactionsReceived
        | RouteId::WalletsVotes => (TRANSACTION_QUERY.to_vec(), true, None),
        // The round's route has no query schema: any query is taken as it is.
        _ => return Ok(Value::Object(input)),
    };
    let mut out = validate_keys(&schema, &input)?;
    if let Some(sorting) = sorting {
        sort_and_patterns(&mut out, sorting, input.len())?;
    }
    if beyond {
        return Err(ApiError::internal());
    }
    if paginates {
        out.insert(PARSED.to_owned(), Value::Object(input));
    }
    Ok(Value::Object(out))
}

/// The checks of a block route after its generic rules: the two patterns, and the sort keys. A
/// missing order gets the route's default where Joi puts it: after the `given` keys of the query,
/// before the other defaults, which follow it in the schema.
fn sort_and_patterns(
    out: &mut Map<String, Value>,
    sorting: &Sorting,
    given: usize,
) -> Result<(), ApiError> {
    for (name, accepts) in [
        ("username", username as fn(&str) -> bool),
        ("payloadHash", payload_hash),
    ] {
        if let Some(value) = out.get(name) {
            let items = value
                .as_array()
                .map_or_else(|| std::slice::from_ref(value), Vec::as_slice);
            if !items.iter().all(|item| item.as_str().is_some_and(accepts)) {
                return Err(invalid(name));
            }
        }
    }
    match out.get("orderBy") {
        Some(order) => known_order(order, sorting.known),
        None => {
            let place = given.min(out.len());
            out.shift_insert(place, "orderBy".to_owned(), Value::from(sorting.default));
            Ok(())
        }
    }
}

/// `/^(?!_)(?=.*[a-z!@$&_.])([a-z0-9!@$&_.]?)+$/`, from 1 to 20 characters: of the delegate-name
/// characters, not starting with `_`, and not only digits.
fn username(text: &str) -> bool {
    let letters = |b: u8| b.is_ascii_lowercase() || b"!@$&_.".contains(&b);
    (1..=20).contains(&text.len())
        && !text.starts_with('_')
        && text.bytes().all(|b| letters(b) || b.is_ascii_digit())
        && text.bytes().any(letters)
}

/// A hexadecimal text of any length.
fn payload_hash(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The reference's sort check: each `property:direction`, split at commas, names a `known`
/// property and gives `asc` or `desc` in lowercase.
fn known_order(order: &Value, known: &[&str]) -> Result<(), ApiError> {
    let items = order
        .as_array()
        .map_or_else(|| std::slice::from_ref(order), Vec::as_slice);
    for item in items.iter().filter_map(Value::as_str) {
        for entry in item.split(',') {
            let (property, direction) = entry.split_once(':').unwrap_or((entry, "asc"));
            if !known.contains(&property) {
                return Err(ApiError::unprocessable(format!(
                    "Unknown orderBy property '{property}'"
                )));
            }
            if direction != "asc" && direction != "desc" {
                return Err(ApiError::unprocessable(format!(
                    "Unexpected orderBy direction '{direction}' for property '{property}'"
                )));
            }
        }
    }
    Ok(())
}

/// Validate the path parameters of `route`.
pub fn validate_params(route: RouteId, params: &Map<String, Value>) -> Result<(), ApiError> {
    let rule = match route {
        RouteId::RoundsDelegates => Rule::Integer(1, SAFE),
        RouteId::VotesShow => HEX_64,
        RouteId::DelegatesBlocks
        | RouteId::DelegatesMissedBlocks
        | RouteId::WalletsTransactionsSent
        | RouteId::WalletsTransactionsReceived
        | RouteId::WalletsVotes => Rule::Either(&WALLET),
        _ => return Ok(()),
    };
    for (name, value) in params {
        if name != "id" {
            return Err(ApiError::unprocessable(format!(
                "\"{name}\" is not allowed"
            )));
        }
        check(rule, value).ok_or_else(|| invalid(name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn query(route: RouteId, value: Value) -> Result<Value, u16> {
        validate_query(route, &value).map_err(|error| error.status)
    }

    #[test]
    fn a_block_query_gets_its_defaults_and_keeps_the_query_as_parsed() {
        let got = query(RouteId::BlocksIndex, json!({"height": "5"})).unwrap();
        assert_eq!(got["height"], 5);
        assert_eq!(got["orderBy"], "height:desc");
        assert_eq!(got["transform"], true);
        assert_eq!(got["limit"], 100);
        assert_eq!(got[PARSED], json!({"height": "5"}));
    }

    #[test]
    fn the_missed_routes_take_three_keys_and_no_transform() {
        let got = query(RouteId::BlocksMissed, json!({"username": "genesis_1"})).unwrap();
        assert_eq!(got["orderBy"], "timestamp:desc");
        assert!(got.get("transform").is_none());
        for other in ["id", "generatorPublicKey", "transform", "reward"] {
            let mut given = Map::new();
            given.insert(other.to_owned(), json!("1"));
            assert_eq!(query(RouteId::BlocksMissed, Value::Object(given)), Err(422));
        }
        assert_eq!(
            query(RouteId::BlocksMissed, json!({"orderBy": "id:asc"})),
            Err(422)
        );
        assert!(query(RouteId::BlocksMissed, json!({"orderBy": "username:asc"})).is_ok());
    }

    #[test]
    fn a_delegates_blocks_leave_out_the_username_and_the_key() {
        assert_eq!(
            query(RouteId::DelegatesBlocks, json!({"username": "a"})),
            Err(422)
        );
        let key = "0".repeat(66);
        let given = json!({ "generatorPublicKey": key });
        assert_eq!(query(RouteId::DelegatesBlocks, given), Err(422));
        assert!(query(RouteId::DelegatesBlocks, json!({"reward": {"from": "1"}})).is_ok());
        assert_eq!(
            query(RouteId::DelegatesBlocks, json!({"orderBy": "username:asc"})),
            Err(422)
        );
        assert_eq!(
            query(RouteId::DelegatesMissedBlocks, json!({"transform": "true"})),
            Err(422)
        );
        assert!(query(RouteId::DelegatesMissedBlocks, json!({"reward": "1"})).is_ok());
    }

    #[test]
    fn the_username_pattern_is_the_blocks_not_the_wallets() {
        for good in ["a", "genesis_1", "a1", "1a", "1.1", "!"] {
            assert!(username(good), "{good}");
        }
        for bad in ["", "_a", "123", "A", "a b", "abcdefghijklmnopqrstu"] {
            assert!(!username(bad), "{bad}");
        }
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"username": "123"})),
            Err(422)
        );
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"username": ["a", "_b"]})),
            Err(422)
        );
        assert!(query(RouteId::BlocksIndex, json!({"username": ["a", "b"]})).is_ok());
    }

    #[test]
    fn the_payload_hash_is_hexadecimal_of_any_length() {
        assert!(query(RouteId::BlocksIndex, json!({"payloadHash": "aB0"})).is_ok());
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"payloadHash": "xyz"})),
            Err(422)
        );
    }

    #[test]
    fn the_sort_keys_are_known_and_lowercase() {
        assert!(
            query(
                RouteId::BlocksIndex,
                json!({"orderBy": ["timestamp:asc", "id:desc"]})
            )
            .is_ok()
        );
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"orderBy": "donations:asc"})),
            Err(422)
        );
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"orderBy": "height:DESC"})),
            Err(422)
        );
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"orderBy": "height"})),
            Err(422)
        );
    }

    #[test]
    fn an_unknown_key_and_out_of_range_values_are_refused() {
        assert_eq!(query(RouteId::BlocksIndex, json!({"nope": "1"})), Err(422));
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"height": "0"})),
            Err(422)
        );
        assert_eq!(
            query(RouteId::BlocksIndex, json!({"limit": "101"})),
            Err(422)
        );
        assert_eq!(query(RouteId::BlocksIndex, json!({"id": "abc"})), Err(422));
    }

    #[test]
    fn the_first_block_and_a_vote_take_only_transform() {
        assert_eq!(
            query(RouteId::BlocksFirst, json!({})).unwrap(),
            json!({"transform": true})
        );
        assert_eq!(
            query(RouteId::VotesShow, json!({"transform": "false"})).unwrap(),
            json!({"transform": false})
        );
        assert_eq!(query(RouteId::BlocksFirst, json!({"page": "1"})), Err(422));
    }

    #[test]
    fn the_transaction_routes_take_the_transaction_schema() {
        let got = query(RouteId::VotesIndex, json!({"amount": "0"})).unwrap();
        assert_eq!(got["orderBy"], json!(["timestamp:desc", "sequence:desc"]));
        assert_eq!(query(RouteId::WalletsVotes, json!({"nope": "1"})), Err(422));
    }

    #[test]
    fn the_round_route_takes_any_query() {
        assert_eq!(
            query(RouteId::RoundsDelegates, json!({"x": "1"})).unwrap(),
            json!({"x": "1"})
        );
    }

    #[test]
    fn a_beyond_value_is_500_once_the_query_is_valid() {
        let given = json!({".beyond": ["asset"], "height": "0"});
        assert_eq!(query(RouteId::BlocksIndex, given), Err(422));
        assert_eq!(
            query(RouteId::BlocksIndex, json!({".beyond": ["asset"]})),
            Err(500)
        );
    }

    #[test]
    fn the_parameters_are_checked_by_route() {
        let params = |id: &str| {
            let mut map = Map::new();
            map.insert("id".to_owned(), json!(id));
            map
        };
        let ok = |route, id: &str| validate_params(route, &params(id)).is_ok();
        assert!(ok(RouteId::RoundsDelegates, "1"));
        assert!(!ok(RouteId::RoundsDelegates, "0"));
        assert!(!ok(RouteId::RoundsDelegates, "x"));
        assert!(ok(RouteId::VotesShow, &"a".repeat(64)));
        assert!(!ok(RouteId::VotesShow, "abc"));
        assert!(ok(RouteId::WalletsVotes, "genesis_1"));
        assert!(ok(RouteId::WalletsVotes, &"D".repeat(34)));
        assert!(ok(RouteId::DelegatesBlocks, &"0".repeat(66)));
        assert!(!ok(RouteId::WalletsVotes, "Genesis"));
        let mut extra = params("1");
        extra.insert("other".to_owned(), json!("1"));
        assert!(validate_params(RouteId::RoundsDelegates, &extra).is_err());
    }
}
