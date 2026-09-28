//! The node routes over the committed devnet chain at its tip, pinned to the reference's node
//! controller's answers for the same chain: `/node/status`, `/node/syncing`,
//! `/node/configuration` (the milestone's dynamic fees over the pool's, `removeFalsy`) and
//! `/node/configuration/crypto`.

mod snapshot;

use heartwood_api::context::{NodeInfo, NodeStatus};
use heartwood_api::controllers::node;
use heartwood_crypto::Milestones;
use heartwood_crypto::crypto::slots;
use heartwood_kernel::config::CryptoFiles;
use serde_json::{Value, json};

use snapshot::chain;

fn info(files: &CryptoFiles) -> NodeInfo {
    NodeInfo {
        crypto: files.clone(),
        version: "4.3.1".into(),
    }
}

fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

const TIP_ID: &str = "f41d004098f2f8d90363101e68d06bf088ea3d6c454c90840c2370c41cfe5284";

#[test]
fn status_and_syncing_match_the_reference() {
    let chain = chain();
    let status = NodeStatus {
        synced: true,
        network_height: 270,
    };
    // The reference's answers with a network height of 270, the timestamp aside.
    let epoch_ms = slots::epoch_unix_ms(chain.milestones().at(1).epoch()).unwrap();
    let now_ms = epoch_ms + 68_295_400;
    assert_eq!(
        text(&node::status_data(
            &chain.state,
            status,
            &info(&chain.files),
            now_ms
        )),
        r#"{"synced":true,"now":265,"blocksCount":5,"timestamp":68295}"#
    );
    assert_eq!(
        text(&node::syncing_data(&chain.state, status)),
        format!(r#"{{"syncing":false,"blocks":5,"height":265,"id":"{TIP_ID}"}}"#)
    );
    // Without a network height the difference is 0; below the tip it is negative.
    let alone = NodeStatus::default();
    assert_eq!(node::syncing_data(&chain.state, alone)["blocks"], 0);
    assert_eq!(node::syncing_data(&chain.state, alone)["syncing"], true);
    let behind = NodeStatus {
        synced: false,
        network_height: 260,
    };
    assert_eq!(node::syncing_data(&chain.state, behind)["blocks"], -5);
}

#[test]
fn the_configuration_matches_the_reference() {
    let chain = chain();
    let pool = chain.pool(Vec::new());
    let data = node::configuration_data(&chain.state, &pool.options, &info(&chain.files));
    let keys: Vec<&String> = data.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "core",
            "nethash",
            "slip44",
            "wif",
            "token",
            "symbol",
            "explorer",
            "version",
            "ports",
            "constants",
            "pool"
        ]
    );
    let head = r#"{"core":{"version":"4.3.1"},"nethash":"c9b03ab996ef3ac216a2ac53eaee71118cbf7995fa44449a7fb7f94bbe18bcca","slip44":1,"wif":252,"token":"dROOT","symbol":"dRT","explorer":"","version":90,"ports":{},"constants":{"height":2,"activeDelegates":53,"block":{"version":0,"maxTransactions":150,"maxPayload":2097152},"blocksToRevokeDelegateResignation":106,"blockTime":8,"burn":{"feeBasisPoints":9000,"txAmount":2000000},"epoch":"2026-09-26T17:27:54.000Z","fees":{"staticFees""#;
    assert!(text(&data).starts_with(head), "{}", text(&data));
    // The milestone turns its dynamic fees on: they replace the pool's, minFee as both floors.
    let pool_json = r#"{"dynamicFees":{"enabled":true,"addonBytes":{"burn":0,"delegateRegistration":1214968,"delegateResignation":0,"htlcClaim":0,"htlcLock":82,"htlcRefund":0,"ipfs":98,"legacyTransfer":99,"legacyVote":98,"multiSignature":16,"secondSignature":99,"transfer":85,"vote":98},"minFeeBroadcast":6173,"minFeePool":6173},"maxTransactionsInPool":15000,"maxTransactionsPerSender":150,"maxTransactionsPerRequest":40,"maxTransactionAge":2700,"maxTransactionBytes":2000000}"#;
    assert_eq!(text(&data["pool"]), pool_json);
    // The constants are the tip's milestone without its dynamic fees and without any `false`.
    let constants = &data["constants"];
    assert!(constants.get("dynamicFees").is_none());
    assert_eq!(constants["bip340"], true);
    assert!(!text(constants).contains("false"));
    let tail = r#""secondaryReward":180000000},"bip340":true,"donations":{"dZDbtMv86KMqo8iPMtAJLhoT5BVKfyVokA":{"basisPoints":500,"purpose":"placeholder-1"},"dPiLD2Fi1dnQhnskBkSnDEPraaWtbTcYPj":{"basisPoints":500,"purpose":"placeholder-2"}}}"#;
    assert!(text(constants).ends_with(tail), "{}", text(constants));
    assert_eq!(text(&data).len(), 2340);
}

/// The chain's files with the milestones' dynamic fees `enabled` set to `on`.
fn files_with_milestone_fees(on: bool) -> CryptoFiles {
    let files = chain().files.clone();
    let mut milestones: Value = serde_json::from_str(&files.milestones_text).unwrap();
    for milestone in milestones.as_array_mut().unwrap() {
        if let Some(fees) = milestone.get_mut("dynamicFees") {
            fees["enabled"] = json!(on);
        }
    }
    let text = milestones.to_string();
    CryptoFiles {
        milestones: Milestones::load(&text, &files.network).unwrap(),
        milestones_text: text,
        ..files
    }
}

#[test]
fn the_pool_fees_apply_when_the_milestone_turns_its_own_off() {
    let chain = chain();
    let files = files_with_milestone_fees(false);
    let mut pool = chain.pool(Vec::new()).options;
    let data = node::configuration_data(&chain.state, &pool, &info(&files));
    let fees = &data["pool"]["dynamicFees"];
    assert_eq!(
        fees.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["enabled", "minFeePool", "minFeeBroadcast", "addonBytes"]
    );
    assert_eq!(
        (&fees["minFeePool"], &fees["addonBytes"]["transfer"]),
        (&json!(3000), &json!(500))
    );
    // The pool's fees turned off too: `{enabled: false}`, whose `false` is then removed.
    pool.dynamic_fees.enabled = false;
    let data = node::configuration_data(&chain.state, &pool, &info(&files));
    assert_eq!(data["pool"]["dynamicFees"], json!({}));
}

#[test]
fn the_crypto_configuration_is_what_the_reference_holds() {
    let chain = chain();
    let data = node::crypto_data(&info(&chain.files));
    let keys: Vec<&String> = data.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["network", "exceptions", "milestones", "genesisBlock"]
    );
    let network: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/difftest/corpus/txgen-one-node/crypto/network.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(text(&data["network"]), text(&network));
    assert_eq!(data["exceptions"], json!({}));
    // The genesis block as loaded, each transaction with the `headerType` the reference's
    // serialiser gives it at boot, last; the file's transactions have a version and a type group.
    let mut expected = chain.files.genesis.clone();
    let transactions = expected["transactions"].as_array_mut().unwrap();
    assert_eq!(transactions.len(), 107);
    for transaction in transactions {
        let transaction = transaction.as_object_mut().unwrap();
        assert!(!transaction.contains_key("headerType"));
        transaction.insert("headerType".into(), json!(0));
    }
    assert_eq!(text(&data["genesisBlock"]), text(&expected));
    // The milestones merged cumulatively, in height order: each carries the keys before it.
    let milestones = data["milestones"].as_array().unwrap();
    let merged = chain.milestones().all();
    assert_eq!(milestones.len(), merged.len());
    for (shown, params) in milestones.iter().zip(merged) {
        assert_eq!(shown["height"], params.height());
        assert_eq!(shown["activeDelegates"], 53);
    }
}

#[test]
fn the_genesis_transactions_carry_what_the_serialiser_sets() {
    let genesis = json!({
        "id": "g",
        "transactions": [
            {"type": 6, "version": 3, "typeGroup": 1},
            {"type": 6, "version": 0, "headerType": null, "fee": "0"},
            {"type": 6, "headerType": 1, "typeGroup": null, "version": 2},
            {"version": "", "headerType": false},
        ],
        "height": 1,
    });
    let expected = json!({
        "id": "g",
        "transactions": [
            {"type": 6, "version": 3, "typeGroup": 1, "headerType": 0},
            {"type": 6, "version": 3, "headerType": 0, "fee": "0", "typeGroup": 1},
            {"type": 6, "headerType": 1, "typeGroup": null, "version": 2},
            {"version": 3, "headerType": 0, "typeGroup": 1},
        ],
        "height": 1,
    });
    assert_eq!(text(&node::genesis_as_held(&genesis)), text(&expected));
}

#[tokio::test]
#[ignore = "integration"]
async fn the_node_routes_over_http() {
    let chain = chain();
    let (address, server) = snapshot::serve_chain("node", chain.pool(Vec::new()), Vec::new()).await;
    let info = NodeInfo {
        crypto: chain.files.clone(),
        version: "4.3.1-heartwood.0.1.0".into(),
    };
    let status = NodeStatus {
        synced: true,
        network_height: 270,
    };
    let syncing = snapshot::get(address, "/api/node/syncing").await;
    assert_eq!(
        syncing.body,
        json!({ "data": node::syncing_data(&chain.state, status) })
    );
    let answer = snapshot::get(address, "/api/node/status?anything=1").await;
    assert_eq!(answer.status, 200);
    assert_eq!(answer.body["data"]["blocksCount"], 5);
    let pool = chain.pool(Vec::new());
    let configuration = snapshot::get(address, "/api/node/configuration").await;
    let expected = node::configuration_data(&chain.state, &pool.options, &info);
    assert_eq!(
        text(&configuration.body),
        text(&json!({ "data": expected }))
    );
    assert_eq!(
        configuration.body["data"]["core"]["version"],
        "4.3.1-heartwood.0.1.0"
    );
    let crypto = snapshot::get(address, "/api/node/configuration/crypto").await;
    assert_eq!(
        text(&crypto.body),
        text(&json!({ "data": node::crypto_data(&info) }))
    );
    server.stop().await;
}

/// The answer's first line, or the whole answer when it has no header.
async fn exchange(address: std::net::SocketAddr, request: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    match answer.split_once("\r\n") {
        Some((line, rest)) if !rest.starts_with("\r\n") => line.to_owned(),
        _ => answer,
    }
}

#[test]
#[ignore = "integration"]
fn a_hostile_query_leaves_the_server_answering() {
    // The API's workers have tokio's default stack of 2 MiB.
    let thread = std::thread::Builder::new().stack_size(2 << 20);
    let answers = thread.spawn(|| {
        let mut runtime = tokio::runtime::Builder::new_current_thread();
        let runtime = runtime.enable_all().build().unwrap();
        runtime.block_on(async {
            let pool = chain().pool(Vec::new());
            let (address, server) = snapshot::serve_chain("hostile", pool, Vec::new()).await;
            let get = |target: String| format!("GET {target} HTTP/1.1\r\nhost: node\r\n\r\n");
            let dots = |key: &str, levels: usize| format!("{key}{}=1", ".b".repeat(levels));
            let gaps: String = (1..=998).map(|i| format!("&a.{}=1", i * 1000)).collect();
            let mut answers = Vec::new();
            for target in [
                "/api/node/status".to_owned(),
                format!("/api/nothing?{}", dots("a", 5_000)),
                format!("/api/node/status?{}", dots("a", 8_000)),
                format!("/api/node/status?{}", dots("a", 8_200)),
                format!("/api/transactions?{}", dots("asset", 127)),
                format!("/api/transactions?{}", dots("asset", 5_000)),
                format!("/api/transactions?a=1,2{gaps}"),
                "/api/node/status".to_owned(),
            ] {
                answers.push(exchange(address, get(target)).await);
            }
            server.stop().await;
            answers
        })
    });
    let answers = answers.unwrap().join().unwrap();
    assert_eq!(
        answers,
        [
            "HTTP/1.1 200 OK",
            "HTTP/1.1 404 Not Found",
            "HTTP/1.1 200 OK",
            "HTTP/1.1 400 Bad Request\r\n\r\n",
            "HTTP/1.1 200 OK",
            "HTTP/1.1 500 Internal Server Error",
            "HTTP/1.1 422 Unprocessable Entity",
            "HTTP/1.1 200 OK",
        ]
    );
}
