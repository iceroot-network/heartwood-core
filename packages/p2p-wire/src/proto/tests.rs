//! The reader's rules, each checked first against the reference's own decoder (the expected values
//! below are what it returned), the writer's, and a round-trip property over every message type.

use super::*;
use crate::errors::DecodeError::{IndexOutOfRange, InvalidWireType};
use crate::messages::Proto;
use crate::messages::blocks::{
    BlockHeader, GetBlocksRequest, GetBlocksResponse, PostBlockRequest, PostBlockResponse,
};
use crate::messages::peer::{
    Common, Config, GetCommonBlocksRequest, GetCommonBlocksResponse, GetPeersRequest,
    GetPeersResponse, GetStatusRequest, GetStatusResponse, Network, Peer, State, StatusHeader,
    Token,
};
use crate::messages::shared::Headers;
use crate::messages::transactions::{
    GetUnconfirmedTransactionsRequest, GetUnconfirmedTransactionsResponse, PostTransactionsRequest,
    PostTransactionsResponse,
};

/// A small deterministic generator for the property tests (xorshift64*).
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self) -> bool {
        self.below(3) != 0
    }
}

/// A random value of a message or a field.
pub(crate) trait Arbitrary {
    fn arbitrary(rng: &mut Rng) -> Self;
}

const TEXTS: [&str; 8] = [
    "",
    "a",
    "4.3.1",
    "@solar-network/api",
    "é€",
    "\u{1F600}",
    "\0\n",
    "0",
];

impl Arbitrary for String {
    fn arbitrary(rng: &mut Rng) -> String {
        let count = rng.below(3);
        (0..count)
            .map(|_| TEXTS[rng.below(TEXTS.len() as u64) as usize])
            .collect()
    }
}

impl Arbitrary for Option<u32> {
    fn arbitrary(rng: &mut Rng) -> Self {
        let value = match rng.below(4) {
            0 => 0,
            1 => u32::MAX,
            2 => rng.below(200) as u32,
            _ => rng.next() as u32,
        };
        rng.chance().then_some(value)
    }
}

impl Arbitrary for Option<bool> {
    fn arbitrary(rng: &mut Rng) -> Self {
        let value = rng.below(2) == 1;
        rng.chance().then_some(value)
    }
}

impl Arbitrary for Option<String> {
    fn arbitrary(rng: &mut Rng) -> Self {
        let value = String::arbitrary(rng);
        rng.chance().then_some(value)
    }
}

impl Arbitrary for Option<Vec<u8>> {
    fn arbitrary(rng: &mut Rng) -> Self {
        let value = (0..rng.below(40)).map(|_| rng.next() as u8).collect();
        rng.chance().then_some(value)
    }
}

impl<T: Message + Arbitrary> Arbitrary for Option<T> {
    fn arbitrary(rng: &mut Rng) -> Self {
        let value = T::arbitrary(rng);
        rng.chance().then_some(value)
    }
}

impl Arbitrary for Vec<String> {
    fn arbitrary(rng: &mut Rng) -> Self {
        (0..rng.below(4)).map(|_| String::arbitrary(rng)).collect()
    }
}

impl<T: Message + Arbitrary> Arbitrary for Vec<T> {
    fn arbitrary(rng: &mut Rng) -> Self {
        (0..rng.below(4)).map(|_| T::arbitrary(rng)).collect()
    }
}

impl Arbitrary for Plugins {
    fn arbitrary(rng: &mut Rng) -> Self {
        const NAMES: [&str; 7] = [
            "b-plugin",
            "1234",
            "7",
            "",
            "__proto__",
            "@solar-network/api",
            "4294967295",
        ];
        let mut plugins = Plugins::new();
        for _ in 0..rng.below(5) {
            let name = NAMES[rng.below(NAMES.len() as u64) as usize].to_owned();
            plugins.insert(name, Arbitrary::arbitrary(rng));
        }
        plugins
    }
}

fn bytes(hex: &str) -> Vec<u8> {
    hex::decode(hex.replace(' ', "")).unwrap()
}

fn headers(version: Option<&str>, port: Option<u32>) -> Headers {
    Headers {
        version: version.map(str::to_owned),
        port,
        ..Headers::default()
    }
}

#[test]
fn uint32_reads_five_bytes_and_skips_five_more_after_a_fifth_with_its_high_bit() {
    let decode = |hex| Headers::decode(&bytes(hex));
    assert_eq!(
        decode("10 ff ff ff ff 8f 01 02 03 04 05"),
        Ok(headers(None, Some(u32::MAX)))
    );
    assert_eq!(
        decode("10 ff ff ff ff 8f 01 02 03 04"),
        Err(IndexOutOfRange)
    );
    assert_eq!(
        decode("10 ff ff ff ff 0f"),
        Ok(headers(None, Some(u32::MAX)))
    );
    // Only the low 4 bits of the fifth byte count.
    assert_eq!(
        decode("10 80 80 80 80 7f"),
        Ok(headers(None, Some(0xf000_0000)))
    );
    assert_eq!(decode("10 80 80"), Err(IndexOutOfRange));
    // A tag is a uint32 too: five bytes, then five skipped.
    assert_eq!(decode("90 80 80 80 00 05"), Ok(headers(None, Some(5))));
    assert_eq!(
        decode("90 80 80 80 80 aa bb cc dd ee 05"),
        Ok(headers(None, Some(5)))
    );
}

#[test]
fn bool_is_a_truncated_uint32_other_than_zero() {
    let forging = |hex| State::decode(&bytes(hex)).map(|state| state.forging_allowed);
    assert_eq!(forging("10 80 80 80 80 10"), Ok(Some(false)));
    assert_eq!(forging("10 02"), Ok(Some(true)));
    assert_eq!(forging("10 00"), Ok(Some(false)));
    assert_eq!(forging(""), Ok(None));
}

#[test]
fn a_known_field_is_read_by_its_declared_type_whatever_its_wire_type() {
    assert_eq!(
        Headers::decode(&bytes("08 01 61")),
        Ok(headers(Some("a"), None))
    );
    assert_eq!(Headers::decode(&bytes("15 05")), Ok(headers(None, Some(5))));
    assert_eq!(
        GetPeersRequest::decode(&bytes("08 02 10 05")).map(|r| r.headers),
        Ok(Some(headers(None, Some(5))))
    );
}

#[test]
fn a_string_is_cut_at_the_end_of_the_payload() {
    assert_eq!(
        Headers::decode(&bytes("0a 05 61")),
        Ok(headers(Some("a"), None))
    );
    assert_eq!(
        GetPeersRequest::decode(&bytes("0a 03 0a 05 61")).map(|r| r.headers),
        Ok(Some(headers(Some("a"), None)))
    );
    // The nested message claims 5 bytes, the string takes the last 3: the next tag is missing.
    assert_eq!(
        GetPeersRequest::decode(&bytes("0a 05 0a 05 61")),
        Err(IndexOutOfRange)
    );
    // A string may run past the end of its nested message into the enclosing one.
    assert_eq!(
        GetPeersRequest::decode(&bytes("0a 04 0a 05 61 62 10 05")).map(|r| r.headers),
        Ok(Some(headers(Some("ab\u{10}\u{5}"), None)))
    );
    // Invalid UTF-8 is replaced, one U+FFFD per maximal invalid subpart.
    assert_eq!(
        Headers::decode(&bytes("0a 02 ff fe")),
        Ok(headers(Some("\u{FFFD}\u{FFFD}"), None))
    );
    assert_eq!(
        Headers::decode(&bytes("0a 03 e2 82 61")),
        Ok(headers(Some("\u{FFFD}a"), None))
    );
}

#[test]
fn bytes_must_all_be_there() {
    let block = |hex| PostBlockRequest::decode(&bytes(hex)).map(|r| r.block);
    assert_eq!(block("0a 05 61"), Err(IndexOutOfRange));
    assert_eq!(block("0a 01 61"), Ok(Some(vec![0x61])));
    assert_eq!(block("0a 00"), Ok(Some(vec![])));
}

#[test]
fn unknown_fields_are_skipped_by_wire_type() {
    let decode = |hex| Headers::decode(&bytes(hex));
    let port = Ok(headers(None, Some(5)));
    assert_eq!(decode("18 ff ff ff ff ff ff ff ff ff ff ff 01 10 05"), port);
    assert_eq!(decode("19 01 02 03 04 05 06 07 08 10 05"), port);
    assert_eq!(decode("1a 02 01 02 10 05"), port);
    assert_eq!(decode("1d 01 02 03 04 10 05"), port);
    assert_eq!(decode("00 00 10 05"), port);
    assert_eq!(decode("18 ff"), Err(IndexOutOfRange));
    assert_eq!(decode("19 01 02 03 04 05 06 07"), Err(IndexOutOfRange));
    assert_eq!(decode("1a 03 01 02"), Err(IndexOutOfRange));
    assert_eq!(decode("1d 01 02 03"), Err(IndexOutOfRange));
}

#[test]
fn groups_are_skipped_to_their_end_at_any_depth() {
    let decode = |hex: &str| Headers::decode(&bytes(hex));
    let port = Ok(headers(None, Some(5)));
    assert_eq!(decode("1b 1b 0c 0c 10 05"), port);
    assert_eq!(
        decode("1b 08 01 11 01 02 03 04 05 06 07 08 1a 01 00 1d 01 02 03 04 0c 10 05"),
        port
    );
    assert_eq!(decode("1b 1b 0c 10 05"), Err(IndexOutOfRange));
    assert_eq!(decode("1b 1e 0c"), Err(InvalidWireType(6)));
    // Deeper than the reference's stack, which overflows there (a documented difference).
    let depth = 50_000;
    let deep = format!("{}{}1005", "1b".repeat(depth), "0c".repeat(depth));
    assert_eq!(decode(&deep), port);
}

#[test]
fn invalid_wire_types_are_refused() {
    for (hex, wire_type) in [("1c", 4), ("1e", 6), ("1f", 7)] {
        assert_eq!(
            Headers::decode(&bytes(hex)),
            Err(InvalidWireType(wire_type)),
            "{hex}"
        );
    }
}

#[test]
fn the_last_occurrence_of_a_field_wins_and_messages_are_not_merged() {
    assert_eq!(
        Headers::decode(&bytes("10 05 10 07")),
        Ok(headers(None, Some(7)))
    );
    assert_eq!(
        GetPeersRequest::decode(&bytes("0a 02 10 05 0a 03 0a 01 61")).map(|r| r.headers),
        Ok(Some(headers(Some("a"), None)))
    );
}

#[test]
fn own_keys_put_repeated_fields_first_then_the_wire_order() {
    let decoded = Headers::decode(&bytes("10 05 0a 01 61 10 07")).unwrap();
    assert_eq!(decoded, headers(Some("a"), Some(7)));
    assert_eq!(decoded.own_keys(), ["port", "version"]);
    assert_eq!(headers(Some("a"), Some(7)).own_keys(), ["version", "port"]);

    let status = GetStatusResponse::decode(&bytes("12 00 0a 00 1a 01 61")).unwrap();
    assert_eq!(
        status.own_keys(),
        ["publicKeys", "signatures", "config", "state"]
    );
    assert_eq!(status.public_keys, ["a"]);
    assert_eq!(
        GetStatusResponse::decode(&[]).unwrap().own_keys(),
        ["publicKeys", "signatures"]
    );
    // A spread that sets a key keeps its position, or appends it.
    let mut request = GetPeersRequest::default();
    request.wire_order.note(1);
    assert!(request.own_keys().is_empty());
}

#[test]
fn plugins_follow_the_own_key_order_of_a_plain_object() {
    let port = |port| {
        Some(Plugin {
            port: Some(port),
            ..Plugin::default()
        })
    };
    let mut plugins = Plugins::new();
    for (name, value) in [
        ("b-plugin", 1),
        ("1234", 2),
        ("a-plugin", 3),
        ("7", 4),
        ("b-plugin", 5),
        ("__proto__", 6),
        // 4294967295 is not an array index, so it keeps its insertion order.
        ("4294967295", 7),
        ("01", 8),
    ] {
        plugins.insert(name.into(), port(value));
    }
    let keys: Vec<(&str, Option<u32>)> = plugins
        .iter()
        .map(|(name, plugin)| (name, plugin.and_then(|p| p.port)))
        .collect();
    assert_eq!(
        keys,
        [
            ("7", Some(4)),
            ("1234", Some(2)),
            ("b-plugin", Some(5)),
            ("a-plugin", Some(3)),
            ("4294967295", Some(7)),
            ("01", Some(8)),
        ]
    );
    assert!(plugins.replaced_prototype());
    assert_eq!(plugins.get("1234"), Some(port(2).as_ref()));
    assert_eq!(plugins.remove("b-plugin"), Some(port(5)));
    assert_eq!(plugins.remove("7"), Some(port(4)));
    assert_eq!(plugins.get("a-plugin"), Some(port(3).as_ref()));
    plugins.retain(|name, _| name != "1234");
    plugins.insert("c".into(), None);
    let names: Vec<&str> = plugins.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["a-plugin", "4294967295", "01", "c"]);
    assert_eq!(plugins.get("c"), Some(None));
    assert_eq!(plugins.len(), 4);
}

#[test]
fn map_entries_are_assigned_as_to_a_plain_object() {
    let decoded = Config::decode(&bytes(
        "1a0e0a08622d706c7567696e12020801 1a0a0a0431323334120208021a0a0a08622d706c7567696e \
         1a00 1a0f0a095f5f70726f746f5f5f12020803 1a050a01371200",
    ))
    .unwrap();
    let port = |port| {
        Some(Plugin {
            port: Some(port),
            ..Plugin::default()
        })
    };
    let entries: Vec<(&str, Option<Plugin>)> = decoded
        .plugins
        .iter()
        .map(|(name, plugin)| (name, plugin.cloned()))
        .collect();
    assert_eq!(
        entries,
        [
            ("7", Some(Plugin::default())),
            ("1234", port(2)),
            ("b-plugin", None),
            ("", None),
        ]
    );
    assert!(decoded.plugins.replaced_prototype());
    assert_eq!(decoded.own_keys(), ["plugins"]);

    let one = |hex| {
        let config = Config::decode(&bytes(hex)).unwrap();
        let entries: Vec<(String, Option<Plugin>)> = config
            .plugins
            .iter()
            .map(|(name, plugin)| (name.to_owned(), plugin.cloned()))
            .collect();
        entries
    };
    // A null prototype makes `__proto__` an ordinary name, which then needs that prototype again
    // in the bytes.
    let proto = hex::encode("__proto__");
    let null_then_own = bytes(&format!(
        "1a0b0a09{proto} 1a0f0a09{proto}12020801 1a050a017812 00"
    ));
    let config = Config::decode(&null_then_own).unwrap();
    let names: Vec<&str> = config.plugins.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["__proto__", "x"]);
    assert!(config.plugins.replaced_prototype());
    assert_eq!(Config::decode(&config.encode()), Ok(config.clone()));
    assert!(hex::encode(config.encode()).starts_with(&format!("1a0b0a09{proto}1a0f")));

    assert_eq!(one("1a 00"), [(String::new(), None)]);
    assert_eq!(one("1a 04 12 02 08 09"), [(String::new(), port(9))]);
    assert_eq!(one("1a 06 18 01 0a 02 61 62"), [("ab".to_owned(), None)]);
}

#[test]
fn the_writer_writes_present_fields_in_field_order() {
    let hex = |message: &dyn Fn() -> Vec<u8>| hex::encode(message());
    assert_eq!(hex(&|| headers(Some(""), Some(0)).encode()), "0a001000");
    assert_eq!(
        hex(&|| headers(None, Some(u32::MAX)).encode()),
        "10ffffffff0f"
    );
    let status = GetStatusResponse {
        state: Some(State::default()),
        config: Some(Config::default()),
        ..GetStatusResponse::default()
    };
    assert_eq!(hex(&|| status.encode()), "0a001200");
    let keys = GetStatusResponse {
        public_keys: vec!["a".into(), String::new()],
        ..GetStatusResponse::default()
    };
    assert_eq!(hex(&|| keys.encode()), "1a01611a00");
    let state = State {
        forging_allowed: Some(true),
        current_slot: Some(-5i32 as u32),
        ..State::default()
    };
    assert_eq!(hex(&|| state.encode()), "100118fbffffff0f");
    // Array-index names first, as the reference's object enumerates them.
    let mut plugins = Plugins::new();
    plugins.insert(
        "b-plugin".into(),
        Some(Plugin {
            port: Some(1),
            ..Plugin::default()
        }),
    );
    plugins.insert(
        "1234".into(),
        Some(Plugin {
            enabled: Some(false),
            ..Plugin::default()
        }),
    );
    let config = Config {
        plugins,
        ..Config::default()
    };
    assert_eq!(
        hex(&|| config.encode()),
        "1a0a0a0431323334120210001a0e0a08622d706c7567696e12020801"
    );
}

#[test]
fn an_entry_without_a_value_is_written_without_it() {
    let mut plugins = Plugins::new();
    plugins.insert("ab".into(), None);
    let config = Config {
        plugins,
        ..Config::default()
    };
    assert_eq!(hex::encode(config.encode()), "1a040a026162");
    assert_eq!(Config::decode(&config.encode()), Ok(config));
}

/// Encode then decode `count` random values of `M`: the same value, with the own keys of a value
/// built in Rust. Then decode random bytes: no panic, and an accepted value round-trips.
fn round_trip<M: Proto + Arbitrary + std::fmt::Debug + PartialEq>(seed: u64, count: usize) {
    let mut rng = Rng::new(seed);
    for _ in 0..count {
        let value = M::arbitrary(&mut rng);
        let decoded = M::decode(&value.encode()).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(decoded.own_keys(), value.own_keys());
        assert_eq!(decoded.encode(), value.encode());
    }
    for _ in 0..count {
        let noise: Vec<u8> = (0..rng.below(24)).map(|_| rng.next() as u8).collect();
        if let Ok(decoded) = M::decode(&noise) {
            assert_eq!(M::decode(&decoded.encode()), Ok(decoded), "{noise:02x?}");
        }
    }
}

#[test]
fn every_message_decodes_what_it_encodes() {
    round_trip::<Headers>(1, 2000);
    round_trip::<GetPeersRequest>(2, 2000);
    round_trip::<GetPeersResponse>(3, 2000);
    round_trip::<Peer>(4, 2000);
    round_trip::<GetCommonBlocksRequest>(5, 2000);
    round_trip::<GetCommonBlocksResponse>(6, 2000);
    round_trip::<Common>(7, 2000);
    round_trip::<GetStatusRequest>(8, 2000);
    round_trip::<GetStatusResponse>(9, 2000);
    round_trip::<State>(10, 2000);
    round_trip::<StatusHeader>(11, 2000);
    round_trip::<Config>(12, 2000);
    round_trip::<Network>(13, 2000);
    round_trip::<Token>(14, 2000);
    round_trip::<Plugin>(15, 2000);
    round_trip::<GetBlocksRequest>(16, 2000);
    round_trip::<GetBlocksResponse>(17, 2000);
    round_trip::<BlockHeader>(18, 2000);
    round_trip::<PostBlockRequest>(19, 2000);
    round_trip::<PostBlockResponse>(20, 2000);
    round_trip::<GetUnconfirmedTransactionsRequest>(21, 2000);
    round_trip::<GetUnconfirmedTransactionsResponse>(22, 2000);
    round_trip::<PostTransactionsRequest>(23, 2000);
    round_trip::<PostTransactionsResponse>(24, 2000);
}
