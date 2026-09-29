//! The relay's configuration: the environment's precedence, `config.json`, the relay profile as
//! compiled-in components, the command line's options under `app.json`'s, the warnings for what
//! the relay does not honour, the components' option readers, the markers, the reset variables
//! and the version floor.

mod common;

use std::fs;

use common::{Setup, relay_plugins, run};
use heartwood_blockchain::{BlockchainOptions, ReplayReason, RestorePath};
use heartwood_core::app::{RelayError, api_options, open_pool, restored, version_floor};
use heartwood_core::config::{Package, RelayConfig};
use heartwood_core::options;
use heartwood_crypto::TransactionId;
use heartwood_kernel::config::Env;
use heartwood_logger::Level;
use heartwood_pool::{Storage, StoredTransaction};
use serde_json::{Value, json};

fn load(setup: &Setup, flags: &str, extra: &[(&str, &str)]) -> Result<RelayConfig, RelayError> {
    RelayConfig::load(
        &run(flags),
        &setup.process(extra),
        &setup.home(),
        &setup.root,
    )
}

fn refusal(setup: &Setup, flags: &str, extra: &[(&str, &str)]) -> String {
    match load(setup, flags, extra) {
        Err(RelayError::Config(message)) => message,
        other => panic!("{:?}", other.map(|config| config.components)),
    }
}

fn env(pairs: &[(&str, &str)]) -> Env {
    Env::from_vars(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    )
}

#[test]
fn the_process_the_env_files_and_the_directories_combine_as_the_reference_combines_them() {
    let setup = Setup::new(
        "precedence",
        "CORE_P2P_PORT=6002\nCORE_API_PORT=06003\nCORE_TOKEN=nope\n",
    );
    fs::create_dir_all(setup.home().join(".solar")).unwrap();
    fs::write(
        setup.home().join(".solar/.env"),
        "CORE_LOG_LEVEL=debug\nCORE_API_PORT=7003\n",
    )
    .unwrap();
    let config = load(&setup, "--network testnet", &[("CORE_P2P_PORT", "5002")]).unwrap();
    assert_eq!(
        (config.token.as_str(), config.network.as_str()),
        ("solar", "testnet")
    );
    let env = &config.env;
    assert_eq!(env.get("CORE_P2P_PORT"), Some("5002"));
    assert_eq!(env.get("CORE_API_PORT"), Some("7003"));
    assert_eq!(env.get("CORE_LOG_LEVEL"), Some("debug"));
    // The process's identity and directories are set before the files are read.
    assert_eq!(env.get("CORE_TOKEN"), Some("solar"));
    assert_eq!(env.get("CORE_NETWORK_NAME"), Some("testnet"));
    assert_eq!(env.get("CORE_ENV"), Some("production"));
    assert_eq!(
        env.get("CORE_PATH_DATA"),
        Some(setup.root.join("data").to_str().unwrap())
    );
    assert_eq!(config.paths.config, setup.config());
    assert_eq!(config.paths.temp, setup.root.join("temp"));
    // Without the home file, the config file's number reads as JavaScript prints it.
    fs::remove_dir_all(setup.home().join(".solar")).unwrap();
    let config = load(&setup, "--network testnet", &[]).unwrap();
    assert_eq!(config.env.get("CORE_API_PORT"), Some("6003"));
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
}

#[test]
fn config_json_fills_a_missing_token_or_network_and_never_overrides_a_flag() {
    let setup = Setup::new("identity", "");
    assert!(refusal(&setup, "", &[]).contains("--network"));
    setup.write("config.json", r#"{"token": "other", "network": "devnet"}"#);
    let config = load(&setup, "", &[]).unwrap();
    assert_eq!(
        (config.token.as_str(), config.network.as_str()),
        ("other", "devnet")
    );
    let config = load(&setup, "--token mine", &[]).unwrap();
    assert_eq!(
        (config.token.as_str(), config.network.as_str()),
        ("mine", "devnet")
    );
    let config = load(&setup, "--token mine --network testnet", &[]).unwrap();
    assert_eq!(
        (config.token.as_str(), config.network.as_str()),
        ("mine", "testnet")
    );
}

#[test]
fn a_token_or_network_that_is_no_plain_name_is_refused_as_the_commands_refuse_it() {
    let setup = Setup::new("names", "");
    let text = refusal(&setup, "--token ../evil --network net1", &[]);
    assert_eq!(
        text,
        r#"the token "../evil" may hold letters, digits, '-' and '_' only"#
    );
    let text = refusal(&setup, "--network ../../evil-net", &[]);
    assert!(
        text.starts_with(r#"the network "../../evil-net""#),
        "{text}"
    );
    // A config.json that names such a network is refused too.
    setup.write("config.json", r#"{"network": "../evil-net"}"#);
    assert!(refusal(&setup, "", &[]).starts_with(r#"the network "../evil-net""#));
    assert!(load(&setup, "--network dev_net-2", &[]).is_ok());
}

#[test]
fn the_relay_profile_becomes_components_in_rank_order() {
    let setup = Setup::new("profile", "");
    let config = load(&setup, "--network testnet", &[]).unwrap();
    assert_eq!(
        config.components,
        [
            Package::Database,
            Package::State,
            Package::Transactions,
            Package::Pool,
            Package::P2p,
            Package::Blockchain,
            Package::Api
        ]
    );
    // Any file order, the API left out, a second entry of a package replacing the first's options.
    let mut plugins: Vec<Value> = relay_plugins()
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .cloned()
        .collect();
    plugins.retain(|plugin| plugin["package"] != "@solar-network/api");
    plugins.push(json!({"package": "@solar-network/p2p", "options": {"rateLimit": 7}}));
    setup.app(Value::Array(plugins));
    let config = load(&setup, "--network testnet", &[]).unwrap();
    assert_eq!(config.components.len(), 6);
    assert!(!config.components.contains(&Package::Api));
    assert_eq!(config.entry(Package::P2p), json!({"rateLimit": 7}));
    assert_eq!(config.entry(Package::Api), json!({}));
    let mut unknown = relay_plugins();
    unknown
        .as_array_mut()
        .unwrap()
        .push(json!({"package": "@solar-network/nope"}));
    setup.app(unknown);
    assert!(refusal(&setup, "--network testnet", &[]).contains("@solar-network/nope"));
    setup.app(json!([{}]));
    assert!(refusal(&setup, "--network testnet", &[]).contains("unknown package"));
    let mut missing = relay_plugins();
    missing
        .as_array_mut()
        .unwrap()
        .retain(|plugin| plugin["package"] != "@solar-network/blockchain");
    setup.app(missing);
    assert!(refusal(&setup, "--network testnet", &[]).contains("@solar-network/blockchain"));
}

#[test]
fn the_command_line_gives_options_that_app_json_overrides() {
    let setup = Setup::new("flags", "");
    let config = load(
        &setup,
        "--network testnet --networkStart --launch-mode seed",
        &[],
    )
    .unwrap();
    assert_eq!(
        config.entry(Package::Blockchain),
        json!({"networkStart": true})
    );
    let p2p = config.entry(Package::P2p);
    for key in ["networkStart", "skipDiscovery", "ignoreMinimumNetworkReach"] {
        assert_eq!(p2p[key], true, "{key}");
    }
    assert!(p2p.get("disableDiscovery").is_none());
    assert_eq!(p2p["dns"], json!([]));
    let mut plugins = relay_plugins();
    plugins[6] =
        json!({"package": "@solar-network/blockchain", "options": {"networkStart": false}});
    setup.app(plugins);
    let config = load(&setup, "--network testnet --networkStart", &[]).unwrap();
    assert_eq!(
        config.entry(Package::Blockchain),
        json!({"networkStart": false})
    );
}

#[test]
fn warnings_name_what_the_relay_does_not_honour() {
    let setup = Setup::new("warnings", "NODE_ENV=test\n");
    let mut plugins = relay_plugins();
    let list = plugins.as_array_mut().unwrap();
    list.push(json!({"package": "@solar-network/forger"}));
    list[5]["options"]["gossip"] = json!(true);
    list[1]["options"] = json!({"storage": {"maxLastBlocks": 50}});
    let app = json!({"core": {"plugins": []}, "relay": {"plugins": plugins, "flags": ["x"]}, "forger": {"plugins": []}});
    setup.write("app.json", &app.to_string());
    setup.write("app.js", "module.exports = {};");
    let extra = [
        ("CORE_SKIP_PEER_STATE_VERIFICATION", "1"),
        ("CORE_WEBHOOKS_ENABLED", "true"),
        ("CORE_SKIP_BLOCKCHAIN_STARTED_CHECK", ""),
        ("CORE_API_CACHE", "1"),
        ("CORE_API_LOG", ""),
        ("CORE_WALLET_SYNC_ENABLED", "true"),
    ];
    let config = load(&setup, "--network testnet --env test", &extra).unwrap();
    let warnings = config.warnings.join("\n");
    for expected in [
        "relay.flags is not read",
        "app.js is not read",
        "the @solar-network/p2p option gossip is not known",
        "@solar-network/forger is not started by a relay",
        "webhooks are not served",
        "CORE_ENV is not honoured",
        "NODE_ENV is not honoured",
        "CORE_SKIP_PEER_STATE_VERIFICATION is not honoured",
        "CORE_API_CACHE is not honoured",
        "CORE_WALLET_SYNC_ENABLED is not honoured",
        "the @solar-network/state option storage is not read by this relay",
    ] {
        assert!(warnings.contains(expected), "{expected}:\n{warnings}");
    }
    for unset in ["CORE_SKIP_BLOCKCHAIN_STARTED_CHECK", "CORE_API_LOG"] {
        assert!(!warnings.contains(unset), "{warnings}");
    }
    assert_eq!(config.warnings.len(), 11, "{warnings}");
}

#[test]
fn peers_json_and_the_crypto_files_are_checked_before_the_start() {
    let setup = Setup::new("files", "");
    setup.write(
        "peers.json",
        r#"{"list": [], "sources": ["https://192.0.2.9/peers.json"]}"#,
    );
    assert!(refusal(&setup, "--network testnet", &[]).contains("https://192.0.2.9/peers.json"));
    setup.write(
        "peers.json",
        r#"{"list": [], "sources": ["http://192.0.2.9/peers.json"]}"#,
    );
    assert_eq!(
        load(&setup, "--network testnet", &[])
            .unwrap()
            .peers
            .sources
            .len(),
        1
    );
    setup.write("peers.json", r#"{"list": [], "extra": 1}"#);
    assert!(refusal(&setup, "--network testnet", &[]).contains("peers.json"));
    setup.write("peers.json", r#"{"list": []}"#);
    fs::write(
        setup.config().join("crypto/exceptions.json"),
        r#"{"blocks": []}"#,
    )
    .unwrap();
    assert!(refusal(&setup, "--network testnet", &[]).contains("exceptions.json"));
    fs::remove_file(setup.config().join("peers.json")).unwrap();
    assert!(refusal(&setup, "--network testnet", &[]).contains("peers.json"));
}

#[test]
fn the_pool_options_are_read_as_the_reference_schema_reads_them() {
    let data = std::path::Path::new("/data");
    let read = |entry: Value, vars: &[(&str, &str)]| options::pool(&entry, &env(vars), data, 8);
    let pool = read(json!({}), &[]).unwrap();
    assert_eq!(
        (
            pool.max_transactions_in_pool,
            pool.max_transactions_per_request,
            pool.worker_count
        ),
        (15_000, 40, 3)
    );
    assert_eq!(pool.storage, data.join("pool/pool.sqlite"));
    let vars = [
        ("CORE_MAX_TRANSACTIONS_IN_POOL", "500"),
        ("CORE_TRANSACTION_POOL_DISABLED", "1"),
    ];
    let pool = read(json!({"maxTransactionsPerSender": " 20 ", "dynamicFees": {"addonBytes": {"transfer": "10"}}}), &vars).unwrap();
    assert_eq!(
        (
            pool.max_transactions_in_pool,
            pool.max_transactions_per_sender
        ),
        (500, 20)
    );
    assert!(!pool.enabled);
    assert_eq!(pool.dynamic_fees.addon_bytes["transfer"], 10);
    assert_eq!(pool.dynamic_fees.addon_bytes["vote"], 100);
    let pool = read(
        json!({"maxTransactionsInPool": 9, "enabled": "TRUE", "storage": "/x.sqlite"}),
        &vars,
    )
    .unwrap();
    assert_eq!(pool.max_transactions_in_pool, 9);
    assert!(pool.enabled);
    for (entry, vars, key) in [
        (
            json!({}),
            &[("CORE_MAX_TRANSACTIONS_IN_POOL", "abc")][..],
            "maxTransactionsInPool",
        ),
        (
            json!({"maxTransactionAge": 0}),
            &[][..],
            "maxTransactionAge",
        ),
        (json!({"dynamicFees": 5}), &[][..], "dynamicFees"),
        (
            json!({"dynamicFees": {"minFeePool": -1}}),
            &[][..],
            "dynamicFees.minFeePool",
        ),
        (
            json!({"dynamicFees": {"addonBytes": {"vote": "x"}}}),
            &[][..],
            "dynamicFees.addonBytes.vote",
        ),
        (json!({"allowedSenders": "x"}), &[][..], "allowedSenders"),
        (
            json!({"workerPool": {"workerCount": "0"}}),
            &[][..],
            "workerPool.workerCount",
        ),
        (json!({"enabled": "yes"}), &[][..], "enabled"),
    ] {
        let error = read(entry.clone(), vars).unwrap_err();
        assert_eq!(
            (error.package, error.key.as_str()),
            ("@solar-network/pool", key),
            "{entry}"
        );
    }
}

#[test]
fn the_database_logger_and_peer_options_are_read_as_the_reference_reads_them() {
    assert_eq!(options::database(&json!({})), Ok(3_000));
    assert_eq!(
        options::database(
            &json!({"apiConnectionTimeout": "5000", "connection": {"logging": "false"}})
        ),
        Ok(5_000)
    );
    for (entry, key) in [
        (json!({"apiConnectionTimeout": 0}), "apiConnectionTimeout"),
        (json!({"connection": null}), "connection"),
        (json!({"connection": {"extra": []}}), "connection.extra"),
        (json!({"connection": {"type": ""}}), "connection.type"),
        (
            json!({"connection": {"synchronise": "no"}}),
            "connection.synchronise",
        ),
    ] {
        assert_eq!(options::database(&entry).unwrap_err().key, key, "{entry}");
    }
    let logger = options::logger(&json!({}), &env(&[("CORE_LOG_LEVEL", "debug")])).unwrap();
    assert_eq!(
        (logger.console, logger.file, logger.emojify),
        (Some(Level::Debug), Some(Level::Debug), true)
    );
    let logger = options::logger(
        &json!({"levels": {"file": "warning"}, "emojify": 0}),
        &env(&[]),
    )
    .unwrap();
    assert_eq!(
        (logger.console, logger.file, logger.emojify),
        (Some(Level::Info), Some(Level::Warning), false)
    );
    // Text that names no level turns that output off, as the reference makes no output for it;
    // a level that is not text is refused at the start.
    let off = options::logger(&json!({}), &env(&[("CORE_LOG_LEVEL_FILE", "loud")])).unwrap();
    assert_eq!((off.console, off.file), (Some(Level::Info), None));
    let off = options::logger(&json!({"levels": {"console": "off"}}), &env(&[])).unwrap();
    assert_eq!((off.console, off.file), (None, Some(Level::Debug)));
    assert_eq!(
        options::logger(&json!({"levels": {"file": 7}}), &env(&[]))
            .unwrap_err()
            .key,
        "levels.file"
    );
    let vars = [
        ("CORE_P2P_PORT", "6002"),
        ("CORE_P2P_MIN_NETWORK_REACH", "1"),
        ("CORE_P2P_ALLOW_PRIVATE_IP_RANGES", "true"),
    ];
    let p2p = options::p2p(&json!({"rateLimit": "50"}), &env(&vars)).unwrap();
    assert_eq!(
        (p2p.port, p2p.minimum_network_reach, p2p.rate_limit),
        (6002, 1, 50)
    );
    assert!(p2p.allow_private_ip_ranges);
    let p2p = options::p2p(&json!({"server": {"port": 7002}}), &env(&vars)).unwrap();
    assert_eq!(p2p.port, 7002);
    assert!(
        !options::p2p(
            &json!({}),
            &env(&[("CORE_P2P_ALLOW_PRIVATE_IP_RANGES", "")])
        )
        .unwrap()
        .allow_private_ip_ranges
    );
    let error = options::p2p(&json!({}), &env(&[("CORE_P2P_PORT", "http")])).unwrap_err();
    assert_eq!(
        (error.package, error.key.as_str()),
        ("@solar-network/p2p", "server.port")
    );
}

#[test]
fn the_blockchain_options_are_read_as_the_reference_schema_reads_them() {
    let defaults = options::blockchain(&json!({})).unwrap();
    assert_eq!(defaults, BlockchainOptions::default());
    let read = options::blockchain(&json!({
        "databaseRollback": {"maxBlockRewind": "20", "steps": 5.0},
        "networkStart": "TRUE",
        "missedBlocksLookback": 60,
        "unknown": [1],
    }))
    .unwrap();
    let bounds = read.database_rollback;
    assert_eq!((bounds.max_block_rewind, bounds.steps), (20, 5));
    assert!(read.network_start);
    assert_eq!(read.missed_blocks_lookback, 60);
    let read = options::blockchain(&json!({"networkStart": false, "missedBlocksLookback": "x"}));
    assert_eq!(read.unwrap().missed_blocks_lookback, 2_592_000);
    for text in [" 1e2 ", "+100", "100.", "1.00e+2"] {
        let read = options::blockchain(&json!({"databaseRollback": {"steps": text}}));
        assert_eq!(read.unwrap().database_rollback.steps, 100, "{text}");
    }
    let rewind = "databaseRollback.maxBlockRewind";
    for (entry, key) in [
        (json!({"databaseRollback": null}), "databaseRollback"),
        (json!({"databaseRollback": {"maxBlockRewind": 0}}), rewind),
        (json!({"databaseRollback": {"maxBlockRewind": 1.5}}), rewind),
        (
            json!({"databaseRollback": {"maxBlockRewind": "1x"}}),
            rewind,
        ),
        (
            json!({"databaseRollback": {"maxBlockRewind": "0x10"}}),
            rewind,
        ),
        (
            json!({"databaseRollback": {"maxBlockRewind": true}}),
            rewind,
        ),
        (
            json!({"databaseRollback": {"maxBlockRewind": 4_294_967_296_u64}}),
            rewind,
        ),
        (
            json!({"databaseRollback": {"steps": "0"}}),
            "databaseRollback.steps",
        ),
        (json!({"networkStart": "yes"}), "networkStart"),
    ] {
        let error = options::blockchain(&entry).unwrap_err();
        assert_eq!(
            (error.package, error.key.as_str()),
            (BlockchainOptions::PACKAGE, key),
            "{entry}"
        );
    }
}

#[test]
fn a_replay_marker_restores_by_the_replay_path_and_is_removed() {
    use std::os::unix::fs::PermissionsExt;
    let setup = Setup::new("markers", "");
    let config = load(&setup, "--network testnet", &[]).unwrap();
    // The run directory the process lock makes: the markers' user is its user.
    fs::create_dir_all(config.paths.data.join("run")).unwrap();
    fs::create_dir_all(&config.paths.temp).unwrap();
    let (_, first) = restored(&config).unwrap();
    assert_eq!(first.path, RestorePath::FirstStart);
    let (_, again) = restored(&config).unwrap();
    assert_eq!(again.path, RestorePath::Table);
    fs::write(config.replay_marker(), "").unwrap();
    let (_, replayed) = restored(&config).unwrap();
    assert_eq!(replayed.path, RestorePath::Replay(ReplayReason::Requested));
    assert!(!config.replay_marker().exists());
    // A marker that others may write is not the relay's: ignored, and left.
    fs::write(config.replay_marker(), "").unwrap();
    let wide = fs::Permissions::from_mode(0o666);
    fs::set_permissions(config.replay_marker(), wide).unwrap();
    let (_, ignored) = restored(&config).unwrap();
    assert_eq!(ignored.path, RestorePath::Table);
    assert!(config.replay_marker().exists());
    assert_eq!(
        config.pool_marker(),
        setup.root.join("temp/clear-pool.lock")
    );
}

#[test]
fn the_reset_variables_empty_the_store_and_the_pool_at_the_start() {
    let setup = Setup::new("resets", "");
    let config = load(&setup, "--network testnet", &[]).unwrap();
    fs::create_dir_all(&config.paths.temp).unwrap();
    assert_eq!(restored(&config).unwrap().1.path, RestorePath::FirstStart);
    assert_eq!(restored(&config).unwrap().1.path, RestorePath::Table);
    let reset = load(&setup, "--network testnet", &[("CORE_RESET_DATABASE", "1")]).unwrap();
    assert_eq!(restored(&reset).unwrap().1.path, RestorePath::FirstStart);
    // The pool: a stored transaction stays unless a reset variable is set and not empty.
    let pool_options = |config: &RelayConfig| {
        let entry = config.entry(Package::Pool);
        options::pool(&entry, &config.env, &config.paths.data, 1).unwrap()
    };
    let storage = pool_options(&config).storage;
    let row = StoredTransaction {
        height: 1,
        id: TransactionId::from_array([7; 32]),
        recipient_id: String::new(),
        sender_id: "sender".into(),
        serialised: vec![1, 2, 3],
    };
    for (variable, value, kept) in [
        ("CORE_RESET_POOL", "", true),
        ("CORE_RESET_POOL", "1", false),
        ("CORE_RESET_DATABASE", "false", false),
    ] {
        let mut stored = Storage::open(&storage, None).unwrap();
        stored.flush().unwrap();
        stored.add(&row).unwrap();
        drop(stored);
        let config = load(&setup, "--network testnet", &[(variable, value)]).unwrap();
        drop(open_pool(&config, pool_options(&config)).unwrap());
        let left = Storage::open(&storage, None).unwrap().all().unwrap();
        assert_eq!(left.len(), usize::from(kept), "{variable}={value}");
    }
}

#[test]
fn a_refused_api_option_leaves_the_api_out_and_https_or_an_unknown_host_stops_the_start() {
    let setup = Setup::new("api-options", "");
    let api = |extra: &[(&str, &str)]| api_options(&load(&setup, "--network testnet", extra)?);
    let served = api(&[("CORE_API_PORT", "7003")]).unwrap().unwrap();
    assert_eq!(served.address.port(), 7003);
    // The reference's API is not a required component: its schema's refusal leaves it out.
    assert_eq!(api(&[("CORE_API_PORT", "abc")]), Ok(None));
    for extra in [("CORE_API_SSL", "1"), ("CORE_API_HOST", "unknown.invalid")] {
        assert!(
            matches!(api(&[extra]), Err(RelayError::Config(_))),
            "{extra:?}"
        );
    }
    let mut plugins = relay_plugins();
    plugins
        .as_array_mut()
        .unwrap()
        .retain(|p| p["package"] != "@solar-network/api");
    setup.app(plugins);
    assert_eq!(api(&[("CORE_API_PORT", "abc")]), Ok(None));
}

#[test]
fn the_version_floor_in_force_refuses_the_start_and_a_later_one_warns() {
    let setup = Setup::new("floor", "");
    let text = fs::read_to_string(setup.config().join("crypto/milestones.json")).unwrap();
    let mut milestones: Vec<Value> = serde_json::from_str(&text).unwrap();
    milestones.push(json!({"height": 100, "p2p": {"minimumVersions": [">=4.4.0"]}}));
    fs::write(
        setup.config().join("crypto/milestones.json"),
        Value::Array(milestones).to_string(),
    )
    .unwrap();
    let config = load(&setup, "--network testnet", &[]).unwrap();
    // The corpus's floor, >=4.3.0, admits this node's prerelease of 4.3.1 on a network that is
    // not mainnet; the floor from height 100 does not.
    assert_eq!(version_floor(&config, 1, &[]), Ok(()));
    assert_eq!(version_floor(&config, 99, &[]), Ok(()));
    match version_floor(&config, 100, &[]) {
        Err(RelayError::Config(message)) => assert!(
            message.contains("height 100") && message.contains(">=4.4.0"),
            "{message}"
        ),
        other => panic!("{other:?}"),
    }
}
