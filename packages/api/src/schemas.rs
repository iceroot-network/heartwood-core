//! The query grammar and the routes' validation, as the reference's server reads them.
//!
//! [`parse_query`] reads a query string as the reference's HTTP server does (Node's
//! `querystring.parse`: `&` and `=`, `+` as a space, percent escapes, at most 1,000 pairs, a
//! repeated key as a list), then applies its comma-array and dot-nesting plugins: `a=1,2` is
//! `["1", "2"]` (any value, `memo` included), `a.b=1` is `{"a": {"b": "1"}}`, and keys are
//! enumerated as JavaScript enumerates an object's keys. [`validate_query`], [`validate_params`]
//! and [`validate_payload`] apply each route's schema as the reference's Joi schemas do: the
//! accepted keys, ranges and defaults, and values converted (numbers and booleans from text). A
//! failure is 422 with a message in Joi's style, whose text is not part of the contract; an unknown
//! query key is 422 on the routes whose schema lists their keys, and ignored on the others.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use heartwood_crypto::utils::js;
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::context::RouteId;
use crate::errors::ApiError;

/// The key under which [`validate_query`] keeps the query as parsed, beside the validated keys, on
/// the routes that paginate. No parsed query can hold it, because the grammar nests every name
/// that has a dot. The pagination links read it: the reference renders them from the validated
/// query overlaid with the query as parsed ([`crate::pagination::envelope`]).
pub const PARSED: &str = ".parsed";

/// The most pairs Node's `querystring.parse` reads.
const MAX_PAIRS: usize = 1000;

/// The most levels of objects and lists below the query that a parsed query is written out with
/// (serde_json's own nesting limit). The reference nests a value as deep as its key has segments.
const MAX_DEPTH: usize = 128;

/// The most holes of sparse lists, in all, that a parsed query is written out with. The reference
/// keeps a list sparse at any length.
const MAX_HOLES: usize = 16_384;

/// The key under which [`parse_query`] names the keys whose value it did not write out whole. No
/// parsed query can hold it, because the grammar nests every name that has a dot.
const BEYOND: &str = ".beyond";

/// The most transactions of one `POST /transactions` (the pool's `maxTransactionsPerRequest`).
const MAX_TRANSACTIONS_PER_REQUEST: usize = 40;

/// A value of the query before the dot-nesting plugin: text, or a list of texts.
enum Flat {
    Text(String),
    List(Vec<String>),
}

/// An object of the query as the reference's plugins build it. Nodes refer to each other by
/// their place in a [`Tree`], as JavaScript's values refer to objects, so a later key can reach
/// and change a value that an earlier key's snapshot still holds.
enum Node {
    Text(String),
    Map(IndexMap<String, usize>),
    List(List),
}

/// A JavaScript array: its `length`, its items by index (a missing index below the length is a
/// hole, `null` in JSON), and its named properties, which neither JSON nor a schema sees but a
/// later key can reach.
#[derive(Default)]
struct List {
    length: u32,
    items: BTreeMap<u32, usize>,
    named: HashMap<String, usize>,
}

/// The query's nodes; the first is the query itself.
#[derive(Default)]
struct Tree {
    nodes: Vec<Node>,
}

/// The query itself, the first node of a [`Tree`].
const ROOT: usize = 0;

impl Tree {
    fn add(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// JavaScript's `typeof value === "object"`, as the dot-nesting plugin tests it.
    fn is_object(&self, id: usize) -> bool {
        matches!(self.nodes.get(id), Some(Node::Map(_) | Node::List(_)))
    }

    /// The property `name` of the object `at`, where the kernel's `set` reads it; a list's
    /// `length` holds a number, to which `set` then assigns `{}`, which throws a RangeError.
    fn get(&self, at: usize, name: &str) -> Result<Option<usize>, Throw> {
        Ok(match self.nodes.get(at) {
            Some(Node::Map(map)) => map.get(name).copied(),
            Some(Node::List(_)) if name == "length" => return Err(Throw),
            Some(Node::List(list)) => match index(name) {
                Some(index) => list.items.get(&index).copied(),
                None => list.named.get(name).copied(),
            },
            _ => None,
        })
    }

    /// Assign the node `id` to the property `name` of the object `at`.
    fn put(&mut self, at: usize, name: &str, id: usize) {
        match self.nodes.get_mut(at) {
            Some(Node::Map(map)) => {
                map.insert(name.to_owned(), id);
            }
            Some(Node::List(list)) => match index(name) {
                Some(index) => {
                    list.items.insert(index, id);
                    list.length = list.length.max(index.saturating_add(1));
                }
                None => {
                    list.named.insert(name.to_owned(), id);
                }
            },
            _ => {}
        }
    }

    /// The query's own keys (the first node's).
    fn keys(&mut self) -> Option<&mut IndexMap<String, usize>> {
        match self.nodes.get_mut(ROOT) {
            Some(Node::Map(map)) => Some(map),
            _ => None,
        }
    }
}

/// `name` as an array index: canonical digits below 2^32 - 1.
fn index(name: &str) -> Option<u32> {
    js::is_array_index(name)
        .then(|| name.parse().ok())
        .flatten()
}

/// The reference's query plugins threw on the query, which its server answers with 500.
struct Throw;

/// The query string `raw` (without the `?`), parsed.
///
/// `Value::Null` stands for a query on which the reference's plugins throw, which its server
/// answers with 500 on every path: a repeated key one of whose values is a lone comma, or a
/// `length` segment below a list. [`validate_query`] answers it with that 500.
///
/// The work and the answer are bounded whatever the query: the keys are walked without recursion,
/// lists stay sparse while they are built, and the answer is written out at most 128 levels deep
/// with at most 16,384 holes in all. A value beyond either bound is written out shortened (a
/// deeper object or list empty, each run of holes past the bound as one hole), and its key is
/// named under a reserved key: every route's schema refuses such a value but the `asset` criteria,
/// which [`validate_query`] then answers with 500, as the reference answers a value too large for
/// its stack or its strings.
pub fn parse_query(raw: &str) -> Value {
    let Ok(tree) = parse(raw) else {
        return Value::Null;
    };
    let mut writer = Writer {
        tree: &tree,
        holes: MAX_HOLES,
        cut: false,
    };
    let Some(Node::Map(keys)) = tree.nodes.get(ROOT) else {
        return Value::Null;
    };
    let mut query = Map::new();
    let mut beyond = Vec::new();
    for key in keys_in_order(keys) {
        let Some(&id) = keys.get(key) else {
            continue;
        };
        writer.cut = false;
        query.insert(key.clone(), writer.value(id, 1));
        if writer.cut {
            beyond.push(Value::String(key.clone()));
        }
    }
    if !beyond.is_empty() {
        query.insert(BEYOND.to_owned(), Value::Array(beyond));
    }
    Value::Object(query)
}

fn parse(raw: &str) -> Result<Tree, Throw> {
    let mut query: IndexMap<String, Flat> = IndexMap::new();
    for (key, value) in pairs(raw) {
        match query.get_mut(&key) {
            None => {
                query.insert(key, Flat::Text(value));
            }
            Some(Flat::List(list)) => list.push(value),
            Some(Flat::Text(first)) => {
                let first = std::mem::take(first);
                query.insert(key, Flat::List(vec![first, value]));
            }
        }
    }
    // The comma-array plugin. A list (a repeated key) is left alone, but `Array.prototype.indexOf`
    // finds a lone comma in it, and the plugin then calls `split` on the list and throws.
    for item in query.values_mut() {
        match item {
            Flat::Text(text) if text.contains(',') => {
                *item = Flat::List(text.split(',').map(str::to_owned).collect());
            }
            Flat::List(list) if list.iter().any(|x| x == ",") => return Err(Throw),
            _ => {}
        }
    }
    let mut tree = Tree::default();
    tree.add(Node::Map(IndexMap::new()));
    for (key, value) in query {
        let id = match value {
            Flat::Text(text) => tree.add(Node::Text(text)),
            Flat::List(texts) => {
                let mut list = List::default();
                for (i, text) in (0..u32::MAX).zip(texts) {
                    list.items.insert(i, tree.add(Node::Text(text)));
                    list.length = i + 1;
                }
                tree.add(Node::List(list))
            }
        };
        tree.put(ROOT, &key, id);
    }
    // The dot-nesting plugin: each key of a snapshot, in order, is deleted and set again by path.
    let snapshot: Vec<(String, usize)> = match tree.keys() {
        Some(keys) => keys_in_order(keys)
            .into_iter()
            .filter_map(|key| Some((key.clone(), *keys.get(key)?)))
            .collect(),
        None => Vec::new(),
    };
    for (key, value) in snapshot {
        if let Some(keys) = tree.keys() {
            keys.shift_remove(&key);
        }
        let segments: Vec<&str> = key.split('.').collect();
        let forbidden = ["__proto__", "prototype", "constructor"];
        if !segments.iter().any(|s| forbidden.contains(s)) {
            set(&mut tree, &segments, value)?;
        }
    }
    Ok(tree)
}

/// The kernel's `set`, a loop over the path's segments: every segment whose property does not
/// hold an object gets `{}`, the last gets the value.
fn set(tree: &mut Tree, path: &[&str], value: usize) -> Result<(), Throw> {
    let mut at = ROOT;
    for (i, name) in path.iter().enumerate() {
        let current = tree.get(at, name)?;
        let next = match current {
            _ if i + 1 == path.len() => value,
            Some(id) if tree.is_object(id) => id,
            _ => tree.add(Node::Map(IndexMap::new())),
        };
        if current != Some(next) {
            tree.put(at, name, next);
        }
        at = next;
    }
    Ok(())
}

/// The keys of `map` in the order JavaScript enumerates them: array indexes first, in numeric
/// order, then the others in insertion order.
fn keys_in_order<V>(map: &IndexMap<String, V>) -> Vec<&String> {
    let mut indexes: Vec<(u32, &String)> = map
        .keys()
        .filter_map(|key| Some((index(key)?, key)))
        .collect();
    indexes.sort_by_key(|(index, _)| *index);
    let others = map.keys().filter(|key| index(key).is_none());
    indexes
        .into_iter()
        .map(|(_, key)| key)
        .chain(others)
        .collect()
}

/// A query's tree written out as JSON (`JSON.stringify`: holes as `null`, a list's named
/// properties left out), within [`MAX_DEPTH`] and [`MAX_HOLES`].
struct Writer<'t> {
    tree: &'t Tree,
    /// The holes left to write out.
    holes: usize,
    /// Whether the value being written was shortened.
    cut: bool,
}

impl Writer<'_> {
    /// The node `id`, `depth` levels below the query.
    fn value(&mut self, id: usize, depth: usize) -> Value {
        let tree = self.tree;
        match tree.nodes.get(id) {
            Some(Node::Text(text)) => Value::String(text.clone()),
            Some(Node::Map(map)) if depth >= MAX_DEPTH => {
                self.cut |= !map.is_empty();
                Value::Object(Map::new())
            }
            Some(Node::Map(map)) => {
                let keys = keys_in_order(map).into_iter();
                let entries = keys.filter_map(|key| Some((key, *map.get(key)?)));
                let entries = entries.map(|(key, id)| (key.clone(), self.value(id, depth + 1)));
                Value::Object(entries.collect())
            }
            Some(Node::List(list)) if depth >= MAX_DEPTH => {
                self.cut |= list.length > 0;
                Value::Array(Vec::new())
            }
            Some(Node::List(list)) => {
                let holes = (list.length as usize).saturating_sub(list.items.len());
                let whole = holes <= self.holes;
                match whole {
                    true => self.holes -= holes,
                    false => self.cut = true,
                }
                let mut out = Vec::with_capacity(list.items.len() + if whole { holes } else { 0 });
                let mut next = 0;
                let items = list.items.iter().map(|(&index, &id)| (index, Some(id)));
                for (index, id) in items.chain([(list.length, None)]) {
                    let gap = index.saturating_sub(next) as usize;
                    let gap = if whole { gap } else { gap.min(1) };
                    out.extend(std::iter::repeat_n(Value::Null, gap));
                    if let Some(id) = id {
                        out.push(self.value(id, depth + 1));
                    }
                    next = index.saturating_add(1);
                }
                Value::Array(out)
            }
            None => Value::Null,
        }
    }
}

/// Node's `querystring.parse` into pairs: `&`-separated, split at the first `=`, `+` as a space,
/// decoded, empty segments skipped (they count toward the 1,000 pairs).
fn pairs(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if raw.is_empty() {
        return out;
    }
    let segments: Vec<&str> = raw.split('&').collect();
    let mut budget = MAX_PAIRS;
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        if segment.is_empty() {
            if last {
                break;
            }
            budget -= 1;
            if budget == 0 {
                break;
            }
            continue;
        }
        let (key, value) = segment.split_once('=').unwrap_or((segment, ""));
        out.push((decode(key, false), decode(value, true)));
        budget -= 1;
        if last || budget == 0 {
            break;
        }
    }
    out
}

/// A key or value as Node decodes it: `+` becomes a space, and when the text holds an escape
/// (`%` and two hex digits) it is decoded as `decodeURIComponent` does, or, where that fails, as
/// `querystring.unescapeBuffer` does (escapes as bytes, other characters truncated to a byte,
/// then UTF-8 with replacement characters).
fn decode(raw: &str, value: bool) -> String {
    let mut run = 0;
    let mut escaped = false;
    for c in raw.chars() {
        if c == '%' {
            run = 1;
        } else if c == '+' && value {
            // In a value Node checks `+` before the escape, so it does not end a run.
        } else if run > 0 && c.is_ascii_hexdigit() {
            run += 1;
            if run == 3 {
                escaped = true;
                break;
            }
        } else {
            run = 0;
        }
    }
    let text = raw.replace('+', " ");
    if !escaped {
        return text;
    }
    decode_uri_component(&text).unwrap_or_else(|| unescape_buffer(&text))
}

fn hex_at(text: &[u16], at: usize) -> Option<u8> {
    let digit = |unit: u16| char::from_u32(u32::from(unit))?.to_digit(16);
    let high = digit(*text.get(at)?)?;
    let low = digit(*text.get(at + 1)?)?;
    u8::try_from(high * 16 + low).ok()
}

/// `decodeURIComponent`: `None` where it throws a URIError.
fn decode_uri_component(text: &str) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out: Vec<u16> = Vec::with_capacity(units.len());
    let mut i = 0;
    while let Some(&unit) = units.get(i) {
        if unit != u16::from(b'%') {
            out.push(unit);
            i += 1;
            continue;
        }
        let lead = hex_at(&units, i + 1)?;
        let length = match lead {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return None,
        };
        let mut bytes = vec![lead];
        for k in 1..length {
            let at = i + 3 * k;
            if units.get(at) != Some(&u16::from(b'%')) {
                return None;
            }
            bytes.push(hex_at(&units, at + 1)?);
        }
        out.extend(std::str::from_utf8(&bytes).ok()?.encode_utf16());
        i += 3 * length;
    }
    String::from_utf16(&out).ok()
}

/// Node's `querystring.unescapeBuffer`, then `Buffer.toString()`.
fn unescape_buffer(text: &str) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while let Some(&unit) = units.get(i) {
        let escape = (unit == u16::from(b'%') && i + 2 < units.len())
            .then(|| hex_at(&units, i + 1))
            .flatten();
        if let Some(byte) = escape {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push((unit & 0xff) as u8);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A rule of a route's schema: the part of Joi's types that the served routes use.
#[derive(Debug, Clone, Copy)]
enum Rule {
    /// `Joi.bool()`: `true` or `false` in any case.
    Boolean,
    /// `Joi.number().integer()` with a minimum and a maximum (safe integers only).
    Integer(i64, i64),
    /// `Joi.string()` of a kind; never empty.
    Text(Text),
    /// The `orderBy` pattern, and for the transaction lists the known properties.
    OrderBy(bool),
    /// A value or a list of values (the criteria's `orCriteria`).
    OneOrMore(&'static Rule),
    /// A number, or `{from}`, `{to}` or `{from, to}` of numbers (`numericCriteria`).
    Range(&'static Rule),
    /// `Joi.object()`: any object.
    Object,
    /// `Joi.alternatives()`: the first rule that accepts the value.
    Either(&'static [Rule]),
}

/// A kind of text.
#[derive(Debug, Clone, Copy)]
enum Text {
    Any,
    /// `alphanum().length(34)`.
    Address,
    /// `hex().length(n)`, hex digits of either case.
    Hex(usize),
    /// `max(255, "utf8")`.
    Memo,
    /// `ip({version: ["ipv4", "ipv6"]})`, with an optional CIDR suffix.
    Ip,
    /// `/^[a-z0-9!@$&_.]{1,20}$/`.
    Username,
}

const SAFE: i64 = (1 << 53) - 1;
const MIN_0: Rule = Rule::Integer(0, SAFE);
const MIN_1: Rule = Rule::Integer(1, SAFE);
const TRANSFORM: Key = key("transform", Rule::Boolean, Some(Fallback::True));
const PAGE: Key = key("page", MIN_1, Some(Fallback::One));
const OFFSET: Key = key("offset", MIN_0, None);
const LIMIT: Key = key("limit", Rule::Integer(1, 100), Some(Fallback::Hundred));

/// A key of a route's query, its rule and its default.
#[derive(Debug, Clone, Copy)]
struct Key {
    name: &'static str,
    rule: Rule,
    default: Option<Fallback>,
}

const fn key(name: &'static str, rule: Rule, default: Option<Fallback>) -> Key {
    Key {
        name,
        rule,
        default,
    }
}

/// A default of a route's query.
#[derive(Debug, Clone, Copy)]
enum Fallback {
    True,
    One,
    Hundred,
    TransactionOrder,
}

impl Fallback {
    fn value(self) -> Value {
        match self {
            Fallback::True => Value::Bool(true),
            Fallback::One => Value::from(1),
            Fallback::Hundred => Value::from(100),
            Fallback::TransactionOrder => Value::from(vec!["timestamp:desc", "sequence:desc"]),
        }
    }
}

const fn any(rule: &'static Rule) -> Rule {
    Rule::OneOrMore(rule)
}

/// The transaction lists (`/transactions`, a block's and a wallet's transactions), in the order
/// of the reference's schema: the criteria, `orderBy`, `transform`, then the page.
const TRANSACTION_QUERY: [Key; 23] = [
    key("address", any(&Rule::Text(Text::Address)), None),
    key("senderId", any(&Rule::Text(Text::Address)), None),
    key("recipientId", any(&Rule::Text(Text::Address)), None),
    key("id", any(&Rule::Text(Text::Hex(64))), None),
    key("version", any(&MIN_1), None),
    key("blockHeight", any(&Rule::Range(&MIN_1)), None),
    key("blockId", any(&Rule::Text(Text::Hex(64))), None),
    key("sequence", any(&Rule::Range(&MIN_1)), None),
    key("timestamp", any(&Rule::Range(&MIN_0)), None),
    key("nonce", any(&Rule::Range(&MIN_1)), None),
    key("senderPublicKey", any(&Rule::Text(Text::Hex(66))), None),
    key("type", any(&MIN_0), None),
    key("typeGroup", any(&MIN_0), None),
    key("memo", any(&Rule::Text(Text::Memo)), None),
    key("amount", any(&Rule::Range(&MIN_0)), None),
    key("fee", any(&Rule::Range(&MIN_0)), None),
    key("burnedFee", any(&Rule::Range(&MIN_0)), None),
    key("asset", any(&Rule::Object), None),
    key(
        "orderBy",
        Rule::OrderBy(true),
        Some(Fallback::TransactionOrder),
    ),
    TRANSFORM,
    PAGE,
    OFFSET,
    LIMIT,
];

const PEERS_QUERY: [Key; 6] = [
    key("ip", Rule::Text(Text::Ip), None),
    key("version", Rule::Text(Text::Any), None),
    key("orderBy", Rule::OrderBy(false), None),
    PAGE,
    OFFSET,
    LIMIT,
];

const UNCONFIRMED_QUERY: [Key; 4] = [TRANSFORM, PAGE, OFFSET, LIMIT];

const TRANSFORM_QUERY: [Key; 1] = [TRANSFORM];

/// The query schema of `route`, and whether the route paginates; `None` for a route that takes
/// any query.
fn query_schema(route: RouteId) -> Option<(&'static [Key], bool)> {
    match route {
        RouteId::BlocksLast | RouteId::BlocksShow | RouteId::TransactionsShow => {
            Some((&TRANSFORM_QUERY, false))
        }
        RouteId::BlocksTransactions | RouteId::TransactionsIndex | RouteId::WalletsTransactions => {
            Some((&TRANSACTION_QUERY, true))
        }
        RouteId::PeersIndex => Some((&PEERS_QUERY, true)),
        RouteId::TransactionsUnconfirmed => Some((&UNCONFIRMED_QUERY, true)),
        _ => None,
    }
}

/// The query of `route`, validated: defaults added and values coerced.
///
/// A route without a query schema gets the query as it is. On a route that paginates, the result
/// also holds the query as parsed under [`PARSED`]. A query on which the reference's plugins
/// throw ([`parse_query`]'s `null`) is 500 on every route, and so is a query that the schema
/// accepts with a value [`parse_query`] did not write out whole.
pub fn validate_query(route: RouteId, query: &Value) -> Result<Value, ApiError> {
    let Value::Object(input) = query else {
        return Err(ApiError::internal());
    };
    let mut input = Cow::Borrowed(input);
    let beyond = input.contains_key(BEYOND);
    if beyond {
        input.to_mut().shift_remove(BEYOND);
    }
    let Some((schema, paginates)) = query_schema(route) else {
        return Ok(Value::Object(input.into_owned()));
    };
    let mut out = validate_keys(schema, &input)?;
    // Only the `asset` criteria accept a value that deep or that sparse, which the grammar did not
    // write out whole; the reference fails on it past its stack or its strings' length.
    if beyond {
        return Err(ApiError::internal());
    }
    if paginates {
        out.insert(PARSED.to_owned(), Value::Object(input.into_owned()));
    }
    Ok(Value::Object(out))
}

/// Joi's object with keys: each known key validated in place, the missing ones with a default
/// appended in the schema's order, an unknown key refused.
fn validate_keys(
    schema: &[Key],
    input: &Map<String, Value>,
) -> Result<Map<String, Value>, ApiError> {
    let mut out = input.clone();
    for key in schema {
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
    match input
        .keys()
        .find(|name| !schema.iter().any(|k| k.name == *name))
    {
        Some(unknown) => Err(ApiError::unprocessable(format!(
            "\"{unknown}\" is not allowed"
        ))),
        None => Ok(out),
    }
}

fn invalid(name: &str) -> ApiError {
    ApiError::unprocessable(format!(
        "\"{name}\" does not match any of the allowed types"
    ))
}

/// Validate the path parameters of `route`.
pub fn validate_params(route: RouteId, params: &Map<String, Value>) -> Result<(), ApiError> {
    const BLOCK: [Rule; 2] = [Rule::Text(Text::Hex(64)), MIN_1];
    const WALLET: [Rule; 3] = [
        Rule::Text(Text::Address),
        Rule::Text(Text::Hex(66)),
        Rule::Text(Text::Username),
    ];
    let (name, rule) = match route {
        RouteId::BlocksShow => ("id", Rule::Either(&BLOCK)),
        RouteId::BlocksTransactions => ("id", Rule::Text(Text::Any)),
        RouteId::TransactionsShow | RouteId::TransactionsShowUnconfirmed => {
            ("id", Rule::Text(Text::Hex(64)))
        }
        RouteId::PeersShow => ("ip", Rule::Text(Text::Ip)),
        RouteId::WalletsShow | RouteId::WalletsTransactions => ("id", Rule::Either(&WALLET)),
        _ => return Ok(()),
    };
    for (key, value) in params {
        if key != name {
            return Err(ApiError::unprocessable(format!("\"{key}\" is not allowed")));
        }
        check(rule, value).ok_or_else(|| invalid(key))?;
    }
    Ok(())
}

/// The payload of `route` (`POST /transactions`), validated: an object whose only key,
/// `transactions`, is a list of 1 to 40 objects. Other routes take any payload.
pub fn validate_payload(route: RouteId, payload: &Value) -> Result<Value, ApiError> {
    if route != RouteId::TransactionsStore {
        return Ok(payload.clone());
    }
    let Value::Object(map) = payload else {
        return Err(ApiError::unprocessable("\"value\" must be of type object"));
    };
    if let Some(unknown) = map.keys().find(|name| *name != "transactions") {
        return Err(ApiError::unprocessable(format!(
            "\"{unknown}\" is not allowed"
        )));
    }
    let Some(Value::Array(transactions)) = map.get("transactions") else {
        return Err(ApiError::unprocessable("\"transactions\" is required"));
    };
    if transactions.is_empty()
        || transactions.len() > MAX_TRANSACTIONS_PER_REQUEST
        || !transactions.iter().all(Value::is_object)
    {
        return Err(invalid("transactions"));
    }
    Ok(payload.clone())
}

/// The value `rule` makes of `value`, or `None` when it refuses it.
fn check(rule: Rule, value: &Value) -> Option<Value> {
    match (rule, value) {
        (Rule::Boolean, Value::String(text)) => match text.to_lowercase().as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        (Rule::Integer(min, max), Value::String(text)) => {
            let number = joi_number(text)?;
            let accepted = number.trunc() == number
                && number.abs() <= SAFE as f64
                && number >= min as f64
                && number <= max as f64;
            // A safe integer converts exactly.
            accepted.then(|| Value::from(number as i64))
        }
        (Rule::Text(kind), Value::String(text)) => {
            (!text.is_empty() && text_accepts(kind, text)).then(|| value.clone())
        }
        (Rule::OrderBy(known), _) => order_by(known, value).then(|| value.clone()),
        (Rule::OneOrMore(item), Value::Array(items)) => items
            .iter()
            .map(|x| check(*item, x))
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        (Rule::OneOrMore(item), _) => check(*item, value),
        (Rule::Range(bound), Value::Object(map)) => map
            .iter()
            .map(|(name, x)| {
                let valid = (name == "from" || name == "to").then(|| check(*bound, x))??;
                Some((name.clone(), valid))
            })
            .collect::<Option<Map<_, _>>>()
            .map(Value::Object),
        (Rule::Range(bound), _) => check(*bound, value),
        (Rule::Object, Value::Object(_)) => Some(value.clone()),
        (Rule::Either(rules), _) => rules.iter().find_map(|r| check(*r, value)),
        _ => None,
    }
}

fn text_accepts(kind: Text, text: &str) -> bool {
    let hex = |length: usize| text.len() == length && text.bytes().all(|b| b.is_ascii_hexdigit());
    match kind {
        Text::Any => true,
        Text::Address => text.len() == 34 && text.bytes().all(|b| b.is_ascii_alphanumeric()),
        Text::Hex(length) => hex(length),
        Text::Memo => text.len() <= 255,
        Text::Ip => is_ip(text),
        Text::Username => {
            text.len() <= 20
                && text
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"!@$&_.".contains(&b))
        }
    }
}

/// The `orderBy` rule: each entry `property:direction` with 1 to 40 letters, dots or underscores
/// and `asc` or `desc` in any case; on the transaction lists the property must also be a known
/// one, and the direction lowercase.
fn order_by(known: bool, value: &Value) -> bool {
    let entries: Option<Vec<&str>> = match value {
        Value::String(text) => Some(vec![text.as_str()]),
        Value::Array(items) => items.iter().map(Value::as_str).collect(),
        _ => None,
    };
    let Some(entries) = entries else {
        return false;
    };
    entries.iter().all(|entry| {
        let Some((property, direction)) = entry.split_once(':') else {
            return false;
        };
        let pattern = (1..=40).contains(&property.len())
            && property
                .bytes()
                .all(|b| b.is_ascii_alphabetic() || b == b'.' || b == b'_')
            && ["asc", "desc"].contains(&direction.to_ascii_lowercase().as_str());
        pattern && (!known || (is_criterion(property) && ["asc", "desc"].contains(&direction)))
    })
}

/// JavaScript's white space and line terminators (`\s` and `String.prototype.trim`).
fn is_js_space(c: char) -> bool {
    "\t\n\u{b}\u{c}\r \u{a0}\u{1680}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}".contains(c)
        || ('\u{2000}'..='\u{200a}').contains(&c)
}

/// Whether a transaction list sorts by `property`: the name of one of its criteria.
fn is_criterion(property: &str) -> bool {
    TRANSACTION_QUERY
        .iter()
        .take_while(|key| key.name != "orderBy")
        .any(|key| key.name == property)
}

/// Joi's number from text: a decimal with an optional exponent, surrounded by white space at
/// most, refused when the double does not print back as the digits given (`number.unsafe`).
fn joi_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_matches(is_js_space);
    let (mantissa, exponent) = match trimmed.find(['e', 'E']) {
        Some(at) => (trimmed.get(..at)?, Some(trimmed.get(at + 1..)?)),
        None => (trimmed, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let unsigned = mantissa.strip_prefix(['+', '-']).unwrap_or(mantissa);
    let mantissa_ok = match unsigned.split_once('.') {
        Some((whole, fraction)) => {
            (digits(whole) && (fraction.is_empty() || digits(fraction)))
                || (whole.is_empty() && digits(fraction))
        }
        None => digits(unsigned),
    };
    let exponent_ok = exponent.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    if !mantissa_ok || !exponent_ok {
        return None;
    }
    let number: f64 = trimmed.parse().ok()?;
    let number = if number == 0.0 { 0.0 } else { number };
    match exponent {
        Some(e) => {
            let power = 10f64.powf(e.parse::<f64>().ok()?);
            let constructed = format!("{}e{e}", js::number_to_string(number / power));
            (normalize_exponent(&constructed) == normalize_exponent(trimmed)).then_some(number)
        }
        None => {
            let printed = js::number_to_string(number);
            (printed.contains('e') || printed == normalize_decimal(trimmed)).then_some(number)
        }
    }
}

/// Joi's `normalizeExponent`.
fn normalize_exponent(text: &str) -> String {
    let mut s = text.replacen('E', "e", 1);
    if let (Some(dot), Some(e)) = (s.find('.'), s.find('e')) {
        let fraction = s.get(dot + 1..e).unwrap_or("");
        if dot < e && fraction.ends_with('0') && fraction.bytes().all(|b| b.is_ascii_digit()) {
            let kept = fraction.trim_end_matches('0').to_owned();
            s.replace_range(dot + 1..e, &kept);
        }
    }
    let s = s.replacen(".e", "e", 1).replacen("e+", "e", 1);
    let s = s.strip_prefix('+').unwrap_or(&s);
    strip_leading_zeros(s, false)
}

/// Joi's `normalizeDecimal`.
fn normalize_decimal(text: &str) -> String {
    let mut s = text.strip_prefix('+').unwrap_or(text).to_owned();
    if let Some(dot) = s.find('.')
        && s.get(dot + 1..)
            .is_some_and(|rest| rest.bytes().all(|b| b == b'0'))
    {
        s.truncate(dot);
    }
    let (sign, body) = s.split_at(usize::from(s.starts_with('-')));
    if body.starts_with('.') && !body.get(1..).unwrap_or("").contains('.') {
        s = format!("{sign}0{body}");
    }
    let mut s = strip_leading_zeros(&s, true);
    if s.contains('.') && s.ends_with('0') {
        s = s.trim_end_matches('0').to_owned();
    }
    if s == "-0" { "0".to_owned() } else { s }
}

/// `^(-?)0+([1-9])` (or `([0-9])` when `any_digit`) replaced by the sign and the digit.
fn strip_leading_zeros(text: &str, any_digit: bool) -> String {
    let (sign, body) = text.split_at(usize::from(text.starts_with('-')));
    let rest = body.trim_start_matches('0');
    let zeros = body.len() - rest.len();
    if zeros > 0 && rest.starts_with(|c: char| c.is_ascii_digit()) {
        format!("{sign}{rest}")
    } else if zeros > 1 && any_digit {
        // `0+([0-9])` backtracks: the last zero is the digit.
        format!("{sign}0{rest}")
    } else {
        text.to_owned()
    }
}

/// Joi's `ip({version: ["ipv4", "ipv6"]})` with its optional CIDR: RFC 3986's addresses, with up
/// to three digits and leading zeros in an IPv4 octet.
fn is_ip(text: &str) -> bool {
    let (address, cidr) = match text.split_once('/') {
        Some((address, cidr)) => (address, Some(cidr)),
        None => (text, None),
    };
    let prefix = |max: u32, leading_zeros: bool| {
        cidr.is_none_or(|c| {
            (1..=3).contains(&c.len())
                && c.bytes().all(|b| b.is_ascii_digit())
                && (leading_zeros || c.len() == 1 || !c.starts_with('0'))
                && c.parse::<u32>().is_ok_and(|n| n <= max)
        })
    };
    (is_ipv4(address) && prefix(32, false)) || (is_ipv6(address) && prefix(128, true))
}

fn is_ipv4(text: &str) -> bool {
    let octets: Vec<&str> = text.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|o| {
            (1..=3).contains(&o.len())
                && o.bytes().all(|b| b.is_ascii_digit())
                && o.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

fn is_ipv6(text: &str) -> bool {
    let groups = |part: &str, ipv4_last: bool| -> Option<usize> {
        if part.is_empty() {
            return Some(0);
        }
        let pieces: Vec<&str> = part.split(':').collect();
        let mut count = 0;
        for (i, piece) in pieces.iter().enumerate() {
            if ipv4_last && i + 1 == pieces.len() && is_ipv4(piece) {
                count += 2;
            } else if (1..=4).contains(&piece.len()) && piece.bytes().all(|b| b.is_ascii_hexdigit())
            {
                count += 1;
            } else {
                return None;
            }
        }
        Some(count)
    };
    match text.split_once("::") {
        Some((head, tail)) => {
            !tail.contains("::")
                && matches!((groups(head, false), groups(tail, true)), (Some(h), Some(t)) if h + t <= 7)
        }
        None => groups(text, true) == Some(8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_joi() {
        for (text, value) in [
            ("5", 5.0),
            (" 5 ", 5.0),
            ("05", 5.0),
            ("+5", 5.0),
            ("5.0", 5.0),
        ] {
            assert_eq!(joi_number(text), Some(value), "{text}");
        }
        for (text, value) in [("1e3", 1000.0), ("1.5e1", 15.0), ("-0", 0.0), (".5", 0.5)] {
            assert_eq!(joi_number(text), Some(value), "{text}");
        }
        for text in ["", "abc", "0x10", "5e", "9007199254740993", "1,5", "1 000"] {
            assert_eq!(joi_number(text), None, "{text}");
        }
    }

    #[test]
    fn ip_follows_the_reference_grammar() {
        for ok in [
            "192.0.2.1",
            "192.000.002.001",
            "192.0.2.4/24",
            "::",
            "::1",
            "2001:db8::1/064",
            "::ffff:192.0.2.4",
        ] {
            assert!(is_ip(ok), "{ok}");
        }
        for bad in [
            "256.0.2.1",
            "192.0.2",
            "192.0.2.4/33",
            "192.0.2.4/08",
            ":::",
            "1::2::3",
            "1:2:3:4:5:6:7:8:9",
        ] {
            assert!(!is_ip(bad), "{bad}");
        }
    }
}
