//! The internal bodies: the reference's byte forms, its readers' edge cases, the quorum's
//! arithmetic against the reference's floating-point formula, and the V29 records of the
//! reference's own codecs.
//!
//! The V29 file is read from `tests/vectors`, or from the directory in `WIRE_VECTORS` (a relative
//! path is taken from the workspace root), such as a local oracle run in `target/vectors/local`.
//! A class that is not installed is reported and the test passes.

use std::path::Path;

use serde_json::json;

use super::*;

fn wallet(username: &str, key: &str) -> Value {
    json!({
        "address": "D6Z26L69gdk9qYmTv5uzk3uGepigtHY4ax",
        "publicKey": key,
        "balance": "100",
        "nonce": "1",
        "attributes": {"knownAttributes": {}, "attributes": {"delegate": {"username": username}}},
        "delegate": {"username": username, "voteBalance": "5", "rank": 1},
    })
}

fn round_body() -> Value {
    json!({
        "allDelegates": [wallet("genesis_1", "02aa"), wallet("genesis_2", "03bb")],
        "canForge": true,
        "current": 2,
        "currentForger": wallet("genesis_2", "03bb"),
        "delegates": [wallet("genesis_2", "03bb"), wallet("genesis_1", "02aa")],
        "lastBlock": {
            "version": 0, "timestamp": 64, "height": 60, "previousBlock": "ab",
            "numberOfTransactions": 0, "totalAmount": "0", "totalFee": "0", "reward": "0",
            "payloadLength": 0, "payloadHash": "e3", "generatorPublicKey": "02aa",
            "blockSignature": "cd", "id": "ef", "burnedFee": "0", "donations": {},
            "username": "genesis_1",
        },
        "reward": "500000000",
        "timestamp": 72,
    })
}

#[test]
fn a_round_is_read_as_the_forger_reads_it_and_written_back() {
    let body = js::stringify(&round_body(), 0);
    let round: CurrentRound = decode(body.as_bytes()).unwrap();
    assert_eq!(
        (round.current, round.timestamp, round.last_block.height),
        (2, 72, 60)
    );
    assert_eq!(round.reward, "500000000");
    assert_eq!(round.current_forger.as_ref().unwrap().public_key, "03bb");
    assert!(round.next_forger.is_none(), "no delegate at the next index");
    let names: Vec<&str> = (round.delegates.iter())
        .map(|w| w.username.as_str())
        .collect();
    assert_eq!(names, ["genesis_2", "genesis_1"]);
    assert_eq!(round.delegates[0].fields["delegate"]["voteBalance"], "5");
    assert_eq!(
        round.last_block.fields["username"].as_str(),
        Some("genesis_1")
    );
    // Every field is written back as it came, the wallets' attribute maps included.
    assert_eq!(encode(&round), js::stringify(&round_body(), 0).into_bytes());
    assert_eq!(decode::<CurrentRound>(&encode(&round)).unwrap(), round);
}

#[test]
fn a_round_without_a_field_the_forger_reads_is_refused() {
    for (path, value) in [
        ("/timestamp", json!(null)),
        ("/timestamp", json!(1.5)),
        ("/reward", json!(5)),
        ("/lastBlock/height", json!("60")),
        ("/delegates", json!({})),
        ("/allDelegates/1/publicKey", json!(null)),
        ("/currentForger/delegate/username", json!(7)),
        ("/delegates/0/delegate", json!(null)),
    ] {
        let mut body = round_body();
        *body.pointer_mut(path).unwrap() = value;
        let error = CurrentRound::from_json(&body).unwrap_err();
        assert_eq!(error.js_class(), "TypeError", "{path}: {error}");
    }
    // A field no forger reads is kept as it came, or left empty when it has another type.
    let mut body = round_body();
    body["lastBlock"]["version"] = json!("zero");
    body["current"] = json!(-1);
    let round = CurrentRound::from_json(&body).unwrap();
    assert_eq!(
        (&round.last_block.fields["version"], round.current),
        (&json!("zero"), 0)
    );
}

fn full_state() -> NetworkState {
    let mut headers = Map::new();
    headers.insert("192.0.2.7".to_owned(), json!({"id": "aa", "height": 61}));
    NetworkState {
        status: NetworkStateStatus::Default,
        node_height: Some(60),
        last_block_id: Some("ef".into()),
        last_generator: Some("genesis_1".into()),
        last_slot_number: Some(9),
        quorum_details: QuorumDetails {
            has_quorum: 30,
            no_quorum: 2,
            over_height: 1,
            over_height_block_headers: headers,
            forked: 1,
            different_slot: 3,
            forging_not_allowed: 4,
            delegates: QuorumDelegates {
                has_quorum: vec!["genesis_1".into()],
                no_quorum: vec!["genesis_2".into()],
                no_response: vec![],
            },
        },
    }
}

#[test]
fn a_network_state_has_the_reference_byte_form() {
    let bytes = encode(&full_state());
    let expected = concat!(
        r#"{"status":0,"quorumDetails":{"hasQuorum":30,"noQuorum":2,"overHeight":1,"#,
        r#""overHeightBlockHeaders":{"192.0.2.7":{"id":"aa","height":61}},"forked":1,"#,
        r#""differentSlot":3,"forgingNotAllowed":4,"delegates":{"hasQuorum":["genesis_1"],"#,
        r#""noQuorum":["genesis_2"],"noResponse":[]}},"nodeHeight":60,"lastBlockId":"ef","#,
        r#""lastGenerator":"genesis_1","lastSlotNumber":9}"#,
    );
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected);
    assert_eq!(decode::<NetworkState>(&bytes).unwrap(), full_state());
    let below = NetworkState {
        status: NetworkStateStatus::BelowMinimumDelegates,
        last_generator: None,
        ..NetworkState::unknown()
    };
    let text = String::from_utf8(encode(&below)).unwrap();
    assert!(
        text.starts_with(r#"{"status":1,"quorumDetails":{"hasQuorum":0,"#),
        "{text}"
    );
    assert!(
        text.ends_with(r#""noResponse":[]}}}"#),
        "no tip field is written: {text}"
    );
}

#[test]
fn network_states_are_read_as_network_state_parse_reads_them() {
    for body in [
        "null",
        "0",
        "false",
        "\"\"",
        "[]",
        "\"state\"",
        "{}",
        r#"{"nodeHeight":5}"#,
    ] {
        assert_eq!(
            decode::<NetworkState>(body.as_bytes()).unwrap(),
            NetworkState::unknown(),
            "{body}"
        );
    }
    let state: NetworkState =
        decode(br#"{"status":3,"nodeHeight":null,"quorumDetails":{"noQuorum":4}}"#).unwrap();
    assert_eq!(state.status, NetworkStateStatus::ColdStart);
    assert_eq!(
        (state.node_height, state.quorum_details.no_quorum),
        (None, 4)
    );
    for body in [
        r#"{"status":5}"#,
        r#"{"status":null}"#,
        r#"{"status":"0"}"#,
        r#"{"status":0,"nodeHeight":-1}"#,
        r#"{"status":0,"lastSlotNumber":1.5}"#,
        r#"{"status":0,"quorumDetails":7}"#,
        r#"{"status":0,"quorumDetails":{"hasQuorum":4294967296}}"#,
        r#"{"status":0,"quorumDetails":{"overHeightBlockHeaders":[]}}"#,
        r#"{"status":0,"quorumDetails":{"delegates":{"hasQuorum":[1]}}}"#,
    ] {
        let error = decode::<NetworkState>(body.as_bytes()).unwrap_err();
        assert_eq!(error.js_class(), "TypeError", "{body}");
    }
    let error = decode::<NetworkState>(b"{status: 0}").unwrap_err();
    assert_eq!(error.js_class(), "SyntaxError");
    // Node reads a malformed UTF-8 sequence as U+FFFD, inside a string as elsewhere.
    let state: NetworkState = decode(b"{\"status\":0,\"lastBlockId\":\"a\xffb\"}").unwrap();
    assert_eq!(state.last_block_id.as_deref(), Some("a\u{fffd}b"));
}

/// The reference's `canForge`, in doubles.
fn reference_can_forge(has: u32, no: u32, active: u32) -> bool {
    let (has, total) = (f64::from(has), f64::from(has) + f64::from(no));
    let quorum = if (has / total).is_finite() {
        has / total
    } else {
        0.0
    };
    let threshold = f64::min(1.0, f64::from(active / 2 + 1) / total);
    quorum >= threshold
}

#[test]
fn the_quorum_forges_as_the_reference_for_every_count() {
    for active in [1, 2, 3, 51, 52, 53, 54, 100] {
        for total in 0..=120_u32 {
            for has in 0..=total {
                let details = QuorumDetails {
                    has_quorum: has,
                    no_quorum: total - has,
                    ..QuorumDetails::default()
                };
                assert_eq!(
                    details.can_forge(active),
                    reference_can_forge(has, total - has, active),
                    "{has} of {total} with {active} active"
                );
            }
        }
    }
    let details = |has, no| QuorumDetails {
        has_quorum: has,
        no_quorum: no,
        ..QuorumDetails::default()
    };
    assert_eq!(details(0, 0).quorum(), 0.0);
    assert_eq!(details(27, 26).quorum(), 27.0 / 53.0);
    assert!(details(27, 26).can_forge(53) && !details(26, 27).can_forge(53));
    assert!(
        details(3, 0).can_forge(53),
        "all of them when fewer than 27 are counted"
    );
    assert!(!details(2, 1).can_forge(53));
    assert!(details(u32::MAX, u32::MAX).can_forge(u32::MAX));
    let test = NetworkState {
        status: NetworkStateStatus::Test,
        ..NetworkState::unknown()
    };
    assert!(
        test.can_forge(53) && test.quorum() == 1.0,
        "the test status always forges"
    );
    assert!(!NetworkState::unknown().can_forge(53));
}

#[test]
fn the_small_bodies_round_trip_in_the_reference_forms() {
    fn both<T: InternalBody + PartialEq + std::fmt::Debug>(body: T, text: &str) {
        assert_eq!(String::from_utf8(encode(&body)).unwrap(), text);
        assert_eq!(decode::<T>(text.as_bytes()).unwrap(), body);
    }
    both(SlotNumberRequest { timestamp: None }, "{}");
    both(
        SlotNumberRequest {
            timestamp: Some(-8),
        },
        r#"{"timestamp":-8}"#,
    );
    both(SlotNumber(123), "123");
    both(NetworkStateRequest { log: true }, r#"{"log":true}"#);
    both(
        EmitEvent {
            event: "block.forged".into(),
            body: json!({"height": 2}),
        },
        r#"{"event":"block.forged","body":{"height":2}}"#,
    );
    both(
        StatusToSignRequest {
            public_keys: vec!["02aa".into(), "02aa".into()],
        },
        r#"{"publicKeys":["02aa","02aa"]}"#,
    );
    both(
        StatusToSignResponse::Offer {
            status: r#"{"config":{}}"#.into(),
            public_keys: vec!["02aa".into()],
        },
        r#"{"status":"{\"config\":{}}","publicKeys":["02aa"]}"#,
    );
    both(StatusToSignResponse::Unchanged, r#"{"unchanged":true}"#);
    both(
        StatusSignatures {
            offer: "ff".into(),
            signatures: vec!["00".into()],
        },
        r#"{"offer":"ff","signatures":["00"]}"#,
    );
    assert_eq!(decode::<SlotNumber>(b"12.0").unwrap(), SlotNumber(12));
    assert!(decode::<NetworkStateRequest>(b"{\"log\":1}").unwrap().log);
    assert!(!decode::<NetworkStateRequest>(b"{}").unwrap().log);
    assert_eq!(
        decode::<SlotNumberRequest>(b"{\"timestamp\":null}")
            .unwrap()
            .timestamp,
        None
    );
    let event: EmitEvent = decode(br#"{"event":"forger.failed"}"#).unwrap();
    assert_eq!(event.body, Value::Null);
    for (bytes, route) in [
        (&b"12.5"[..], RouteId::GetSlotNumber),
        (b"\"12\"", RouteId::GetSlotNumber),
        (b"9007199254740992", RouteId::GetSlotNumber),
        (b"null", RouteId::GetNetworkState),
        (br#"{"unchanged":false}"#, RouteId::GetStatusToSign),
        (br#"{"status":"x"}"#, RouteId::GetStatusToSign),
        (
            br#"{"offer":"ff","signatures":"00"}"#,
            RouteId::PostStatusSignatures,
        ),
        (br#"{"body":{}}"#, RouteId::EmitEvent),
    ] {
        let refused = match route {
            RouteId::GetSlotNumber => decode::<SlotNumber>(bytes).is_err(),
            RouteId::GetNetworkState => decode::<NetworkStateRequest>(bytes).is_err(),
            RouteId::GetStatusToSign => decode::<StatusToSignResponse>(bytes).is_err(),
            RouteId::PostStatusSignatures => decode::<StatusSignatures>(bytes).is_err(),
            _ => decode::<EmitEvent>(bytes).is_err(),
        };
        assert!(refused, "{} was read", String::from_utf8_lossy(bytes));
    }
    assert_eq!(parse(b" true ").unwrap(), Value::Bool(true));
}

/// The V29 records of the internal bodies, or `None` when the class is not installed.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::var_os("WIRE_VECTORS").map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let text = std::fs::read_to_string(dir.join("V29-forging.jsonl")).ok()?;
    let records = text
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    Some(
        records
            .filter(|r| {
                r["op"]
                    .as_str()
                    .is_some_and(|op| op.starts_with("internal."))
            })
            .collect(),
    )
}

/// Each record: the relay's bytes as the reference writes them, and the forger's reading of them.
/// A network state built from the reading is written to the same bytes; a round is read to the
/// same fields, and written and read again to the same round.
#[test]
fn the_internal_bodies_agree_with_the_reference_records() {
    let Some(records) = records() else {
        eprintln!(
            "V29-forging is not installed (make vectors-install, or WIRE_VECTORS=<a vector run>)"
        );
        return;
    };
    assert!(records.iter().any(|r| r["op"] == "internal.networkState"));
    assert!(records.iter().any(|r| r["op"] == "internal.currentRound"));
    for record in &records {
        let name = &record["name"];
        let output = &record["output"];
        let bytes = output["json"].as_str().unwrap().as_bytes();
        let read = &output["read"];
        if record["op"] == "internal.networkState" {
            let state: NetworkState = decode(bytes).unwrap();
            let active = u32::try_from(output["activeDelegates"].as_u64().unwrap()).unwrap();
            assert_eq!(json!(state.status as u8), read["status"], "{name}");
            assert_eq!(json!(state.node_height), read["nodeHeight"], "{name}");
            assert_eq!(json!(state.last_block_id), read["lastBlockId"], "{name}");
            assert_eq!(json!(state.last_generator), read["lastGenerator"], "{name}");
            assert_eq!(
                json!(state.last_slot_number),
                read["lastSlotNumber"],
                "{name}"
            );
            assert_eq!(read["quorum"].as_f64(), Some(state.quorum()), "{name}");
            assert_eq!(json!(state.can_forge(active)), read["canForge"], "{name}");
            let headers: Vec<&Value> = state
                .quorum_details
                .over_height_block_headers
                .values()
                .collect();
            assert_eq!(json!(headers), read["overHeightBlockHeaders"], "{name}");
            if state.status != NetworkStateStatus::Unknown {
                assert_eq!(encode(&state), bytes, "{name}: the relay's bytes");
            }
        } else {
            let round: CurrentRound = decode(bytes).unwrap();
            let names = |list: &[DelegateWallet]| -> Value {
                list.iter()
                    .map(|w| json!([w.public_key, w.username]))
                    .collect()
            };
            let reading = json!({
                "timestamp": round.timestamp,
                "reward": round.reward,
                "lastBlockHeight": round.last_block.height,
                "currentForger": round.current_forger.as_ref().map(|w| &w.public_key),
                "nextForger": round.next_forger.as_ref().map(|w| &w.public_key),
                "delegates": names(&round.delegates),
                "allDelegates": names(&round.all_delegates),
            });
            assert_eq!(&reading, read, "{name}");
            assert_eq!(
                decode::<CurrentRound>(&encode(&round)).unwrap(),
                round,
                "{name}"
            );
        }
    }
}
