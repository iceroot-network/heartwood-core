//! The schemas of the state's listings: the delegate criteria of `GET /delegates` and
//! `GET /delegates/{id}/voters`, the wallet criteria of `GET /wallets` and `GET /wallets/top`
//! (`attributes` an open object, as the reference's `Joi.object()`), their sortable properties,
//! and `GET /delegates/{id}`'s parameter.
//!
//! A criteria key takes one value or a list of them, each validated by the key's schema (a range
//! also `{from, to}`), and its validated value is always the list. A number is validated to a JSON
//! number, a BigNumber to its decimal text and a Semver to its text; `orderBy` becomes the list of
//! `{property, direction}`, empty by default.

use std::borrow::Cow;

use serde_json::{Map, Value};

use super::{BEYOND, LIMIT, OFFSET, PAGE, PARSED, Rule, SAFE, check, invalid, joi_number};
use crate::context::RouteId;
use crate::errors::ApiError;
use crate::search::convert::{Big, json_number, text};

/// The value schema of a criteria key.
#[derive(Clone, Copy)]
enum Node {
    /// One value.
    Leaf(Leaf),
    /// An object of keys.
    Group(&'static [(&'static str, Node)]),
}

/// A value's kind.
#[derive(Clone, Copy)]
enum Leaf {
    /// `/^[a-z0-9!@$&_.]{1,20}$/`.
    Username,
    /// An address, or an address pattern with a `%`.
    Address,
    /// A public key, or a pattern with a `%`.
    PublicKey,
    /// A block's id: its height text, its hash, or a pattern with a `%`.
    BlockId,
    /// `Joi.boolean()`.
    Boolean,
    /// `valid("permanent", "temporary")`.
    Resignation,
    /// `Joi.string()`.
    Text,
    /// `Joi.object()`.
    Object,
    /// An item or `{from, to}` of items.
    Range(Item),
}

/// A range's item.
#[derive(Clone, Copy)]
enum Item {
    /// `Joi.number().integer().min(n)`.
    Integer(i64),
    /// `Joi.number().min(n)`, with a maximum.
    Number(f64, Option<f64>),
    /// A BigNumber, at least zero when the flag is set.
    Big(bool),
    /// A Semver.
    Semver,
}

use Leaf::{Address, BlockId, Boolean, Object, PublicKey, Range, Resignation, Username};
use Node::{Group, Leaf as Is};

const COUNT: Node = Is(Range(Item::Integer(0)));
const AMOUNT: Node = Is(Range(Item::Big(true)));

/// The wallet criteria's keys.
const WALLET_KEYS: &[(&str, Node)] = &[
    ("address", Is(Address)),
    ("publicKey", Is(PublicKey)),
    ("balance", Is(Range(Item::Big(false)))),
    ("nonce", AMOUNT),
    ("attributes", Is(Object)),
];

/// The delegate criteria's keys.
const DELEGATE_KEYS: &[(&str, Node)] = &[
    ("username", Is(Username)),
    ("address", Is(Address)),
    ("publicKey", Is(PublicKey)),
    (
        "votesReceived",
        Group(&[
            ("percent", Is(Range(Item::Number(0.0, Some(100.0))))),
            ("votes", AMOUNT),
            ("voters", COUNT),
        ]),
    ),
    ("rank", Is(Range(Item::Integer(1)))),
    ("isResigned", Is(Boolean)),
    ("resignationType", Is(Resignation)),
    (
        "blocks",
        Group(&[
            ("produced", COUNT),
            ("missed", COUNT),
            ("productivity", Is(Range(Item::Number(0.0, None)))),
            (
                "last",
                Group(&[
                    ("id", Is(BlockId)),
                    ("height", Is(Range(Item::Integer(1)))),
                    (
                        "timestamp",
                        Group(&[("epoch", COUNT), ("unix", COUNT), ("human", Is(Leaf::Text))]),
                    ),
                ]),
            ),
        ]),
    ),
    (
        "forged",
        Group(&[
            ("burnedFees", AMOUNT),
            ("fees", AMOUNT),
            ("rewards", AMOUNT),
            ("donations", AMOUNT),
            ("total", AMOUNT),
        ]),
    ),
    ("version", Is(Range(Item::Semver))),
];

/// The query of `route`, validated: defaults added and values coerced, or 422.
pub fn validate_query(route: RouteId, query: &Value) -> Result<Value, ApiError> {
    let (keys, wildcard): (&[(&str, Node)], &[&str]) = match route {
        RouteId::DelegatesIndex => (DELEGATE_KEYS, &[]),
        RouteId::WalletsIndex | RouteId::WalletsTop | RouteId::DelegatesVoters => {
            (WALLET_KEYS, &["attributes"])
        }
        _ => return Ok(query.clone()),
    };
    let Value::Object(input) = query else {
        return Err(ApiError::internal());
    };
    let mut input = Cow::Borrowed(input);
    let beyond = input.contains_key(BEYOND);
    if beyond {
        input.to_mut().shift_remove(BEYOND);
    }
    let mut out = (*input).clone();
    for (name, node) in keys {
        if let Some(value) = input.get(*name) {
            out.insert(
                (*name).to_owned(),
                many(node, value).ok_or_else(|| invalid(name))?,
            );
        }
    }
    match input.get("orderBy") {
        Some(value) => {
            let sorting = order_by(value, keys, wildcard).ok_or_else(|| invalid("orderBy"))?;
            out.insert("orderBy".to_owned(), sorting);
        }
        None => {
            out.insert("orderBy".to_owned(), Value::Array(Vec::new()));
        }
    }
    for key in [PAGE, OFFSET, LIMIT] {
        match input.get(key.name) {
            Some(value) => {
                let valid = check(key.rule, value).ok_or_else(|| invalid(key.name))?;
                out.insert(key.name.to_owned(), valid);
            }
            None => {
                if let Some(default) = key.default {
                    out.insert(key.name.to_owned(), default.value());
                }
            }
        }
    }
    let known = |name: &String| {
        keys.iter().any(|(key, _)| key == name)
            || ["orderBy", PAGE.name, OFFSET.name, LIMIT.name].contains(&name.as_str())
    };
    if let Some(unknown) = input.keys().find(|name| !known(name)) {
        return Err(ApiError::unprocessable(format!(
            "\"{unknown}\" is not allowed"
        )));
    }
    // The grammar wrote out no value that deep or that sparse whole; the reference fails on it.
    if beyond {
        return Err(ApiError::internal());
    }
    out.insert(PARSED.to_owned(), Value::Object(input.into_owned()));
    Ok(Value::Object(out))
}

/// Validate the path parameters of `route`: `id` is an address, a public key or a username.
pub fn validate_params(route: RouteId, params: &Map<String, Value>) -> Result<(), ApiError> {
    if !matches!(route, RouteId::DelegatesShow | RouteId::DelegatesVoters) {
        return Ok(());
    }
    const ID: [Rule; 3] = [
        Rule::Text(super::Text::Address),
        Rule::Text(super::Text::Hex(66)),
        Rule::Text(super::Text::Username),
    ];
    for (key, value) in params {
        if key != "id" {
            return Err(ApiError::unprocessable(format!("\"{key}\" is not allowed")));
        }
        check(Rule::Either(&ID), value).ok_or_else(|| invalid(key))?;
    }
    Ok(())
}

/// `Joi.array().single().items(node)`: a list of the validated values, of a value or of a list.
fn many(node: &Node, value: &Value) -> Option<Value> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|one| item(node, one))
            .collect::<Option<Vec<_>>>(),
        one => item(node, one).map(|valid| vec![valid]),
    }
    .map(Value::Array)
}

fn item(node: &Node, value: &Value) -> Option<Value> {
    match (node, value) {
        (Node::Leaf(leaf), _) => leaf.check(value),
        (Node::Group(keys), Value::Object(map)) => {
            let mut out = map.clone();
            for (name, node) in *keys {
                if let Some(value) = map.get(*name) {
                    out.insert((*name).to_owned(), many(node, value)?);
                }
            }
            map.keys()
                .all(|name| keys.iter().any(|(key, _)| key == name))
                .then_some(Value::Object(out))
        }
        (Node::Group(_), _) => None,
    }
}

impl Leaf {
    fn check(self, value: &Value) -> Option<Value> {
        let string = value.as_str();
        let all = |s: &str, ok: fn(u8) -> bool| s.bytes().all(ok);
        let pattern = |s: &str, max: usize, ok: fn(u8) -> bool| {
            s.contains('%') && (1..=max).contains(&s.len()) && all(s, ok)
        };
        let alphanum = |b: u8| b.is_ascii_alphanumeric();
        let lower = |b: u8| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'%';
        let accepted = match (self, string) {
            (Leaf::Username, Some(s)) => {
                super::text_accepts(super::Text::Username, s) && !s.is_empty()
            }
            (Leaf::Address, Some(s)) => {
                (s.len() == 34 && all(s, alphanum))
                    || pattern(s, 34, |b| b.is_ascii_alphanumeric() || b == b'%')
            }
            (Leaf::PublicKey, Some(s)) => {
                (s.len() == 66 && all(s, |b| b.is_ascii_hexdigit())) || pattern(s, 66, lower)
            }
            (Leaf::BlockId, Some(s)) => {
                let height = (13..=20).contains(&s.len()) && all(s, |b| b.is_ascii_digit());
                let hash = s.len() == 64 && all(s, |b| b.is_ascii_hexdigit());
                height
                    || hash
                    || pattern(s, 64, lower)
                    || pattern(s, 20, |b| b.is_ascii_digit() || b == b'%')
            }
            (Leaf::Resignation, Some(s)) => s == "permanent" || s == "temporary",
            (Leaf::Text, Some(s)) => !s.is_empty(),
            (Leaf::Boolean, _) => return check(Rule::Boolean, value),
            (Leaf::Object, _) => return value.is_object().then(|| value.clone()),
            (Leaf::Range(item), _) => return item.range(value),
            _ => false,
        };
        accepted.then(|| value.clone())
    }
}

impl Item {
    /// `alternatives(item, object({from: item, to: item}).or("from", "to"))`.
    fn range(self, value: &Value) -> Option<Value> {
        let Value::Object(map) = value else {
            return self.check(value);
        };
        if let Some(single) = self.check(value) {
            return Some(single);
        }
        if !(map.contains_key("from") || map.contains_key("to")) {
            return None;
        }
        let mut out = map.clone();
        for (name, bound) in map {
            if name != "from" && name != "to" {
                return None;
            }
            out.insert(name.clone(), self.check(bound)?);
        }
        Some(Value::Object(out))
    }

    fn check(self, value: &Value) -> Option<Value> {
        match self {
            Item::Integer(min) => check(Rule::Integer(min, SAFE), value),
            Item::Number(min, max) => {
                let number = joi_number(value.as_str()?)?;
                let held =
                    number.abs() <= SAFE as f64 && number >= min && max.is_none_or(|m| number <= m);
                held.then(|| json_number(number))
            }
            Item::Big(nonnegative) => {
                let big = Big::of(value)?;
                let zero = Big::parse("0")?;
                (!nonnegative || big >= zero).then(|| Value::String(big.to_string()))
            }
            Item::Semver => {
                let version = text(value);
                heartwood_p2p::utils::npm_semver::valid(&version).map(|_| Value::String(version))
            }
        }
    }
}

/// `createSortingSchema`'s `orderBy`: the list of `{property, direction}`, or `None` where the
/// reference's custom validation fails (an unknown property or direction, or a value that is not
/// text or a list of text).
fn order_by(value: &Value, keys: &[(&str, Node)], wildcard: &[&str]) -> Option<Value> {
    let entries: Vec<&str> = match value {
        Value::String(text) if text.is_empty() => return Some(Value::Array(Vec::new())),
        Value::String(text) => vec![text],
        Value::Array(items) => items.iter().map(Value::as_str).collect::<Option<_>>()?,
        _ => return None,
    };
    let mut sorting = Vec::new();
    for entry in entries {
        for one in entry.split(',') {
            let pair: Vec<&str> = one.split(':').collect();
            let property = *pair.first()?;
            let direction = pair.get(1).copied().unwrap_or("asc");
            let known = is_path(keys, property)
                || wildcard.iter().any(|w| {
                    property
                        .strip_prefix(w)
                        .is_some_and(|rest| rest.starts_with('.'))
                });
            if !known || (direction != "asc" && direction != "desc") {
                return None;
            }
            let mut sort = Map::new();
            sort.insert("property".to_owned(), Value::from(property));
            sort.insert("direction".to_owned(), Value::from(direction));
            sorting.push(Value::Object(sort));
        }
    }
    Some(Value::Array(sorting))
}

/// Whether `path` names a leaf of `keys`.
fn is_path(keys: &[(&str, Node)], path: &str) -> bool {
    let (head, rest) = match path.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (path, None),
    };
    keys.iter()
        .find(|(name, _)| *name == head)
        .is_some_and(|(_, node)| match (node, rest) {
            (Node::Leaf(_), None) => true,
            (Node::Group(inner), Some(rest)) => is_path(inner, rest),
            _ => false,
        })
}
