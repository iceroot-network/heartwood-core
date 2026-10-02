//! `config publish` and `config forger private-key`: the template written into an empty
//! directory and refused over an existing one, the key written into the existing `delegates.json`
//! (the bytes the reference's `writeJsonSync` leaves, at mode 0600, through a rename, a link
//! refused), the deferred BIP39 method, the colon spellings, and V35's `config.forgerKey` records.
//!
//! The records are read from `tests/vectors`, or from the directory in `CLI_VECTORS` (a relative
//! path is taken from the workspace root); a directory that lacks the kind is said so and passes
//! until the vectors stage installs it.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use heartwood_cli::argv::rewrite;
use heartwood_cli::config::{ConfigError, forger, forger_private_key, publish, with_key};
use heartwood_cli::env;
use heartwood_cli::seams::{Instance, Role};
use heartwood_kernel::config::app::{self, Role as AppRole};
use heartwood_kernel::paths::Paths;
use serde_json::Value;

/// The key every test writes: lower case and upper case spell it.
const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-config")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An instance whose configuration directory is `root/config`, which is not made.
fn instance(root: &Path) -> Instance {
    Instance {
        token: "node1".into(),
        network: "devnet".into(),
        role: Role::Relay,
        paths: Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            log: root.join("log"),
            temp: root.join("temp"),
        },
    }
}

/// An instance whose `delegates.json` holds `text`.
fn with_file(test: &str, text: &str) -> Instance {
    let instance = instance(&scratch(test));
    fs::create_dir_all(&instance.paths.config).unwrap();
    fs::write(delegates(&instance), text).unwrap();
    instance
}

fn delegates(instance: &Instance) -> PathBuf {
    instance.paths.config.join("delegates.json")
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().mode() & 0o777
}

#[test]
fn publish_fills_an_empty_directory_and_refuses_an_existing_one_without_force() {
    let instance = instance(&scratch("publish"));
    let config = &instance.paths.config;
    let published = publish(&instance, false).unwrap();
    assert_eq!(&published.dir, config);
    let names: Vec<_> = published
        .files
        .iter()
        .map(|f| f.file_name().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            ".env",
            "app.json",
            "peers.json",
            "delegates.json",
            "config.json"
        ]
    );
    // The key file is private; the others are not secret.
    assert_eq!(mode(&config.join("delegates.json")), 0o600);
    for name in [".env", "app.json", "peers.json", "config.json"] {
        assert_eq!(mode(&config.join(name)), 0o644, "{name}");
    }
    assert_eq!(
        fs::read_to_string(config.join("delegates.json")).unwrap(),
        "{\n    \"keys\": []\n}\n"
    );
    assert_eq!(
        fs::read_to_string(config.join("peers.json")).unwrap(),
        "{\n    \"list\": []\n}\n"
    );
    assert_eq!(
        fs::read_to_string(config.join("config.json")).unwrap(),
        "{\n    \"token\": \"node1\",\n    \"network\": \"devnet\"\n}\n"
    );
    // No crypto files: they come from `network generate --chain-only` or a copy, as the output says.
    assert!(!config.join("crypto").exists());
    let said = published.to_string();
    assert!(said.contains("network generate --chain-only"), "{said}");
    assert!(
        said.contains(&config.join("crypto").display().to_string()),
        "{said}"
    );
    // Nothing is left beside the files: no temporary file.
    assert_eq!(fs::read_dir(config).unwrap().count(), 5);

    // An existing directory is refused with the reference's text, and left as it was.
    fs::write(config.join("peers.json"), "{\"list\": [], \"changed\": 1}").unwrap();
    let refused = publish(&instance, false).unwrap_err();
    assert_eq!(
        refused.to_string(),
        "Please use the --force flag if you wish to overwrite the existing configuration"
    );
    assert!(
        fs::read_to_string(config.join("peers.json"))
            .unwrap()
            .contains("changed")
    );

    // `--force` writes the template's files over it and leaves every other file.
    fs::create_dir(config.join("crypto")).unwrap();
    fs::write(config.join("crypto/network.json"), "kept").unwrap();
    publish(&instance, true).unwrap();
    assert!(
        !fs::read_to_string(config.join("peers.json"))
            .unwrap()
            .contains("changed")
    );
    assert_eq!(
        fs::read_to_string(config.join("crypto/network.json")).unwrap(),
        "kept"
    );
}

#[test]
fn publish_fills_a_directory_that_exists_and_is_empty() {
    let instance = instance(&scratch("publish-empty"));
    fs::create_dir_all(&instance.paths.config).unwrap();
    publish(&instance, false).unwrap();
    assert_eq!(fs::read_dir(&instance.paths.config).unwrap().count(), 5);
}

#[test]
fn the_template_is_one_for_any_network_and_the_node_loads_it() {
    for network in ["devnet", "mainnet", "my-net_2"] {
        let mut instance = instance(&scratch(&format!("template-{network}")));
        instance.network = network.into();
        publish(&instance, false).unwrap();
        let config = &instance.paths.config;
        let json = fs::read_to_string(config.join("config.json")).unwrap();
        assert!(json.contains(network), "{json}");
        // The ports of Solar's testnet, and its other defaults.
        let text = fs::read_to_string(config.join(".env")).unwrap();
        let vars = env::parse(&text);
        let var = |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(var("CORE_P2P_PORT"), Some("6002"));
        assert_eq!(var("CORE_API_PORT"), Some("6003"));
        assert_eq!(var("CORE_LOG_LEVEL"), Some("debug"));
        // Every profile the loader requires, and the core's is the relay's with the forger.
        let packages = |role| -> Vec<String> {
            let profile = app::load(config, role).unwrap();
            profile.plugins.into_iter().map(|p| p.package).collect()
        };
        let (relay, core) = (packages(AppRole::Relay), packages(AppRole::Core));
        assert_eq!(core[..relay.len()], relay[..]);
        assert_eq!(
            core.last().map(String::as_str),
            Some("@solar-network/forger")
        );
        assert_eq!(packages(AppRole::Forger).last(), core.last());
        assert!(relay.iter().any(|p| p == "@solar-network/api"));
        let peers = heartwood_kernel::config::peers::load(config).unwrap();
        assert!(peers.list.is_empty() && peers.sources.is_empty());
    }
}

#[test]
fn publish_refuses_a_symbolic_link_in_place_of_a_file_and_leaves_its_target() {
    let root = scratch("publish-link");
    let instance = instance(&root);
    let config = &instance.paths.config;
    fs::create_dir_all(config).unwrap();
    let target = root.join("elsewhere.env");
    fs::write(&target, "KEPT=1\n").unwrap();
    symlink(&target, config.join(".env")).unwrap();
    // Even with `--force`: a link is refused, as the other writers of the instance refuse one.
    let refused = publish(&instance, true).unwrap_err();
    assert!(matches!(refused, ConfigError::Io { .. }), "{refused:?}");
    assert!(refused.to_string().contains("symbolic link"), "{refused}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "KEPT=1\n");
    assert!(
        fs::symlink_metadata(config.join(".env"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn private_key_refuses_an_absent_delegates_file_naming_its_path() {
    let instance = instance(&scratch("absent"));
    fs::create_dir_all(&instance.paths.config).unwrap();
    let refused = forger_private_key(&instance, KEY).unwrap_err();
    assert!(matches!(refused, ConfigError::Absent(_)), "{refused:?}");
    let said = refused.to_string();
    assert!(
        said.contains(&delegates(&instance).display().to_string()),
        "{said}"
    );
    assert!(!delegates(&instance).exists());
}

#[test]
fn private_key_replaces_only_keys_and_keeps_the_rest_in_place() {
    let text = "{\n  \"secrets\": [],\n  \"extra\": {\"a\": [1, 2]},\n  \"keys\": [\"old\"],\n  \"z\": 1.0\n}";
    let instance = with_file("replace", text);
    forger_private_key(&instance, &KEY.to_uppercase()).unwrap();
    // The bytes the reference's writeJsonSync leaves: four spaces, a final newline, the key in
    // lower case where `keys` was, every other key as it was.
    let wanted = format!(
        "{{\n    \"secrets\": [],\n    \"extra\": {{\n        \"a\": [\n            1,\n            2\n        ]\n    }},\n    \"keys\": [\n        \"{KEY}\"\n    ],\n    \"z\": 1\n}}\n"
    );
    assert_eq!(fs::read_to_string(delegates(&instance)).unwrap(), wanted);
    // A key file with no `keys` gets it after the others; `secrets: []` stays.
    let instance = with_file("append", "{\"secrets\": []}");
    forger_private_key(&instance, KEY).unwrap();
    let wanted =
        format!("{{\n    \"secrets\": [],\n    \"keys\": [\n        \"{KEY}\"\n    ]\n}}\n");
    assert_eq!(fs::read_to_string(delegates(&instance)).unwrap(), wanted);
    // A non-empty `secrets` stays too: the next start refuses it.
    let instance = with_file("secrets", "{\"secrets\": [\"one two\"]}");
    forger_private_key(&instance, KEY).unwrap();
    let written: Value =
        serde_json::from_str(&fs::read_to_string(delegates(&instance)).unwrap()).unwrap();
    assert_eq!(written["secrets"][0], "one two");
    assert_eq!(written["keys"][0], KEY);
}

#[test]
fn private_key_writes_mode_0600_through_a_rename() {
    let instance = with_file("mode", "{\"keys\": []}\n");
    let path = delegates(&instance);
    // A file that was wider is private after the write, and was replaced, not written in place.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(&path).unwrap().ino();
    forger_private_key(&instance, KEY).unwrap();
    assert_eq!(mode(&path), 0o600);
    assert_ne!(
        fs::metadata(&path).unwrap().ino(),
        before,
        "written in place"
    );
    // Nothing else is left in the directory.
    let names: Vec<_> = fs::read_dir(&instance.paths.config).unwrap().collect();
    assert_eq!(names.len(), 1, "{names:?}");
    // A private file stays private.
    forger_private_key(&instance, KEY).unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn private_key_refuses_a_key_that_is_not_64_hexadecimal_characters() {
    let instance = with_file("bad-key", "{\"keys\": []}\n");
    let bad = [
        &KEY[1..],
        &format!("{KEY}0"),
        "",
        &format!("g{}", &KEY[1..]),
        &format!("0x{}", &KEY[2..]),
        &format!("{} ", &KEY[..63]),
        &format!("{}\n", &KEY[..63]),
        &"\u{ff10}".repeat(64),
    ];
    for key in bad {
        let refused = forger_private_key(&instance, key).unwrap_err();
        assert_eq!(refused, ConfigError::Key, "{key:?}");
    }
    assert_eq!(
        fs::read_to_string(delegates(&instance)).unwrap(),
        "{\"keys\": []}\n"
    );
    // The refusal never prints the key.
    assert!(!ConfigError::Key.to_string().contains(KEY));
    // No key at all is the same refusal.
    assert_eq!(forger(&instance, None, None).unwrap_err(), ConfigError::Key);
}

#[test]
fn private_key_checks_the_key_before_it_reads_the_file() {
    // The reference's validator runs before its `require`: a bad key with no file is the key's
    // refusal, and the file is not read.
    let instance = instance(&scratch("key-first"));
    fs::create_dir_all(&instance.paths.config).unwrap();
    let refused = forger_private_key(&instance, "xyz").unwrap_err();
    assert_eq!(refused, ConfigError::Key);
}

#[test]
fn private_key_refuses_a_fifo_without_waiting_for_a_writer() {
    let instance = instance(&scratch("fifo"));
    fs::create_dir_all(&instance.paths.config).unwrap();
    nix::unistd::mkfifo(&delegates(&instance), nix::sys::stat::Mode::S_IRWXU).unwrap();
    let (sent, got) = std::sync::mpsc::channel();
    std::thread::spawn(move || sent.send(forger_private_key(&instance, KEY)));
    let done = got.recv_timeout(std::time::Duration::from_secs(10));
    let done = done.expect("the command waits for a writer of the FIFO");
    assert!(matches!(done, Err(ConfigError::Io { .. })), "{done:?}");
}

#[test]
fn private_key_refuses_a_symbolic_link_and_leaves_its_target() {
    let instance = with_file("link", "{\"keys\": []}\n");
    let target = instance.paths.config.join("elsewhere.json");
    fs::rename(delegates(&instance), &target).unwrap();
    symlink(&target, delegates(&instance)).unwrap();
    let refused = forger_private_key(&instance, KEY).unwrap_err();
    assert!(matches!(refused, ConfigError::Io { .. }), "{refused:?}");
    assert!(refused.to_string().contains("symbolic link"), "{refused}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "{\"keys\": []}\n");
    assert!(
        fs::symlink_metadata(delegates(&instance))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn private_key_refuses_a_file_that_is_not_a_json_object_and_keeps_it() {
    for (name, text) in [
        ("array", "[]"),
        ("null", "null"),
        ("text", "not json"),
        ("empty", ""),
    ] {
        let instance = with_file(&format!("not-object-{name}"), text);
        let refused = forger_private_key(&instance, KEY).unwrap_err();
        assert!(
            matches!(refused, ConfigError::Io { .. }),
            "{name}: {refused:?}"
        );
        assert_eq!(fs::read_to_string(delegates(&instance)).unwrap(), text);
    }
}

#[test]
fn the_bip39_method_names_its_deferral_and_changes_nothing() {
    let instance = with_file("bip39", "{\"keys\": []}\n");
    let refused = forger(&instance, Some("bip39"), Some(KEY)).unwrap_err();
    assert_eq!(refused, ConfigError::Method("bip39".into()));
    assert!(refused.to_string().contains("deferred"), "{refused}");
    assert_eq!(
        fs::read_to_string(delegates(&instance)).unwrap(),
        "{\"keys\": []}\n"
    );
    // The private-key method, named or not, writes the key and says which file.
    assert_eq!(
        forger(&instance, Some("privateKey"), Some(KEY)).unwrap(),
        delegates(&instance)
    );
    assert!(
        fs::read_to_string(delegates(&instance))
            .unwrap()
            .contains(KEY)
    );
}

#[test]
fn every_colon_spelling_of_config_is_its_spaced_form() {
    let args = |words: &[&str]| -> Vec<std::ffi::OsString> {
        ["heartwood"].iter().chain(words).map(Into::into).collect()
    };
    let more = ["--network", "devnet", "--privateKey", KEY];
    for (colon, spaced) in [
        ("config:publish", &["config", "publish"][..]),
        ("config:forger", &["config", "forger"]),
        (
            "config:forger:private-key",
            &["config", "forger", "private-key"],
        ),
    ] {
        let colon = rewrite(args(&[&[colon][..], &more].concat()));
        assert_eq!(colon, args(&[spaced, &more].concat()), "{colon:?}");
    }
}

/// The class's records of kind `config.forgerKey`, or `None` when there are none to read.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("CLI_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let text = fs::read_to_string(dir.join("V35-operator.jsonl")).ok()?;
    let lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    Some(
        lines
            .filter(|record| record["op"] == "config.forgerKey")
            .collect(),
    )
}

#[test]
fn config_forger_key_matches_the_records() {
    let Some(records) = records().filter(|records| !records.is_empty()) else {
        eprintln!("V35-operator has no config.forgerKey records to read");
        return;
    };
    let scratch = scratch("vectors");
    let (mut accepted, mut refused) = (0, 0);
    for (number, record) in records.iter().enumerate() {
        let (input, name) = (&record["input"], &record["name"]);
        let instance = instance(&scratch.join(number.to_string()));
        fs::create_dir_all(&instance.paths.config).unwrap();
        if let Some(file) = input["delegatesJson"].as_str() {
            fs::write(delegates(&instance), file).unwrap();
        }
        // The command's own flag is text; any other kind of flag is the schema's refusal.
        let key = input["flags"]["privateKey"].as_str();
        let done = key.map(|key| forger_private_key(&instance, key));
        match (record.get("output"), done) {
            (Some(output), Some(Ok(()))) => {
                let wanted = output["delegatesJson"].as_str().unwrap();
                assert_eq!(
                    fs::read_to_string(delegates(&instance)).unwrap(),
                    wanted,
                    "{name}"
                );
                assert_eq!(mode(&delegates(&instance)), 0o600, "{name}");
                accepted += 1;
            }
            (None, None | Some(Err(_))) => refused += 1,
            (output, done) => panic!("{name}: the reference gave {output:?}, Heartwood {done:?}"),
        }
        // The pure form agrees with the command on every record.
        if let (Some(key), Some(file)) = (key, input["delegatesJson"].as_str()) {
            let text = with_key(file, key);
            assert_eq!(text.is_ok(), record.get("output").is_some(), "{name}");
        }
    }
    assert!(
        accepted >= 10 && refused >= 10,
        "{accepted} accepted, {refused} refused"
    );
    assert!(
        records.len() >= 20,
        "{} config.forgerKey records",
        records.len()
    );
}
