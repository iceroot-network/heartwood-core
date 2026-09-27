//! Instance resolution: explicit flags over `config.json`, the five `CORE_PATH_*` variables, the
//! discovery of the one network with a complete chain definition, and the instance's name.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use heartwood_cli::InstanceError;
use heartwood_cli::instance::{CRYPTO_FILES, InstanceFlags, config_paths, resolve};
use heartwood_cli::seams::Role;

/// A fresh directory for one test, under the target's temporary directory.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-instance")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn flags(token: Option<&str>, network: Option<&str>) -> InstanceFlags {
    InstanceFlags {
        token: token.map(str::to_owned),
        network: network.map(str::to_owned),
    }
}

/// An environment with `vars`, the home directory `home` as XDG's defaults use it.
fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name: &str| vars.get(name).cloned()
}

fn write_config(dir: &Path, text: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("config.json"), text).unwrap();
}

#[test]
fn flags_name_the_instance_and_its_directories_under_the_token_core() {
    let home = scratch("flags");
    let cwd = home.join("work");
    let env = env(&[]);
    let instance = resolve(
        Role::Relay,
        &flags(Some("node1"), Some("devnet")),
        &env,
        &home,
        &cwd,
    )
    .unwrap();
    assert_eq!(instance.name(), "node1-devnet-relay");
    assert_eq!(instance.token, "node1");
    assert_eq!(instance.network, "devnet");
    assert_eq!(
        instance.paths.config,
        home.join(".config/node1-core/devnet")
    );
    assert_eq!(
        instance.paths.data,
        home.join(".local/share/node1-core/devnet")
    );
    assert_eq!(instance.paths.cache, home.join(".cache/node1-core/devnet"));
    assert_eq!(
        instance.paths.log,
        home.join(".local/state/node1-core/devnet")
    );
    let user = home.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        instance.paths.temp,
        Path::new("/tmp").join(user).join("node1-core/devnet")
    );
    let forger = resolve(
        Role::Forger,
        &flags(Some("node1"), Some("devnet")),
        &env,
        &home,
        &cwd,
    )
    .unwrap();
    assert_eq!(forger.name(), "node1-devnet-forger");
    assert_eq!(forger.paths, instance.paths);
}

#[test]
fn every_one_of_the_five_core_path_variables_is_honoured() {
    let home = scratch("paths");
    let cwd = home.join("work");
    let env = env(&[
        ("CORE_PATH_CONFIG", "/nodes/n3/config"),
        ("CORE_PATH_DATA", "~/n3/data"),
        ("CORE_PATH_CACHE", "~+/cache"),
        ("CORE_PATH_LOG", "relative/log"),
        ("CORE_PATH_TEMP", "/nodes/n3/temp/../tmp"),
    ]);
    let instance = resolve(
        Role::Forger,
        &flags(Some("node3"), Some("devnet")),
        &env,
        &home,
        &cwd,
    )
    .unwrap();
    assert_eq!(instance.paths.config, Path::new("/nodes/n3/config"));
    assert_eq!(instance.paths.data, home.join("n3/data"));
    assert_eq!(instance.paths.cache, cwd.join("cache"));
    assert_eq!(instance.paths.log, cwd.join("relative/log"));
    assert_eq!(instance.paths.temp, Path::new("/nodes/n3/tmp"));
}

#[test]
fn config_json_fills_missing_flags_and_never_overrides_a_given_one() {
    let home = scratch("config");
    let cwd = home.clone();
    let config = home.join("nodes/n2/config");
    write_config(&config, r#"{"token": "node2", "network": "devnet"}"#);
    let config_dir = config.to_str().unwrap().to_owned();
    let env = env(&[("CORE_PATH_CONFIG", &config_dir)]);
    let filled = resolve(Role::Relay, &flags(None, None), &env, &home, &cwd).unwrap();
    assert_eq!(filled.name(), "node2-devnet-relay");
    assert_eq!(filled.paths.config, config);
    assert_eq!(
        filled.paths.data,
        home.join(".local/share/node2-core/devnet")
    );

    let token_given = resolve(Role::Relay, &flags(Some("other"), None), &env, &home, &cwd).unwrap();
    assert_eq!(token_given.name(), "other-devnet-relay");
    let both_given = resolve(
        Role::Forger,
        &flags(Some("mine"), Some("testnet")),
        &env,
        &home,
        &cwd,
    )
    .unwrap();
    assert_eq!(both_given.name(), "mine-testnet-forger");
}

#[test]
fn config_json_is_found_where_the_reference_looks_for_it() {
    let home = scratch("config-places");
    let cwd = home.clone();
    let none = env(&[]);
    assert_eq!(
        config_paths("solar", None, &none, &home, &cwd),
        vec![home.join(".config/solar-core/config.json")]
    );
    assert_eq!(
        config_paths("solar", Some("testnet"), &none, &home, &cwd),
        vec![home.join(".config/solar-core/testnet/config.json")]
    );
    let set = env(&[("CORE_PATH_CONFIG", "/etc/node")]);
    assert_eq!(
        config_paths("solar", Some("testnet"), &set, &home, &cwd),
        vec![
            PathBuf::from("/etc/node/config.json"),
            home.join(".config/solar-core/testnet/config.json")
        ]
    );

    // Without flags, the token defaults to solar and the network comes from the token's own
    // config.json; a CORE_PATH_CONFIG without one falls back to it, as the reference does.
    write_config(
        &home.join(".config/solar-core"),
        r#"{"network": "testnet"}"#,
    );
    let empty = home.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let empty = env(&[("CORE_PATH_CONFIG", empty.to_str().unwrap())]);
    for env in [&none as &dyn Fn(&str) -> Option<String>, &empty] {
        let instance = resolve(Role::Relay, &flags(None, None), env, &home, &cwd).unwrap();
        assert_eq!(instance.name(), "solar-testnet-relay");
    }
}

#[test]
fn a_network_is_discovered_only_when_one_has_a_complete_chain_definition() {
    let home = scratch("discover");
    let cwd = home.clone();
    let env = env(&[]);
    let root = home.join(".config/solar-core");
    let missing = resolve(Role::Relay, &flags(None, None), &env, &home, &cwd);
    assert!(matches!(missing, Err(InstanceError::MissingNetwork { .. })));

    let crypto = root.join("devnet/crypto");
    fs::create_dir_all(&crypto).unwrap();
    for file in &CRYPTO_FILES[..3] {
        fs::write(crypto.join(file), "{}").unwrap();
    }
    fs::create_dir_all(root.join("notes")).unwrap();
    let incomplete = resolve(Role::Relay, &flags(None, None), &env, &home, &cwd);
    assert!(matches!(
        incomplete,
        Err(InstanceError::MissingNetwork { .. })
    ));

    fs::write(crypto.join(CRYPTO_FILES[3]), "{}").unwrap();
    let found = resolve(Role::Relay, &flags(None, None), &env, &home, &cwd).unwrap();
    assert_eq!(found.name(), "solar-devnet-relay");

    let second = root.join("iceroot/crypto");
    fs::create_dir_all(&second).unwrap();
    for file in CRYPTO_FILES {
        fs::write(second.join(file), "{}").unwrap();
    }
    let two = resolve(Role::Relay, &flags(None, None), &env, &home, &cwd);
    assert!(matches!(two, Err(InstanceError::MissingNetwork { .. })));
}

#[test]
fn a_config_json_that_is_not_valid_is_refused() {
    let home = scratch("invalid");
    let cwd = home.clone();
    let config = home.join("config");
    let config_dir = config.to_str().unwrap().to_owned();
    let env = env(&[("CORE_PATH_CONFIG", &config_dir)]);
    for text in ["{", "[]", r#"{"token": 3}"#, r#"{"network": ["devnet"]}"#] {
        write_config(&config, text);
        let refused = resolve(Role::Relay, &flags(None, Some("devnet")), &env, &home, &cwd);
        assert!(
            matches!(&refused, Err(InstanceError::Config { path, .. }) if *path == config.join("config.json")),
            "{text}: {refused:?}"
        );
    }
    write_config(&config, r#"{"token": null, "other": 1}"#);
    let accepted = resolve(Role::Relay, &flags(None, Some("devnet")), &env, &home, &cwd).unwrap();
    assert_eq!(accepted.name(), "solar-devnet-relay");
}

#[test]
fn names_that_cannot_be_part_of_a_file_name_are_refused() {
    let home = scratch("names");
    let cwd = home.clone();
    let env = env(&[]);
    for (token, network, what) in [
        ("../x", "devnet", "token"),
        ("node 1", "devnet", "token"),
        ("", "devnet", "token"),
        ("node1", "dev/net", "network"),
        ("node1", "", "network"),
    ] {
        let refused = resolve(
            Role::Relay,
            &flags(Some(token), Some(network)),
            &env,
            &home,
            &cwd,
        );
        assert!(
            matches!(&refused, Err(InstanceError::InvalidName { what: w, .. }) if *w == what),
            "{token:?} {network:?}: {refused:?}"
        );
    }
    let fine = resolve(
        Role::Relay,
        &flags(Some("Node_1-a"), Some("dev-net_2")),
        &env,
        &home,
        &cwd,
    );
    assert_eq!(fine.unwrap().name(), "Node_1-a-dev-net_2-relay");
}
