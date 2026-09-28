//! `network generate`: the chain definition byte-identical to the genesis vectors (`V10-genesis`)
//! for their three seeds, the key files and their modes, the single-node instance, `--force` and
//! `--chain-only`, the epoch from the clock, the profile's overrides, the refusals, and the
//! `ChainGenerator` seam that `devnet init` calls.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use heartwood_cli::NetworkError;
use heartwood_cli::network::{DEVNET_PROFILE, NetworkGenerator, generate};
use heartwood_cli::seams::{ChainGenerator, GenerateOptions, Profile};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::genesis::derive;
use heartwood_crypto::utils::hex;
use heartwood_kernel::ManualClock;
use serde_json::{Value, json};

const EPOCH: &str = "2026-01-01T00:00:00.000Z";

/// The genesis vectors' records, after the meta record.
fn records() -> Vec<Value> {
    let text = include_str!("../../crypto/tests/vectors/V10-genesis.jsonl");
    let lines = text.lines().filter(|line| !line.trim().is_empty()).skip(1);
    lines
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// A fresh directory for one test, under the target's temporary directory.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-network")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn options(seed: &str, out: &Path) -> GenerateOptions {
    GenerateOptions {
        network: "devnet".into(),
        seed: seed.into(),
        epoch: Some(EPOCH.into()),
        profile: Profile::Devnet,
        pub_key_hash: None,
        token: None,
        symbol: None,
        explorer: None,
        out: out.to_path_buf(),
        chain_only: false,
        force: false,
    }
}

fn clock() -> ManualClock {
    ManualClock::new(1_790_000_123_456)
}

fn json_file(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_built_in_profile_is_the_genesis_vectors_profile() {
    let profile: Value = serde_json::from_str(DEVNET_PROFILE).unwrap();
    for record in records() {
        assert_eq!(record["input"]["profile"], profile);
    }
}

#[test]
fn every_genesis_vector_is_written_byte_for_byte() {
    let records = records();
    let seeds: Vec<&str> = records
        .iter()
        .map(|r| r["input"]["seed"].as_str().unwrap())
        .collect();
    assert_eq!(seeds.len(), 3);
    assert!(seeds.contains(&"solar-devnet") && seeds.contains(&"difftest-seed-2"));
    for (k, record) in records.iter().enumerate() {
        let (input, expected) = (&record["input"], &record["output"]);
        assert_eq!(input["epoch"], EPOCH);
        let out = scratch(&format!("vector-{k}"));
        let generated = generate(&options(seeds[k], &out), &clock()).unwrap();
        let texts = expected["files"].as_object().unwrap();
        assert_eq!(texts.len(), 4);
        for (i, (file, text)) in texts.iter().enumerate() {
            let bytes = fs::read(out.join(file)).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&bytes),
                text.as_str().unwrap(),
                "{file}"
            );
            let digest = &expected["fileSha256"][file];
            assert_eq!(hex::encode(&sha256(&bytes)), *digest, "{} {file}", seeds[k]);
            assert_eq!(generated.files[i], Path::new(file));
        }
        assert_eq!(generated.nethash, expected["nethash"]);
        assert_eq!(generated.genesis_id, expected["genesisId"]);
        assert_eq!(generated.epoch, EPOCH);
        let block = json_file(&out.join("crypto/genesisBlock.json"));
        assert_eq!(block["id"], expected["genesisId"]);
        assert_eq!(
            block["transactions"].as_array().unwrap().len(),
            expected["transactions"].as_u64().unwrap() as usize
        );

        let wallets = json_file(&out.join("genesis-wallet.json"));
        assert_eq!(wallets["generator"], expected["generator"]);
        assert_eq!(wallets["genesis"], expected["genesisWallets"]);
        assert_eq!(wallets["donations"], expected["donationWallets"]);
        assert_eq!(wallets["wallets"], expected["testWallets"]);
        assert_eq!(wallets["delegates"], expected["delegates"]);
        assert_eq!(wallets["generator"]["issued"], expected["issued"]);

        let delegates = expected["delegates"].as_array().unwrap();
        let keys: Vec<&Value> = delegates.iter().map(|d| &d["privateKey"]).collect();
        let key_file = json_file(&out.join("delegates.json"));
        assert_eq!(key_file, json!({ "keys": keys }));
        assert_eq!(generated.delegates.len(), 53);
        for (ours, theirs) in generated.delegates.iter().zip(delegates) {
            assert_eq!(ours.username, theirs["username"]);
            assert_eq!(ours.public_key, theirs["publicKey"]);
            assert_eq!(ours.secret_key.as_str(), theirs["privateKey"]);
        }
        let aux = derive::aux("solar-devnet-v1/", "forger", seeds[k]);
        assert_eq!(generated.forger_aux, aux);
    }
}

#[test]
fn the_keys_are_private_and_the_single_node_instance_is_complete() {
    let out = scratch("instance");
    let generated = generate(&options("solar-devnet", &out), &clock()).unwrap();
    let names: Vec<String> = generated
        .files
        .iter()
        .map(|f| f.display().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "crypto/network.json",
            "crypto/milestones.json",
            "crypto/genesisBlock.json",
            "crypto/exceptions.json",
            "genesis-wallet.json",
            "delegates.json",
            ".env",
            "app.json",
            "peers.json",
            "config.json",
        ]
    );
    assert_eq!(generated.out, out);
    for file in &generated.files {
        let secret = ["genesis-wallet.json", "delegates.json"].contains(&file.to_str().unwrap());
        assert_eq!(
            mode(&out.join(file)),
            if secret { 0o600 } else { 0o644 },
            "{file:?}"
        );
    }
    let env =
        heartwood_kernel::config::dot_env::parse(&fs::read_to_string(out.join(".env")).unwrap());
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| (k.clone(), format!("{v:?}")))
        .collect();
    assert_eq!(
        env,
        [
            ("CORE_LOG_LEVEL", "Text(\"info\")"),
            ("CORE_LOG_LEVEL_FILE", "Text(\"info\")"),
            ("CORE_P2P_HOST", "Text(\"127.0.0.1\")"),
            ("CORE_P2P_PORT", "Number(6002.0)"),
            ("CORE_P2P_MIN_NETWORK_REACH", "Number(0.0)"),
            ("CORE_API_HOST", "Text(\"127.0.0.1\")"),
            ("CORE_API_PORT", "Number(6003.0)"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
    );
    let app = json_file(&out.join("app.json"));
    let packages = |role: &str| -> Vec<String> {
        app[role]["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                p["package"]
                    .as_str()
                    .unwrap()
                    .replace("@solar-network/", "")
            })
            .collect()
    };
    assert_eq!(
        packages("relay"),
        [
            "logger",
            "state",
            "database",
            "transactions",
            "pool",
            "p2p",
            "blockchain",
            "api"
        ]
    );
    assert_eq!(packages("forger"), ["logger", "transactions", "forger"]);
    assert_eq!(
        app["relay"]["plugins"][5]["options"]["remoteAccess"][0],
        "127.0.0.1"
    );
    let forger = &app["forger"]["plugins"][2]["options"];
    assert_eq!(
        forger["hosts"],
        json!([{ "hostname": "127.0.0.1", "port": 6002 }])
    );
    assert_eq!(forger["aux"], hex::encode(&generated.forger_aux));
    assert_eq!(
        json_file(&out.join("peers.json")),
        json!({ "list": [{ "ip": "127.0.0.1", "port": 6002 }] })
    );
    assert_eq!(
        json_file(&out.join("config.json")),
        json!({ "token": "solar", "network": "devnet" })
    );
    let text = fs::read_to_string(out.join("config.json")).unwrap();
    assert_eq!(
        text,
        "{\n    \"token\": \"solar\",\n    \"network\": \"devnet\"\n}\n"
    );
}

#[test]
fn the_generated_app_json_loads_for_both_roles() {
    use heartwood_kernel::config::app::{Role, parse};
    let out = scratch("app-json-loads");
    generate(&options("solar-devnet", &out), &clock()).unwrap();
    let text = fs::read_to_string(out.join("app.json")).unwrap();
    for role in [Role::Relay, Role::Forger] {
        let app = parse(&text, role).unwrap_or_else(|error| panic!("{role:?}: {error}"));
        assert!(!app.plugins.is_empty(), "{role:?}");
    }
}

#[test]
fn a_directory_that_is_not_empty_needs_force_which_overwrites_only_the_files_written() {
    let out = scratch("force");
    fs::write(out.join("notes.txt"), "mine").unwrap();
    let mut options = options("solar-devnet", &out);
    options.chain_only = true;
    let refused = generate(&options, &clock());
    assert!(matches!(refused, Err(NetworkError::Exists { path }) if path == out));
    assert!(!out.join("crypto").exists());

    options.force = true;
    fs::write(out.join("delegates.json"), "{}").unwrap();
    fs::set_permissions(
        out.join("delegates.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let first = generate(&options, &clock()).unwrap();
    assert_eq!(mode(&out.join("delegates.json")), 0o600);
    assert_eq!(fs::read_to_string(out.join("notes.txt")).unwrap(), "mine");
    let bytes = fs::read(out.join("crypto/genesisBlock.json")).unwrap();
    let second = generate(&options, &clock()).unwrap();
    assert_eq!(
        fs::read(out.join("crypto/genesisBlock.json")).unwrap(),
        bytes
    );
    assert_eq!(first.files, second.files);
}

#[test]
fn chain_only_writes_the_chain_definition_and_the_keys_alone() {
    let out = scratch("chain-only");
    let mut options = options("solar-devnet", &out);
    options.chain_only = true;
    let generated = generate(&options, &clock()).unwrap();
    assert_eq!(generated.files.len(), 6);
    for file in [".env", "app.json", "peers.json", "config.json"] {
        assert!(!out.join(file).exists(), "{file}");
    }
    assert!(out.join("delegates.json").exists());
}

#[test]
fn without_an_epoch_the_clock_gives_its_current_second_and_nothing_else_changes() {
    let out = scratch("clock-epoch");
    let mut options = options("solar-devnet", &out);
    options.epoch = None;
    let generated = generate(&options, &clock()).unwrap();
    assert_eq!(generated.epoch, "2026-09-21T14:15:23.000Z");
    let milestones = json_file(&out.join("crypto/milestones.json"));
    assert_eq!(milestones[0]["epoch"], "2026-09-21T14:15:23.000Z");
    let record = &records()[0]["output"];
    assert_eq!(generated.nethash, record["nethash"]);
    assert_eq!(generated.genesis_id, record["genesisId"]);

    // An epoch without milliseconds is read as the whole second it names.
    let out = scratch("short-epoch");
    let mut short = chain_only(&out);
    short.epoch = Some("2026-01-01T00:00:00Z".into());
    assert_eq!(generate(&short, &clock()).unwrap().epoch, EPOCH);
}

fn chain_only(out: &Path) -> GenerateOptions {
    let mut options = options("solar-devnet", out);
    options.chain_only = true;
    options
}

#[test]
fn the_network_flags_replace_the_profile_values() {
    let out = scratch("overrides");
    let mut options = options("solar-devnet", &out);
    options.network = "iceroot".into();
    options.pub_key_hash = Some(30);
    options.token = Some("ROOT".into());
    options.symbol = Some("RT".into());
    options.explorer = Some("https://explorer.invalid".into());
    let generated = generate(&options, &clock()).unwrap();
    let network = json_file(&out.join("crypto/network.json"));
    assert_eq!(network["name"], "iceroot");
    assert_eq!(network["pubKeyHash"], 30);
    assert_eq!(network["addressCharacter"], "D");
    assert_eq!(
        network["client"],
        json!({ "token": "ROOT", "symbol": "RT", "explorer": "https://explorer.invalid" })
    );
    assert_eq!(network["nethash"], generated.nethash);
    assert_ne!(generated.nethash, records()[0]["output"]["nethash"]);
    assert_eq!(json_file(&out.join("config.json"))["network"], "iceroot");
}

#[test]
fn inputs_that_cannot_build_a_chain_are_refused() {
    let out = scratch("refusals");
    let mut bad_epoch = chain_only(&out.join("epoch"));
    bad_epoch.epoch = Some("2026-01-01".into());
    let refused = generate(&bad_epoch, &clock());
    assert!(matches!(&refused, Err(NetworkError::Epoch { epoch }) if epoch == "2026-01-01"));

    let mut empty_seed = chain_only(&out.join("seed"));
    empty_seed.seed = String::new();
    assert!(matches!(
        generate(&empty_seed, &clock()),
        Err(NetworkError::Genesis(_))
    ));

    let profile: Value = serde_json::from_str(DEVNET_PROFILE).unwrap();
    let changed = |edit: &dyn Fn(&mut Value)| {
        let mut profile = profile.clone();
        edit(&mut profile);
        let file = out.join("profile.json");
        fs::write(&file, profile.to_string()).unwrap();
        let mut options = chain_only(&out.join("custom"));
        options.profile = Profile::File(file);
        options.force = true;
        generate(&options, &clock())
    };
    assert!(changed(&|_| {}).is_ok());
    let edits: [&dyn Fn(&mut Value); 5] = [
        &|p| p["supply"] = json!("10000000000000001"),
        &|p| p["allocations"][0]["amount"] = json!("03400000000000000"),
        &|p| p["network"] = json!("devnet"),
        &|p| p["rules"] = json!([]),
        &|p| p["delegates"]["count"] = json!(-1),
    ];
    for edit in edits {
        assert!(
            matches!(changed(edit), Err(NetworkError::Profile { .. })),
            "{:?}",
            changed(edit)
        );
    }
    let mut missing = chain_only(&out.join("missing"));
    missing.profile = Profile::File(out.join("no-such-profile.json"));
    assert!(matches!(
        generate(&missing, &clock()),
        Err(NetworkError::Profile { .. })
    ));
}

#[test]
fn devnet_init_reaches_the_generator_through_its_seam() {
    let direct = scratch("seam-direct");
    let seam = scratch("seam");
    let generator: &dyn ChainGenerator = &NetworkGenerator;
    let through = generator.generate(&chain_only(&seam), &clock()).unwrap();
    let plain = generate(&chain_only(&direct), &clock()).unwrap();
    assert_eq!(through.files, plain.files);
    assert_eq!(through.nethash, plain.nethash);
    for file in &plain.files {
        assert_eq!(
            fs::read(seam.join(file)).unwrap(),
            fs::read(direct.join(file)).unwrap()
        );
    }
}

/// Writes `heartwood network generate`'s files for the seed and epoch the environment names into
/// `HEARTWOOD_GENERATE_OUT`, so that the devnet tooling can boot a reference node from them.
/// Without it, the files go to a scratch directory with the tests' epoch.
#[test]
#[ignore = "devnet"]
fn generate_for_the_reference_devnet() {
    let out = std::env::var("HEARTWOOD_GENERATE_OUT")
        .map_or_else(|_| scratch("reference-devnet"), PathBuf::from);
    let mut options = options(
        &std::env::var("HEARTWOOD_GENERATE_SEED").unwrap_or_else(|_| "solar-devnet".into()),
        &out,
    );
    if let Ok(epoch) = std::env::var("HEARTWOOD_GENERATE_EPOCH") {
        options.epoch = Some(epoch);
    }
    let generated = generate(&options, &clock()).unwrap();
    println!(
        "generate_for_the_reference_devnet: nethash {} genesis id {} epoch {}",
        generated.nethash, generated.genesis_id, generated.epoch
    );
}
