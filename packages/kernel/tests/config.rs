//! The crypto files: the all-four rule, the strict schema of the milestone, network and
//! exceptions files, and the files of every devnet and corpus, which must load.

#[macro_use]
mod common;

use std::fs;
use std::path::{Path, PathBuf};

use heartwood_kernel::config::crypto::{self, CryptoTexts, FILES};
use heartwood_kernel::config::{CryptoFilesError, KeyProblem, SchemaError};
use serde_json::{Value, json};

/// The crypto directory of the committed one-node corpus.
fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/difftest/corpus/txgen-one-node/crypto")
}

/// The four texts of the committed corpus.
fn devnet() -> CryptoTexts {
    let read = |file| Some(fs::read_to_string(corpus_dir().join(file)).expect("a corpus file"));
    let [network, milestones, genesis, exceptions] = FILES.map(read);
    CryptoTexts {
        network,
        milestones,
        genesis,
        exceptions,
    }
}

fn parsed(text: &Option<String>) -> Value {
    serde_json::from_str(text.as_deref().expect("a text")).expect("JSON")
}

/// The devnet with its milestone file changed by `change`.
fn with_milestones(change: impl FnOnce(&mut Vec<Value>)) -> CryptoTexts {
    let mut texts = devnet();
    let Value::Array(mut milestones) = parsed(&texts.milestones) else {
        panic!("the milestones are an array")
    };
    change(&mut milestones);
    texts.milestones = Some(Value::Array(milestones).to_string());
    texts
}

/// The devnet with `pointer` of its first milestone set to `value`.
fn first_milestone(pointer: &str, value: Value) -> CryptoTexts {
    with_milestones(|milestones| set(&mut milestones[0], pointer, value))
}

/// The devnet with `pointer` of its network file set to `value`.
fn network(pointer: &str, value: Value) -> CryptoTexts {
    let mut texts = devnet();
    let mut file = parsed(&texts.network);
    set(&mut file, pointer, value);
    texts.network = Some(file.to_string());
    texts
}

/// Set the value at the JSON pointer `pointer`, creating the last key.
fn set(target: &mut Value, pointer: &str, value: Value) {
    let (parent, key) = pointer.rsplit_once('/').expect("a pointer");
    let parent = target.pointer_mut(parent).expect("the parent exists");
    parent
        .as_object_mut()
        .expect("the parent is an object")
        .insert(key.to_owned(), value);
}

fn refusal(texts: &CryptoTexts) -> CryptoFilesError {
    crypto::parse(texts).expect_err("the files are refused")
}

fn key_problem(texts: &CryptoTexts) -> (&'static str, Option<u64>, String, KeyProblem) {
    match refusal(texts) {
        CryptoFilesError::Schema(SchemaError::Key {
            file,
            height,
            path,
            problem,
        }) => (file, height, path, problem),
        other => panic!("not a key refusal: {other:?}"),
    }
}

#[test]
fn the_devnet_and_corpus_files_load() {
    let files = crypto::load(&corpus_dir()).expect("the corpus's files load");
    assert_eq!(files.network.symbol, "dRT");
    assert_eq!(files.milestones.all().len(), 2);
    assert_eq!(files.genesis["height"], 1);
    assert_eq!(files, crypto::parse(&devnet()).expect("the texts load"));
    // Corpora kept outside the repository, when this checkout has them.
    let local = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus");
    for entry in fs::read_dir(local).into_iter().flatten().flatten() {
        let dir = entry.path().join("crypto");
        if dir.is_dir() {
            crypto::load(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        }
    }
}

#[test]
fn every_generated_chain_definition_loads() {
    let file = common::VectorFile::parse(
        "V10-genesis",
        include_str!("../../crypto/tests/vectors/V10-genesis.jsonl"),
    );
    assert!(!file.records.is_empty());
    for record in &file.records {
        let files = &record.expected.as_ref().expect("a chain definition")["files"];
        let text = |name: &str| files[format!("crypto/{name}")].as_str().map(str::to_owned);
        let [network, milestones, genesis, exceptions] = FILES.map(text);
        let texts = CryptoTexts {
            network,
            milestones,
            genesis,
            exceptions,
        };
        crypto::parse(&texts).unwrap_or_else(|e| panic!("{}: {e}", record.label("V10")));
    }
}

#[test]
fn all_four_files_are_required() {
    let mut texts = devnet();
    texts.genesis = None;
    let error = refusal(&texts);
    assert_eq!(
        error,
        CryptoFilesError::Missing {
            files: vec!["genesisBlock.json"]
        }
    );
    assert!(error.to_string().contains("genesisBlock.json"), "{error}");
    texts.network = None;
    texts.exceptions = None;
    assert_eq!(
        refusal(&texts),
        CryptoFilesError::Missing {
            files: vec!["network.json", "genesisBlock.json", "exceptions.json"]
        }
    );
    let empty =
        std::env::temp_dir().join(format!("heartwood-kernel-config-{}", std::process::id()));
    fs::create_dir_all(&empty).expect("a directory");
    assert_eq!(
        crypto::load(&empty),
        Err(CryptoFilesError::Missing {
            files: FILES.to_vec()
        })
    );
    fs::remove_dir_all(&empty).expect("removed");
}

#[test]
fn each_file_must_be_json() {
    for (index, file) in FILES.into_iter().enumerate() {
        let mut texts = devnet();
        let slot = [
            &mut texts.network,
            &mut texts.milestones,
            &mut texts.genesis,
            &mut texts.exceptions,
        ]
        .into_iter()
        .nth(index)
        .expect("four files");
        *slot = Some("{".into());
        match refusal(&texts) {
            CryptoFilesError::Json { file: named, .. } => assert_eq!(named, file),
            other => panic!("{file}: {other:?}"),
        }
    }
}

#[test]
fn unknown_keys_are_refused_at_every_level() {
    let cases = [
        ("/aip11", json!(true), "aip11"),
        ("/vendorFieldLength", json!(255), "vendorFieldLength"),
        ("/block/idFullSha256", json!(true), "block.idFullSha256"),
        ("/burn/feePercent", json!(90), "burn.feePercent"),
        (
            "/dynamicFees/addonBytes/payment",
            json!(1),
            "dynamicFees.addonBytes.payment",
        ),
        (
            "/fees/staticFees/entity",
            json!(1),
            "fees.staticFees.entity",
        ),
        ("/fees/dynamic", json!({}), "fees.dynamic"),
        ("/p2p/maxPeers", json!(10), "p2p.maxPeers"),
        ("/transfer/max", json!(1), "transfer.max"),
        ("/dynamicReward/tiers", json!([]), "dynamicReward.tiers"),
    ];
    for (pointer, value, path) in cases {
        let (file, height, named, problem) = key_problem(&first_milestone(pointer, value));
        assert_eq!(
            (file, height, named.as_str()),
            ("milestones.json", Some(1), path)
        );
        assert_eq!(problem, KeyProblem::Unknown, "{path}");
    }
    let error = refusal(&first_milestone("/aip11", json!(true)));
    assert_eq!(
        error.to_string(),
        "milestones.json at height 1: aip11 is not a known key"
    );
    // A later milestone names its own height.
    let later =
        with_milestones(|milestones| set(&mut milestones[1], "/multiPaymentLimit", json!(64)));
    assert_eq!(
        key_problem(&later),
        (
            "milestones.json",
            Some(2),
            "multiPaymentLimit".into(),
            KeyProblem::Unknown
        )
    );
    // Every type key, the removed types' included, is a fee table key.
    for key in [
        "legacyTransfer",
        "ipfs",
        "htlcRefund",
        "multiSignature",
        "burn",
        "vote",
    ] {
        crypto::parse(&first_milestone(
            &format!("/fees/staticFees/{key}"),
            json!(0),
        ))
        .unwrap_or_else(|e| panic!("{key}: {e}"));
    }
    for (pointer, path) in [
        ("/wallet", "wallet"),
        ("/client/url", "client.url"),
        ("/bip32/seed", "bip32.seed"),
    ] {
        let (file, height, named, problem) = key_problem(&network(pointer, json!("x")));
        assert_eq!(
            (file, height, named.as_str(), problem),
            ("network.json", None, path, KeyProblem::Unknown)
        );
    }
}

#[test]
fn nulls_are_refused_at_any_depth() {
    let cases = [
        ("/reward", json!(null), "reward"),
        ("/dynamicFees", json!(null), "dynamicFees"),
        ("/dynamicFees/minFee", json!(null), "dynamicFees.minFee"),
        (
            "/fees/staticFees/transfer",
            json!(null),
            "fees.staticFees.transfer",
        ),
        (
            "/dynamicReward/enabled",
            json!({"on": null}),
            "dynamicReward.enabled.on",
        ),
        ("/epoch", json!([1, null]), "epoch.1"),
        ("/donations", json!(null), "donations"),
        ("/legacyVote", json!(null), "legacyVote"),
        (
            "/blocksToRevokeDelegateResignation",
            json!(null),
            "blocksToRevokeDelegateResignation",
        ),
    ];
    for (pointer, value, path) in cases {
        let (_, _, named, problem) = key_problem(&first_milestone(pointer, value));
        assert_eq!((named.as_str(), problem), (path, KeyProblem::Null));
    }
    let ranks = with_milestones(|milestones| {
        set(&mut milestones[0], "/dynamicReward/ranks/7", json!(null))
    });
    assert_eq!(key_problem(&ranks).2, "dynamicReward.ranks.7");
    let (file, _, path, problem) = key_problem(&network("/client/explorer", json!(null)));
    assert_eq!(
        (file, path.as_str(), problem),
        ("network.json", "client.explorer", KeyProblem::Null)
    );
    assert_eq!(
        refusal(&network("/client/explorer", json!(null))).to_string(),
        "network.json: client.explorer is null"
    );
}

#[test]
fn values_of_the_wrong_type_are_refused() {
    let cases = [
        ("/fees", json!(5), "fees", "an object"),
        ("/p2p", json!([]), "p2p", "an object"),
        ("/dynamicReward", json!(true), "dynamicReward", "an object"),
        ("/block", json!("block"), "block", "an object"),
        (
            "/p2p/minimumVersions",
            json!(">=4.3.0"),
            "p2p.minimumVersions",
            "an array of strings",
        ),
        (
            "/p2p/minimumVersions",
            json!([">=4.3.0", 4]),
            "p2p.minimumVersions",
            "an array of strings",
        ),
        (
            "/p2p/minimumVersions",
            json!([null]),
            "p2p.minimumVersions",
            "an array of strings",
        ),
        (
            "/fees/staticFees/transfer",
            json!(-1),
            "fees.staticFees.transfer",
            "an integer from 0 to 2^53 - 1",
        ),
        (
            "/fees/staticFees/transfer",
            json!("50000000"),
            "fees.staticFees.transfer",
            "an integer from 0 to 2^53 - 1",
        ),
        (
            "/fees/staticFees/vote",
            json!(1.5),
            "fees.staticFees.vote",
            "an integer from 0 to 2^53 - 1",
        ),
        (
            "/dynamicFees/addonBytes/transfer",
            json!(-85),
            "dynamicFees.addonBytes.transfer",
            "an integer from 0 to 2^53 - 1",
        ),
        (
            "/dynamicFees/addonBytes/vote",
            json!(9_007_199_254_740_992_u64),
            "dynamicFees.addonBytes.vote",
            "an integer from 0 to 2^53 - 1",
        ),
    ];
    for (pointer, value, path, expected) in cases {
        let (_, _, named, problem) = key_problem(&first_milestone(pointer, value));
        assert_eq!(
            (named.as_str(), problem),
            (path, KeyProblem::Type(expected)),
            "{pointer}"
        );
    }
    assert_eq!(
        refusal(&first_milestone("/fees", json!(5))).to_string(),
        "milestones.json at height 1: fees must be an object"
    );
    // Integers in any JSON form that reads as one are fine, as are the free-form maps.
    crypto::parse(&first_milestone("/fees/staticFees/transfer", json!(5.0e7))).expect("5.0e7");
    crypto::parse(&first_milestone("/p2p/minimumVersions", json!([]))).expect("no floor");
    crypto::parse(&first_milestone("/dynamicFees/addonBytes", json!({}))).expect("no addons");
    // The network file: its values are the crypto crate's loader's to check.
    match refusal(&network("/pubKeyHash", json!(256))) {
        CryptoFilesError::Config { file, .. } => assert_eq!(file, "network.json"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_height_may_appear_once() {
    let twice = with_milestones(|milestones| {
        let copy = milestones[1].clone();
        milestones.push(copy);
    });
    assert_eq!(
        refusal(&twice),
        CryptoFilesError::Schema(SchemaError::DuplicateHeight { height: 2 })
    );
    assert_eq!(
        refusal(&twice).to_string(),
        "milestones.json: two milestones at height 2"
    );
    // The same height written as another JSON number is the same height.
    let spelled = with_milestones(|milestones| milestones.push(json!({"height": 2.0})));
    assert_eq!(
        refusal(&spelled),
        CryptoFilesError::Schema(SchemaError::DuplicateHeight { height: 2 })
    );
    let first = with_milestones(|milestones| milestones.push(json!({"height": 1})));
    assert_eq!(
        refusal(&first),
        CryptoFilesError::Schema(SchemaError::DuplicateHeight { height: 1 })
    );
}

#[test]
fn removed_features_are_accepted_only_off() {
    let off = [
        ("legacyTransfer", json!(false)),
        ("legacyVote", json!(false)),
        ("bip340", json!(true)),
        ("acceptLegacySchnorrTransactions", json!(false)),
        ("htlcEnabled", json!(false)),
    ];
    for (key, value) in off {
        crypto::parse(&first_milestone(&format!("/{key}"), value.clone()))
            .unwrap_or_else(|e| panic!("{key}: {e}"));
        let later =
            with_milestones(|milestones| set(&mut milestones[1], &format!("/{key}"), value));
        crypto::parse(&later).unwrap_or_else(|e| panic!("{key} later: {e}"));
    }
    let on = [
        ("legacyTransfer", json!(true)),
        ("legacyVote", json!(true)),
        ("bip340", json!(false)),
        ("acceptLegacySchnorrTransactions", json!(true)),
        ("htlcEnabled", json!(true)),
        ("legacyTransfer", json!(0)),
        ("bip340", json!(1)),
        ("htlcEnabled", json!("false")),
    ];
    for (key, value) in on {
        let later =
            with_milestones(|milestones| set(&mut milestones[1], &format!("/{key}"), value));
        let (file, height, path, problem) = key_problem(&later);
        assert_eq!(
            (file, height, path.as_str(), problem),
            ("milestones.json", Some(2), key, KeyProblem::RemovedFeature)
        );
    }
    assert_eq!(
        refusal(&first_milestone("/htlcEnabled", json!(true))).to_string(),
        "milestones.json at height 1: htlcEnabled turns on a removed feature"
    );
}

#[test]
fn exceptions_must_be_an_empty_object() {
    for text in [
        "{\"transactions\": []}",
        "[]",
        "null",
        "0",
        "{\"blocks\": null}",
    ] {
        let mut texts = devnet();
        texts.exceptions = Some(text.to_owned());
        let error = refusal(&texts);
        assert_eq!(
            error,
            CryptoFilesError::Schema(SchemaError::Exceptions),
            "{text}"
        );
        assert!(error.to_string().contains("exceptions.json"));
    }
    let mut texts = devnet();
    texts.exceptions = Some(" { } \n".into());
    crypto::parse(&texts).expect("an empty object");
}

#[test]
fn the_crypto_crate_loaders_run_after_the_schema() {
    // A value the strict schema leaves to the milestone loader.
    match refusal(&first_milestone("/blockTime", json!("8"))) {
        CryptoFilesError::Config { file, source } => {
            assert_eq!(file, "milestones.json");
            assert!(source.to_string().contains("blockTime"), "{source}");
        }
        other => panic!("{other:?}"),
    }
    // A milestone without a usable height is the loader's to refuse.
    let no_height = with_milestones(|milestones| milestones.push(json!({"reward": 0})));
    assert!(matches!(
        refusal(&no_height),
        CryptoFilesError::Config {
            file: "milestones.json",
            ..
        }
    ));
    // The schema runs first: a file with an unknown key and a bad value names the key.
    let both = with_milestones(|milestones| {
        set(&mut milestones[0], "/blockTime", json!("8"));
        set(&mut milestones[0], "/aip11", json!(true));
    });
    assert_eq!(key_problem(&both).3, KeyProblem::Unknown);
    // A network file of another shape is the network loader's to refuse.
    let mut texts = devnet();
    texts.network = Some("[]".into());
    assert!(matches!(
        refusal(&texts),
        CryptoFilesError::Config {
            file: "network.json",
            ..
        }
    ));
}
