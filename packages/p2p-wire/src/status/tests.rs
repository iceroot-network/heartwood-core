//! Unit tests of the status attestation. The reference's results quoted here were taken from its
//! kernel `stringify`, its getStatus codec and its reply schema.

use heartwood_crypto::blocks::deserialiser::deserialise_header;
use heartwood_crypto::utils::bytes::Reader;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{KeyPair, SecretKey};

use super::json::{Json, write};
use super::*;
use crate::codecs::Amount;
use crate::errors::ReplyRefused;
use crate::messages::peer::Plugin;

/// The block at height 2 of the reference's status vectors.
const BLOCK: &str = "000000000800000002000000cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd00000000000000000000000000000000000000000095ba0a0000000000000000e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85502dcb90a589d24e2d7e836ccc299f11d3f015ef86e208418d8b3d6de614a02cccdd812e107620705e0ecad4d8cad23327134749dabe83074d54cd8b6ffd1ef3b2086b1ecfdb2cc167981c23a3fbb5ff23eded9f2c50ac24c41ccc48870d1625f23";

/// The reference's preimage of that block at slot 1, forging allowed, version 4.3.1, the devnet
/// network and the API plugin.
const PREIMAGE: &str = concat!(
    r#"{"config":{"network":{"explorer":"","name":"devnet","nethash":"#,
    r#""0000000000000000000000000000000000000000000000000000000000000000","#,
    r#""token":{"name":"dROOT","symbol":"dRT"},"version":90},"plugins":{"@solar-network/api":"#,
    r#"{"enabled":true,"estimateTotalCount":true,"port":6003}},"version":"4.3.1"},"state":"#,
    r#"{"currentSlot":1,"forgingAllowed":true,"header":{"blockSignature":"#,
    r#""d812e107620705e0ecad4d8cad23327134749dabe83074d54cd8b6ffd1ef3b20"#,
    r#"86b1ecfdb2cc167981c23a3fbb5ff23eded9f2c50ac24c41ccc48870d1625f23","#,
    r#""generatorPublicKey":"02dcb90a589d24e2d7e836ccc299f11d3f015ef86e208418d8b3d6de614a02cccd","#,
    r#""height":2,"id":"e5591e2ae5f063b1644206f825fa83e225a59ff81d96d4f01ed71ec0401c03b6","#,
    r#""numberOfTransactions":0,"#,
    r#""payloadHash":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","#,
    r#""payloadLength":0,"previousBlock":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","#,
    r#""reward":"180000000","timestamp":8,"totalAmount":"0","totalFee":"0","version":0},"#,
    r#""height":2}}"#,
);

fn state(current_slot: i64) -> SignedState {
    let bytes = hex::decode(BLOCK).unwrap();
    let header = deserialise_header(&mut Reader::new(&bytes)).unwrap();
    SignedState {
        height: header.height,
        forging_allowed: true,
        current_slot,
        header: StatusHeaderData::from_header(&header),
    }
}

fn plugin(port: u32) -> PluginConfig {
    PluginConfig {
        enabled: true,
        port,
        estimate_total_count: None,
    }
}

fn api() -> (String, PluginConfig) {
    let config = PluginConfig {
        estimate_total_count: Some(true),
        ..plugin(6003)
    };
    ("@solar-network/api".to_owned(), config)
}

fn config(plugins: Vec<(String, PluginConfig)>) -> PeerConfig {
    PeerConfig {
        version: "4.3.1".to_owned(),
        network: NetworkConfig {
            version: 90,
            name: "devnet".to_owned(),
            nethash: "00".repeat(32),
            explorer: String::new(),
            token: TokenConfig {
                name: "dROOT".to_owned(),
                symbol: "dRT".to_owned(),
            },
        },
        plugins,
    }
}

fn key(passphrase: &str) -> KeyPair {
    KeyPair::from_secret_key(SecretKey::from_passphrase(passphrase).unwrap())
}

/// The response a relay sends, as the verifier's decoder gives it back.
fn received(state: &SignedState, config: &PeerConfig, keys: &[&KeyPair]) -> ReceivedStatus {
    let preimage = signing_preimage(state, config);
    let attestations: Vec<Attestation> =
        keys.iter().map(|k| attest(&preimage, k).unwrap()).collect();
    let header = &state.header;
    ReceivedStatus {
        message: response(state, config, &attestations),
        total_amount: Amount::from(header.total_amount),
        total_fee: Amount::from(header.total_fee),
        reward: Amount::from(header.reward),
    }
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

/// `status` as the reply check gives it back; the replies of these tests pass the check.
fn checked(status: ReceivedStatus) -> CheckedStatus {
    check_reply(status, 53).unwrap()
}

/// Whether every attestation of a checked reply verifies against its preimage.
fn verifies(status: &CheckedStatus, local_slot_negative: bool) -> bool {
    let preimage = verifying_preimage(status, local_slot_negative);
    let message = &status.message;
    message.public_keys.len() == message.signatures.len()
        && (message.public_keys.iter().zip(&message.signatures))
            .all(|(k, s)| verify_attestation(&preimage, k, s))
}

#[test]
fn keys_are_sorted_by_utf16_code_units_and_prototype_names_are_left_out() {
    let object = |entries: &[(&str, Json)]| {
        Json::Object(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    };
    let inner = object(&[
        ("toString", object(&[("x", Json::Int(1))])),
        ("valueOf", Json::Int(2)),
        ("", Json::Int(3)),
        ("1234", Json::Int(4)),
        ("é", Json::Int(5)),
        ("\u{1F600}", Json::Int(6)),
        ("\u{FF61}", Json::Int(7)),
        ("toJSON", Json::Int(8)),
        ("__proto__x", Json::Int(9)),
        ("constructor", Json::Null),
    ]);
    let mut out = String::new();
    write(&object(&[("b", Json::Int(1)), ("a", inner)]), &mut out);
    // What the reference's stringify writes for the same object.
    assert_eq!(
        out,
        "{\"a\":{\"\":3,\"1234\":4,\"__proto__x\":9,\"toJSON\":8,\"é\":5,\"\u{1F600}\":6,\"\u{FF61}\":7},\"b\":1}"
    );
}

#[test]
fn the_signing_preimage_is_the_references() {
    assert_eq!(
        text(signing_preimage(&state(1), &config(vec![api()]))),
        PREIMAGE
    );
}

#[test]
fn a_negative_slot_is_signed_negative_and_sent_as_its_32_bit_value() {
    let signer = key("delegate one");
    let (state, config) = (state(-3), config(vec![api()]));
    let status = checked(received(&state, &config, &[&signer]));
    assert!(text(signing_preimage(&state, &config)).contains("\"currentSlot\":-3,"));
    let wire_state = status.message.state.as_ref().unwrap();
    assert_eq!(wire_state.current_slot, Some(4_294_967_293));
    assert!(text(verifying_preimage(&status, false)).contains("\"currentSlot\":4294967293,"));
    assert_eq!(
        verifying_preimage(&status, true),
        signing_preimage(&state, &config)
    );
    assert!(!verifies(&status, false));
    assert!(verifies(&status, true));
    // A slot of 2^31 or more turns negative when the verifier's own slot is.
    let status = checked(received(&self::state(1 << 31), &config, &[&signer]));
    assert!(text(verifying_preimage(&status, true)).contains("\"currentSlot\":-2147483648,"));
    // A slot of 0 stays 0.
    let status = checked(received(&self::state(0), &config, &[&signer]));
    assert!(verifies(&status, true) && verifies(&status, false));
}

#[test]
fn an_empty_plugin_set_is_signed_but_left_out_by_every_verifier() {
    let signer = key("delegate one");
    let (state, config) = (state(5), config(Vec::new()));
    let status = checked(received(&state, &config, &[&signer]));
    assert!(text(signing_preimage(&state, &config)).contains("\"plugins\":{}"));
    assert!(!text(verifying_preimage(&status, false)).contains("plugins"));
    assert!(!verifies(&status, false));
}

#[test]
fn a_key_listed_twice_signs_twice() {
    let signer = key("delegate one");
    let status = checked(received(
        &state(5),
        &config(vec![api()]),
        &[&signer, &signer],
    ));
    let message = &status.message;
    assert_eq!(message.public_keys.len(), 2);
    assert_eq!(message.public_keys[0], message.public_keys[1]);
    assert_eq!(message.signatures[0], message.signatures[1]);
    assert_eq!(
        message.public_keys[0],
        signer.public_key().to_compressed().to_hex()
    );
    assert!(verifies(&status, false));
}

#[test]
fn the_plugins_are_an_object_with_array_index_names_first() {
    let names = [
        "b-plugin",
        "4294967294",
        "1234",
        "4294967295",
        "b-plugin",
        "__proto__",
    ];
    let plugins = names
        .iter()
        .zip(1..)
        .map(|(n, p)| ((*n).to_owned(), plugin(p)))
        .collect();
    let config = config(plugins);
    let status = received(&state(5), &config, &[]);
    let wire = &status.message.config.as_ref().unwrap().plugins;
    let order: Vec<&str> = wire.iter().map(|(name, _)| name).collect();
    assert_eq!(order, ["1234", "4294967294", "b-plugin", "4294967295"]);
    // A repeated name keeps its first place and takes the last value.
    assert_eq!(wire.get("b-plugin").unwrap().unwrap().port, Some(5));
}

#[test]
fn a_prototype_name_is_on_the_wire_but_not_in_the_preimage() {
    let config = config(vec![api(), ("constructor".to_owned(), plugin(5))]);
    let status = received(&state(5), &config, &[]);
    assert!(
        status
            .message
            .config
            .as_ref()
            .unwrap()
            .plugins
            .get("constructor")
            .is_some()
    );
    assert!(!text(signing_preimage(&state(5), &config)).contains("constructor"));
    assert_eq!(
        signing_preimage(&state(5), &config),
        signing_preimage(&state(5), &self::config(vec![api()]))
    );
}

/// A `__proto__` entry without a value makes the prototype `null`; a second one is an own entry,
/// which the reference's `Config.toObject` turns into the prototype of a new object. Its fields
/// that another entry has, and that no entry shadows, are in the reference's preimage.
#[test]
fn an_own_proto_entry_lends_its_fields_to_the_preimage() {
    let plugin = |port, enabled, estimate| Plugin {
        port: Some(port),
        enabled: Some(enabled),
        estimate_total_count: estimate,
        ..Plugin::default()
    };
    let with = |entries: Vec<(&str, Option<Plugin>)>| {
        let (status, result) = check(|status| {
            let plugins = plugins(status);
            *plugins = crate::messages::peer::Plugins::new();
            for (name, value) in entries {
                plugins.insert(name.to_owned(), value);
            }
        });
        assert_eq!(result, Ok(()));
        let preimage = text(verifying_preimage(&status.unwrap(), false));
        let at = preimage.find("\"plugins\":").unwrap();
        let end = at + preimage[at..].find("},\"version\"").unwrap();
        preimage[at..=end].to_owned()
    };
    let proto = "__proto__";
    // The reference's preimages, from its getStatus codec and its kernel `stringify`.
    assert_eq!(
        with(vec![
            ("@solar-network/api", Some(plugin(6003, true, Some(true)))),
            (proto, None),
            (proto, Some(plugin(1, true, Some(false)))),
            ("abcd", Some(plugin(2, true, None))),
        ]),
        concat!(
            r#""plugins":{"@solar-network/api":{"enabled":true,"estimateTotalCount":true,"#,
            r#""port":6003},"abcd":{"enabled":true,"port":2},"enabled":true,"#,
            r#""estimateTotalCount":false,"port":1}"#
        )
    );
    assert_eq!(
        with(vec![
            (proto, None),
            (proto, Some(plugin(1, true, Some(true)))),
            ("port", Some(plugin(2, false, None))),
        ]),
        r#""plugins":{"enabled":true,"port":{"enabled":false,"port":2}}"#
    );
    // A prototype that is a plugin from the start is not an own entry, and lends nothing.
    assert_eq!(
        with(vec![
            (proto, Some(plugin(1, true, Some(true)))),
            ("abcd", Some(plugin(2, true, None))),
        ]),
        r#""plugins":{"abcd":{"enabled":true,"port":2}}"#
    );
}

/// A reply signed by one key, changed by `change`, and checked: the checked reply when the check
/// accepts it, and the check's result.
fn check(
    change: impl FnOnce(&mut ReceivedStatus),
) -> (Option<CheckedStatus>, Result<(), ReplyRefused>) {
    let mut status = received(&state(5), &config(vec![api()]), &[&key("delegate one")]);
    change(&mut status);
    match check_reply(status, 53) {
        Ok(checked) => (Some(checked), Ok(())),
        Err(part) => (None, Err(part)),
    }
}

fn plugins(status: &mut ReceivedStatus) -> &mut crate::messages::peer::Plugins {
    &mut status.message.config.as_mut().unwrap().plugins
}

fn add_plugin(name: &str, port: Option<u32>) -> impl FnOnce(&mut ReceivedStatus) {
    let name = name.to_owned();
    move |status| {
        let value = port.map(|port| Plugin {
            port: Some(port),
            ..Plugin::default()
        });
        plugins(status).insert(name, value);
    }
}

#[test]
fn plugin_names_outside_4_to_64_utf16_units_are_removed_after_the_count() {
    let astral = "\u{1F600}".repeat(32);
    for (name, kept) in [
        ("abc", false),
        ("abcd", true),
        (&*"p".repeat(64), true),
        (&*"p".repeat(65), false),
        ("ab\ncd", false),
        ("ab\rcd", false),
        ("ab\u{2028}cd", false),
        ("ab\u{2029}cd", false),
        ("ab\u{85}cd", true),
        (&*astral, true),
        (&*format!("a{astral}"), false),
        ("", false),
    ] {
        let (status, result) = check(add_plugin(name, Some(1)));
        assert_eq!(result, Ok(()), "{name:?}");
        let mut status = status.unwrap().into_status();
        assert_eq!(plugins(&mut status).get(name).is_some(), kept, "{name:?}");
        assert_eq!(
            plugins(&mut status).len(),
            1 + usize::from(kept),
            "{name:?}"
        );
    }
    // The count is checked before any name is removed.
    let many = |count: u32| {
        move |status: &mut ReceivedStatus| {
            for i in 0..count {
                plugins(status).insert(format!("plugin-{i}"), Some(Plugin::default()));
            }
            plugins(status).insert("abc".to_owned(), Some(Plugin::default()));
        }
    };
    assert_eq!(check(many(30)).1, Ok(()));
    assert_eq!(check(many(31)).1, Err(ReplyRefused::Plugins));
}

#[test]
fn a_remaining_plugin_needs_a_value_and_a_port_that_fits_16_bits() {
    assert_eq!(
        check(add_plugin("abcd", None)).1,
        Err(ReplyRefused::Plugins)
    );
    assert_eq!(
        check(add_plugin("abcd", Some(65_536))).1,
        Err(ReplyRefused::Plugins)
    );
    assert_eq!(check(add_plugin("abcd", Some(65_535))).1, Ok(()));
    // Under a removed name, neither matters.
    assert_eq!(check(add_plugin("abc", None)).1, Ok(()));
    assert_eq!(check(add_plugin("abc", Some(65_536))).1, Ok(()));
    // A removed name changes the signed bytes back to what the signer signed.
    let (status, result) = check(add_plugin("xyz", Some(1)));
    assert!(result.is_ok() && status.is_some_and(|status| verifies(&status, false)));
}

fn header(status: &mut ReceivedStatus) -> &mut crate::messages::peer::StatusHeader {
    status
        .message
        .state
        .as_mut()
        .unwrap()
        .header
        .as_mut()
        .unwrap()
}

#[test]
fn the_generator_key_is_lowercased_and_still_verifies() {
    let (status, result) = check(|status| {
        let key = header(status).generator_public_key.as_mut().unwrap();
        *key = key.to_uppercase();
    });
    assert_eq!(result, Ok(()));
    let status = status.unwrap();
    let key = header(&mut status.clone().into_status())
        .generator_public_key
        .clone()
        .unwrap();
    assert_eq!(key, key.to_lowercase());
    assert!(verifies(&status, false));
    // The block id is not lowercased: the signed bytes change.
    let (status, result) = check(|status| {
        let id = header(status).id.as_mut().unwrap();
        *id = id.to_uppercase();
    });
    assert!(result.is_ok() && status.is_some_and(|status| !verifies(&status, false)));
}

/// A change to a reply.
type Change = Box<dyn FnOnce(&mut ReceivedStatus)>;

#[test]
fn the_first_rule_that_fails_names_its_part() {
    let cases: [(ReplyRefused, Change); 11] = [
        (ReplyRefused::Root, Box::new(|s| s.message.config = None)),
        (
            ReplyRefused::State,
            Box::new(|s| s.message.state.as_mut().unwrap().height = Some(0)),
        ),
        (
            ReplyRefused::State,
            Box::new(|s| s.message.state.as_mut().unwrap().current_slot = None),
        ),
        (
            ReplyRefused::Header,
            Box::new(|s| header(s).block_signature = Some("ab".into())),
        ),
        (
            ReplyRefused::Config,
            Box::new(|s| s.message.config.as_mut().unwrap().version = None),
        ),
        (
            ReplyRefused::Network,
            Box::new(|s| s.message.config.as_mut().unwrap().network = None),
        ),
        (
            ReplyRefused::PublicKeys,
            Box::new(|s| s.message.public_keys = vec!["0".repeat(64)]),
        ),
        (
            ReplyRefused::Signatures,
            Box::new(|s| s.message.signatures = vec!["zz".repeat(64)]),
        ),
        // A required key with a schema of its own is checked where its schema is: a missing
        // configuration is found after the state and its header.
        (
            ReplyRefused::Header,
            Box::new(|s| {
                header(s).id = None;
                s.message.config = None;
            }),
        ),
        (
            ReplyRefused::State,
            Box::new(|s| {
                s.message.state.as_mut().unwrap().forging_allowed = None;
                s.message.config = None;
            }),
        ),
        // A header and a configuration that both fail: the header is checked first.
        (
            ReplyRefused::Header,
            Box::new(|s| {
                header(s).id = Some("x".into());
                s.message.config.as_mut().unwrap().version = None;
            }),
        ),
    ];
    for (index, (part, change)) in cases.into_iter().enumerate() {
        assert_eq!(check(change).1, Err(part), "case {index}");
    }
    let (_, result) = check(|s| s.message.public_keys = vec!["0".repeat(66); 54]);
    assert_eq!(result, Err(ReplyRefused::PublicKeys));
}

#[test]
fn lengths_count_code_points_and_absent_fields_read_as_defaults() {
    let version = |text: String| {
        move |s: &mut ReceivedStatus| s.message.config.as_mut().unwrap().version = Some(text)
    };
    assert_eq!(
        check(version(format!("\u{1F600}{}", "x".repeat(23)))).1,
        Ok(())
    );
    assert_eq!(check(version("x".repeat(25))).1, Err(ReplyRefused::Config));
    let network = |change: fn(&mut crate::messages::peer::Network)| {
        move |s: &mut ReceivedStatus| {
            change(s.message.config.as_mut().unwrap().network.as_mut().unwrap())
        }
    };
    assert_eq!(check(network(|n| n.explorer = None)).1, Ok(()));
    assert_eq!(check(network(|n| n.version = None)).1, Ok(()));
    assert_eq!(
        check(network(|n| n.version = Some(256))).1,
        Err(ReplyRefused::Network)
    );
    assert_eq!(
        check(network(|n| n.name = None)).1,
        Err(ReplyRefused::Network)
    );
    assert_eq!(
        check(network(|n| n.nethash = Some("AB".repeat(32)))).1,
        Ok(())
    );
    assert_eq!(
        check(network(|n| n.token = None)).1,
        Err(ReplyRefused::Network)
    );
}

#[test]
fn a_header_at_height_1_may_have_no_previous_block_and_unbounded_totals() {
    let at_height_1 = |previous: &str| {
        let previous = previous.to_owned();
        move |s: &mut ReceivedStatus| {
            header(s).height = Some(1);
            header(s).previous_block = Some(previous);
        }
    };
    for previous in [
        "",
        "0x0",
        " 0 ",
        "0e5",
        ".0",
        "-0",
        "1e-400",
        "\u{A0}0\u{FEFF}",
    ] {
        assert_eq!(check(at_height_1(previous)).1, Ok(()), "{previous:?}");
    }
    for previous in ["1e-300", "x", "-0x0", "0\u{85}"] {
        assert_eq!(
            check(at_height_1(previous)).1,
            Err(ReplyRefused::Header),
            "{previous:?}"
        );
    }
    assert_eq!(
        check(|s| header(s).previous_block = Some(String::new())).1,
        Err(ReplyRefused::Header)
    );
    let big = Amount::new(1 << 63);
    let totals = move |s: &mut ReceivedStatus| {
        header(s).height = Some(1);
        s.total_amount = big;
        s.total_fee = big;
    };
    assert_eq!(check(totals).1, Ok(()));
    assert_eq!(check(|s| s.total_fee = big).1, Err(ReplyRefused::Header));
    let reward = move |s: &mut ReceivedStatus| {
        header(s).height = Some(1);
        s.reward = big;
    };
    assert_eq!(check(reward).1, Err(ReplyRefused::Header));
    assert_eq!(
        check(|s| s.total_fee = Amount::new(-1)).1,
        Err(ReplyRefused::Header)
    );
}

#[test]
fn a_session_proof_is_a_fixed_prefix_and_a_challenge_of_64_hex_digits() {
    let challenge = "0123456789abcdef".repeat(4);
    let proof = session_proof(&challenge).unwrap();
    assert_eq!(
        proof,
        format!("Heartwood attestation session {challenge}").into_bytes()
    );
    assert!(parse_preimage(&proof).is_err());
    for other in [
        &challenge[1..],
        &challenge.to_uppercase(),
        &format!("{challenge}0"),
        "",
    ] {
        assert_eq!(session_proof(other), None, "{other}");
    }
    // A proof signs and verifies as an attestation of its own text only.
    let signed = attest(&proof, &key("proof")).unwrap();
    let public = key("proof").public_key().to_hex();
    let signature = signed.signature.to_hex();
    assert!(verify_attestation(&proof, &public, &signature));
    assert!(!verify_attestation(
        PREIMAGE.as_bytes(),
        &public,
        &signature
    ));
}
