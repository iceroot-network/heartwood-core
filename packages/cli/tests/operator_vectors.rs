//! Vector class V35 (the operator commands' files), the environment file's kinds:
//!
//! - `env.stringify`: the text `env set` leaves in the file. A record with a file is run through
//!   `env::set` on a real file (comments and blank lines dropped, the order of a JavaScript
//!   object's keys, no final line break); one without is `env::stringify` over its variables.
//! - `env.parseCli`: what the reference's CLI reads with envfile. Heartwood's commands read the
//!   file with the node's parser, so a line the record notes `cliEnvParser` is read otherwise, and
//!   every other record reads alike.
//!
//! The class is read from `tests/vectors`, or from the directory in `CLI_VECTORS` (a relative path
//! is taken from the workspace root), such as a local oracle run in `target/vectors/local`. A
//! directory named in the environment may lack the class, and the test then says so and passes;
//! the crate's own directory holds it once `make vectors-install` has put it there, and until the
//! crate has a `tests/vectors` at all the test says so and passes.

use std::fs;
use std::path::{Path, PathBuf};

use heartwood_cli::env::{parse, set, stringify};
use heartwood_cli::seams::{Instance, Role};
use heartwood_kernel::paths::Paths;
use serde_json::{Value, json};

const CLASS: &str = "V35-operator";
const MINIMUM: usize = 20;

/// The class's records without its meta record, or `None` when there is none to read.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("CLI_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(_) if named.is_some() => return None,
        Err(_) if !dir.join("MANIFEST.sha256").exists() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    Some(lines.collect())
}

fn instance(config: PathBuf) -> Instance {
    let elsewhere = |name: &str| config.with_file_name(name);
    Instance {
        token: "node1".into(),
        network: "devnet".into(),
        role: Role::Relay,
        paths: Paths {
            data: elsewhere("data"),
            cache: elsewhere("cache"),
            log: elsewhere("log"),
            temp: elsewhere("temp"),
            config,
        },
    }
}

/// A value as text: a string as it is, a number as JavaScript writes it.
fn value(value: &Value) -> String {
    match value.as_str() {
        Some(text) => text.to_owned(),
        None => heartwood_crypto::utils::js::number_to_string(value.as_f64().unwrap()),
    }
}

fn variables(input: &Value) -> Vec<(String, String)> {
    let list = input["vars"].as_array().unwrap();
    let pair = |pair: &Value| (pair[0].as_str().unwrap().to_owned(), value(&pair[1]));
    list.iter().map(pair).collect()
}

#[test]
fn env_stringify_matches_the_records() {
    let Some(records) = records() else {
        eprintln!("{CLASS} is not there to read");
        return;
    };
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-operator-vectors");
    let (mut compared, mut files) = (0, 0);
    for (number, record) in records.iter().enumerate() {
        if record["op"] != "env.stringify" {
            continue;
        }
        let (input, wanted) = (&record["input"], record["output"]["text"].as_str().unwrap());
        let vars = variables(input);
        let name = &record["name"];
        let got = match input["file"].as_str() {
            Some(file) => {
                files += 1;
                let config = scratch.join(number.to_string()).join("config");
                let _ = fs::remove_dir_all(config.parent().unwrap());
                fs::create_dir_all(&config).unwrap();
                fs::write(config.join(".env"), file).unwrap();
                let on = instance(config.clone());
                for (key, text) in &vars {
                    set(&on, key, text).unwrap_or_else(|error| panic!("{name}: {error}"));
                }
                fs::read_to_string(config.join(".env")).unwrap()
            }
            None => stringify(&vars),
        };
        if record["output"]["notes"]["cliEnvParser"] != true {
            assert_eq!(got, wanted, "{name}");
        }
        compared += 1;
    }
    assert!(compared >= MINIMUM, "{compared} env.stringify records");
    assert!(files >= 10, "{files} of them with a file");
}

#[test]
fn env_parse_cli_matches_the_records_where_the_parsers_agree() {
    let Some(records) = records() else {
        eprintln!("{CLASS} is not there to read");
        return;
    };
    let (mut compared, mut differing) = (0, 0);
    for record in records.iter().filter(|r| r["op"] == "env.parseCli") {
        let text = record["input"]["text"].as_str().unwrap();
        let ours: Vec<Value> = parse(text)
            .into_iter()
            .map(|(k, v)| json!([k, v]))
            .collect();
        let same = Value::Array(ours) == record["output"]["pairs"];
        // The note is exactly where the two parsers read the text differently.
        let noted = record["output"]["notes"]["cliEnvParser"] == true;
        assert_eq!(same, !noted, "{}", record["name"]);
        differing += usize::from(noted);
        compared += 1;
    }
    assert!(compared >= MINIMUM, "{compared} env.parseCli records");
    assert!(differing >= 1, "no record shows the parsers apart");
}
