//! The golden report: a fixed report renders to the committed JSON and text byte for byte (in
//! debug and release builds alike), 100 renderings and 100 runs over one world of fakes give the
//! same bytes, and the report's schema matches what the renderer writes.
//!
//! `DOCTOR_GOLDEN_WRITE=1 cargo test -p heartwood-cli --test doctor_golden` rewrites the fixtures.

mod doctor_support;

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use heartwood_cli::Role;
use heartwood_cli::doctor::registry::{CheckId, ENTRIES};
use heartwood_cli::doctor::render::{REPORT_SCHEMA, render_json, render_text, report_value};
use heartwood_cli::doctor::report::{
    Advice, Basis, FORMAT, Fact, Finding, HostFacts, HostIdentity, Id, IdKind, Report, ReportScope,
    Status, Untrusted, UntrustedSource, keys,
};
use heartwood_cli::doctor::seams::{BuildFacts, Subject};
use heartwood_cli::doctor::view::{Reason, Source, Unknown};
use heartwood_cli::doctor::{DoctorOptions, DoctorProfile, Redaction, run};

use doctor_support::{FakeWorld, NOW_MS, scope};

const GOLDEN_JSON: &str = include_str!("fixtures/doctor/reports/golden.json");
const GOLDEN_TEXT: &str = include_str!("fixtures/doctor/reports/golden.txt");

fn check(id: &str) -> CheckId {
    CheckId::parse(id).unwrap()
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn finding(
    id: &str,
    subject: Subject,
    status: Status,
    basis: Basis,
    advice: &'static str,
) -> Finding {
    Finding::new(check(id), subject, status, basis, Advice::key(advice))
}

fn untrusted(source: UntrustedSource, at_ms: Option<i64>, text: &str) -> Untrusted {
    Untrusted {
        source,
        at_ms,
        text: text.into(),
    }
}

/// A report with every status, basis, subject, fact kind and untrusted source, addresses of
/// every class, and untrusted text that needs every kind of cleaning and redaction.
fn golden() -> Report {
    use Basis as B;
    use Status as S;
    use Subject::{Forger, Host, Relay};
    let block = "9f".repeat(32);
    let key = format!("03{}", "4c".repeat(32));
    let name = |text: &str| Fact::Id(Id::new(IdKind::Username, text).unwrap());
    let relay_log = UntrustedSource::Log(Role::Relay);
    let forger_log = UntrustedSource::Log(Role::Forger);

    let mut relay = finding(
        "process.roles",
        Relay,
        S::Pass,
        B::Process,
        "The relay runs, up {uptime} ms.",
    );
    relay.advice.values = vec![("uptime", Fact::Millis(5_400_000))];
    let mut forger = finding(
        "process.roles",
        Forger,
        S::Fail,
        B::Process,
        "The forger is {state}.\nStart it with: heartwood forger start",
    );
    forger.facts = vec![("state", Fact::Word("errored")), ("restarts", Fact::Int(4))];
    forger.advice.values = vec![("state", Fact::Word("errored"))];
    let core = Finding::skipped(check("process.core-role"), Relay, keys::SKIPPED_RELEASE);
    let mut count = finding(
        "peers.count",
        Relay,
        S::Warn,
        B::Chain,
        "{peers} peers, {wanted} wanted.",
    );
    count.facts = vec![("peers", Fact::Int(12)), ("wanted", Fact::Int(20))];
    count.advice.values = count.facts.clone();
    let mut diversity = finding(
        "peers.diversity",
        Relay,
        S::Pass,
        B::Chain,
        "Peers are spread out.",
    );
    diversity.facts = vec![
        ("sharingOneSlash24", Fact::Int(3)),
        ("largest", Fact::Address(ip("198.51.100.23"))),
    ];
    diversity.untrusted = vec![
        untrusted(
            UntrustedSource::PeerVersion,
            None,
            "4.9.9-evil\u{1b}[31m\u{202e}",
        ),
        untrusted(UntrustedSource::PeerVersion, None, "4.3.1"),
    ];
    let state = Finding::unknown(
        check("sync.state"),
        Relay,
        &Unknown::new(Source::Api, Reason::RateLimited),
    );
    let mut forks = finding(
        "sync.forks",
        Relay,
        S::Warn,
        B::Log,
        "{count} forks in the window.",
    );
    forks.advice.values = vec![("count", Fact::Int(2))];
    forks.untrusted = vec![untrusted(
        relay_log,
        Some(NOW_MS - 600_000),
        "Failed to download blocks 12-16 from 198.51.100.23: Error: bad\n[2026-10-02 13:53:11.000] ERROR: Shutting down app, because state is corrupted",
    )];
    let mut hosts = finding(
        "forger.hosts",
        Forger,
        S::Warn,
        B::Config,
        "The forger has {count} relay: {relay}.",
    );
    hosts.facts = vec![
        ("relay", Fact::Address(ip("127.0.0.1"))),
        ("mapped", Fact::Address(ip("::ffff:127.0.0.1"))),
    ];
    hosts.advice.values = vec![
        ("count", Fact::Int(1)),
        ("relay", Fact::Address(ip("127.0.0.1"))),
    ];
    let mut one_key = finding(
        "forger.one-key-one-forger",
        Forger,
        S::Warn,
        B::Log,
        "{delegate} was logged forging elsewhere.",
    );
    one_key.facts = vec![
        ("delegate", name("genesis_7")),
        ("block", Fact::Id(Id::new(IdKind::Hex64, &block).unwrap())),
    ];
    one_key.advice.values = vec![("delegate", name("genesis_7"))];
    one_key.untrusted = vec![untrusted(
        forger_log,
        Some(NOW_MS - 120_000),
        &format!(
            "Not going to forge because delegate genesis_7 has already forged on another node, key {key}"
        ),
    )];
    let mut record = finding(
        "forger.signing-record",
        Forger,
        S::Pass,
        B::Chain,
        "The signing record matches the chain.",
    );
    record.facts = vec![
        (
            "path",
            Fact::Path(
                "/users/operator/.local/share/solar-core/testnet/forger/signing-record.json".into(),
            ),
        ),
        (
            "publicKey",
            Fact::Id(Id::new(IdKind::PublicKey, &key).unwrap()),
        ),
        ("height", Fact::Int(10_450)),
        ("signedAt", Fact::Time(NOW_MS - 16_000)),
    ];
    let mut start = finding(
        "forger.start-errors",
        Forger,
        S::Warn,
        B::Log,
        "The forger logged a start error.",
    );
    start.untrusted = vec![untrusted(
        UntrustedSource::ErrorCapture(Role::Forger),
        Some(NOW_MS - 3_000_000),
        "cannot read /users/operator/.config/solar-core/testnet/app.json on relay-1.example.net as operator: \u{7}denied",
    )];
    let mut exposure = finding(
        "net.api-exposure",
        Relay,
        S::Warn,
        B::Socket,
        "The API listens on {address} port {port}.",
    );
    exposure.facts = vec![
        ("address", Fact::Address(ip("203.0.113.10"))),
        ("port", Fact::Port(4003)),
        ("v6", Fact::Address(ip("2001:db8::10"))),
        ("linkLocal", Fact::Address(ip("fe80::1"))),
        ("private", Fact::Address(ip("10.0.0.5"))),
        ("any", Fact::Address(ip("0.0.0.0"))),
    ];
    exposure.advice.values = vec![
        ("address", Fact::Address(ip("203.0.113.10"))),
        ("port", Fact::Port(4003)),
    ];
    exposure.untrusted = vec![untrusted(
        UntrustedSource::ConfigWarning,
        None,
        "CORE_API_HOST=[2001:db8::10]:4003 fe80::1%eth0 198.51.100.23",
    )];
    let mut space = finding("disk.space", Host, S::Pass, B::System, "{free} bytes free.");
    space.facts = vec![
        ("free", Fact::Bytes(107_374_182_400)),
        ("total", Fact::Bytes(214_748_364_800)),
    ];
    space.advice.values = vec![("free", Fact::Bytes(107_374_182_400))];
    let mut key_file = finding(
        "keys.file",
        Forger,
        S::Fail,
        B::System,
        "delegates.json is mode {mode}.\nRun: chmod 600 {path}",
    );
    let key_path = Fact::Path("/users/operator/.config/solar-core/testnet/delegates.json".into());
    key_file.facts = vec![
        ("mode", Fact::Mode(0o644)),
        ("owner", Fact::Word("same")),
        ("path", key_path.clone()),
    ];
    key_file.advice.values = vec![("mode", Fact::Mode(0o644)), ("path", key_path)];
    let mut resources = finding(
        "host.resources",
        Host,
        S::Pass,
        B::System,
        "{cpus} CPUs and {memory} bytes of memory.",
    );
    resources.facts = vec![
        ("cpus", Fact::Int(4)),
        ("memory", Fact::Bytes(8_214_528_000)),
        ("fromPlanning", Fact::Bool(true)),
    ];
    resources.advice.values = vec![
        ("cpus", Fact::Int(4)),
        ("memory", Fact::Bytes(8_214_528_000)),
    ];
    let proof = Finding::skipped(check("onboarding.sync-proof"), Relay, keys::SKIPPED_PROFILE);

    Report {
        created_at_ms: NOW_MS,
        tool: BuildFacts {
            version: "4.3.1-heartwood.0.3.0",
            features: vec!["devnet"],
        },
        scope: ReportScope {
            token: "solar".into(),
            network: "testnet".into(),
            roles: vec![Role::Relay, Role::Forger],
            profile: DoctorProfile::Default,
            window_minutes: 60,
        },
        redaction: Redaction::default(),
        host: HostFacts {
            os: "linux",
            arch: "x86_64",
            kernel: Some("6.8".into()),
            cpus: Some(4),
            memory_bytes: Some(8_214_528_000),
        },
        identity: HostIdentity {
            home: Some(PathBuf::from("/users/operator")),
            user: Some("operator".into()),
            host_name: Some("relay-1.example.net".into()),
        },
        findings: vec![
            relay, forger, core, count, diversity, state, forks, hosts, one_key, record, start,
            exposure, space, key_file, resources, proof,
        ],
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/doctor/reports")
        .join(name)
}

#[test]
fn the_golden_report_renders_to_the_committed_json_and_text() {
    let report = golden();
    let (json, text) = (render_json(&report), render_text(&report, false));
    if std::env::var_os("DOCTOR_GOLDEN_WRITE").is_some() {
        std::fs::write(fixture("golden.json"), &json).unwrap();
        std::fs::write(fixture("golden.txt"), &text).unwrap();
        return;
    }
    assert_eq!(
        json, GOLDEN_JSON,
        "the JSON report changed; see the module doc"
    );
    assert_eq!(text, GOLDEN_TEXT, "the text changed; see the module doc");
    let order: Vec<&str> = ENTRIES.iter().map(|entry| entry.id).collect();
    let value: Value = serde_json::from_str(&json).unwrap();
    let ids: Vec<&str> = value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    let positions: Vec<usize> = ids
        .iter()
        .map(|id| order.iter().position(|o| o == id).unwrap())
        .collect();
    assert!(
        positions.is_sorted(),
        "findings in the registry's order: {ids:?}"
    );
}

#[test]
fn nothing_the_golden_report_hides_reaches_its_json_or_text() {
    let report = golden();
    let mut shown = report.clone();
    shown.redaction.addresses = false;
    for output in [
        render_json(&report),
        render_text(&report, false),
        render_text(&report, true),
    ] {
        for hidden in [
            "198.51.100",
            "203.0.113",
            "2001:db8",
            "fe80",
            "10.0.0.5",
            "127.0.0.1",
            "/users/operator",
            "relay-1",
            "operator",
            "eth0",
            "\\n[2026",
            "\u{202e}",
            "\n[2026",
        ] {
            assert!(!output.contains(hidden), "{hidden:?} in {output}");
        }
    }
    let block = "9f".repeat(32);
    assert!(
        render_json(&report).contains(&block),
        "a block id is public chain data"
    );
    let text = render_text(&shown, false);
    assert!(text.contains("198.51.100.23") && text.contains("[2001:db8::10]:4003"));
    assert!(!text.contains("/users/operator") && !text.contains("relay-1"));
    let mut hidden = report.clone();
    hidden.redaction.identity = true;
    for output in [render_json(&hidden), render_text(&hidden, false)] {
        assert!(
            !output.contains("genesis_7") && !output.contains(&"4c".repeat(32)),
            "{output}"
        );
        assert!(output.contains("delegate-1"), "{output}");
    }
    assert!(render_json(&hidden).contains(r#""publicKey":"pk-1""#));
}

#[test]
fn a_hundred_renderings_and_a_hundred_runs_give_the_same_bytes() {
    let report = golden();
    let (json, text) = (render_json(&report), render_text(&report, true));
    for _ in 0..100 {
        assert_eq!(render_json(&golden()), json);
        assert_eq!(render_text(&golden(), true), text);
    }
    let world = FakeWorld::default();
    let (scope, options) = (scope("/nodes/node1"), DoctorOptions::default());
    let first = render_json(&run(&world, &scope, &options));
    for _ in 0..100 {
        assert_eq!(render_json(&run(&world, &scope, &options)), first);
    }
}

/// Validate `value` against `schema` for the keywords the report's schema uses; an object whose
/// every property is required and that admits no other must hold them in the schema's order.
fn validate(root: &Value, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    let fail = |what: &str| Err(format!("{at}: {what} in {value}"));
    let schema = match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => &root["$defs"][reference.trim_start_matches("#/$defs/")],
        None => schema,
    };
    let known = [
        "$schema",
        "$id",
        "$defs",
        "$ref",
        "title",
        "description",
        "type",
        "const",
        "enum",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "maxItems",
        "format",
    ];
    for keyword in schema.as_object().unwrap().keys() {
        assert!(
            known.contains(&keyword.as_str()),
            "{at}: keyword {keyword} is not checked"
        );
    }
    if let Some(types) = schema.get("type") {
        let types: Vec<&str> = match types {
            Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
            other => vec![other.as_str().unwrap()],
        };
        let kind = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if !types.contains(&kind) {
            return fail(&format!("{kind} is not {types:?}"));
        }
    }
    if schema.get("const").is_some_and(|c| c != value) {
        return fail("not the constant");
    }
    if let Some(list) = schema.get("enum").and_then(Value::as_array) {
        if !list.contains(value) {
            return fail("not in the enum");
        }
    }
    let number = value.as_f64();
    if schema
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|m| number.is_some_and(|n| n < m))
    {
        return fail("below the minimum");
    }
    if schema
        .get("maximum")
        .and_then(Value::as_f64)
        .is_some_and(|m| number.is_some_and(|n| n > m))
    {
        return fail("above the maximum");
    }
    if let Some(text) = value.as_str() {
        let chars = u64::try_from(text.chars().count()).unwrap();
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|m| chars > m)
        {
            return fail("too long");
        }
        let date = text.len() == 24 && text.as_bytes()[10] == b'T' && text.ends_with('Z');
        if schema.get("format").and_then(Value::as_str) == Some("date-time") && !date {
            return fail("not a UTC date-time");
        }
    }
    if let Some(items) = value.as_array() {
        let most = schema.get("maxItems").and_then(Value::as_u64);
        if most.is_some_and(|m| items.len() > usize::try_from(m).unwrap()) {
            return fail("too many items");
        }
        for (n, item) in items.iter().enumerate() {
            validate(root, &schema["items"], item, &format!("{at}[{n}]"))?;
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
        if let Some(properties) = properties {
            let names: Vec<&str> = properties.keys().map(String::as_str).collect();
            if closed
                && required == names
                && object.keys().map(String::as_str).ne(names.iter().copied())
            {
                return fail(&format!("keys are not {names:?} in that order"));
            }
            for name in &required {
                if !object.contains_key(*name) {
                    return fail(&format!("no {name}"));
                }
            }
        }
        for (name, item) in object {
            let own = properties.and_then(|p| p.get(name));
            let sub = match (own, schema.get("additionalProperties")) {
                (Some(own), _) => own,
                (None, Some(Value::Bool(false))) => return fail(&format!("{name} is not allowed")),
                (None, Some(other)) => other,
                (None, None) => continue,
            };
            validate(root, sub, item, &format!("{at}.{name}"))?;
        }
    }
    Ok(())
}

fn schema() -> Value {
    serde_json::from_str(REPORT_SCHEMA).unwrap()
}

#[test]
fn the_schema_accepts_what_the_renderer_writes() {
    let schema = schema();
    let mut hidden = golden();
    hidden.redaction.identity = true;
    let mut empty = golden();
    empty.findings.clear();
    empty.host = HostFacts::default();
    let world = FakeWorld::default();
    let fakes = run(&world, &scope("/nodes/node1"), &DoctorOptions::default());
    let mut many = golden();
    for n in 0..30 {
        let mut finding = Finding::new(
            check("sync.forks"),
            Subject::Relay,
            Status::Warn,
            Basis::Log,
            Advice::key("x"),
        );
        finding.untrusted.push(untrusted(
            UntrustedSource::Log(Role::Relay),
            Some(NOW_MS),
            &format!("{n} {}", "x".repeat(400)),
        ));
        many.findings.push(finding);
    }
    for report in [golden(), hidden, empty, fakes, many] {
        let value = report_value(&report);
        validate(&schema, &schema, &value, "report").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&render_json(&report)).unwrap(),
            value
        );
    }
}

#[test]
fn the_schema_names_the_code_s_words() {
    let schema = schema();
    let names = |list: &Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    let finding = &schema["$defs"]["finding"]["properties"];
    let statuses: Vec<&str> = Status::ALL.iter().map(|s| s.name()).collect();
    assert_eq!(names(&finding["status"]["enum"]), statuses);
    let bases = [
        Basis::Log,
        Basis::Chain,
        Basis::Process,
        Basis::Socket,
        Basis::Config,
        Basis::System,
    ];
    assert_eq!(names(&finding["basis"]["enum"]), bases.map(|b| b.name()));
    let subjects = [Subject::Host, Subject::Relay, Subject::Forger];
    assert_eq!(
        names(&finding["subject"]["enum"]),
        subjects.map(|s| s.name())
    );
    let roles: Vec<&str> = Role::ALL.iter().map(|r| r.name()).collect();
    assert_eq!(
        names(&schema["properties"]["scope"]["properties"]["roles"]["items"]["enum"]),
        roles
    );
    assert_eq!(schema["properties"]["format"]["const"], FORMAT);
    assert_eq!(schema["properties"]["untrusted"]["maxItems"], 20);
    assert_eq!(
        schema["$defs"]["excerpt"]["properties"]["text"]["maxLength"],
        160
    );
}

#[test]
fn the_schema_refuses_a_report_the_renderer_would_not_write() {
    let schema = schema();
    let good = report_value(&golden());
    let breaks: [(&str, fn(&mut Value)); 8] = [
        ("a missing key", |v| {
            drop(v.as_object_mut().unwrap().shift_remove("host"))
        }),
        ("an extra key", |v| {
            drop(v.as_object_mut().unwrap().insert("extra".into(), json!(1)))
        }),
        ("keys out of order", |v| {
            let host = v.as_object_mut().unwrap().shift_remove("host").unwrap();
            v.as_object_mut().unwrap().insert("host".into(), host);
        }),
        ("a status", |v| v["findings"][0]["status"] = json!("ok")),
        ("a fact that is an object", |v| {
            v["findings"][0]["facts"]["x"] = json!({})
        }),
        ("addresses shown", |v| {
            v["redaction"]["addresses"] = json!(false)
        }),
        ("a long excerpt", |v| {
            v["untrusted"][0]["text"] = json!("x".repeat(161))
        }),
        ("a local time", |v| {
            v["createdAt"] = json!("2026-10-02 14:03:11")
        }),
    ];
    for (what, change) in breaks {
        let mut value = good.clone();
        change(&mut value);
        assert!(
            validate(&schema, &schema, &value, "report").is_err(),
            "{what} passed"
        );
    }
}
