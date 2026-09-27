//! The pool routes' shapes and the static transaction routes over the committed devnet chain,
//! pinned to the reference's answers for the same chain: a transaction of each kept type as the
//! pool routes show it (transformed, and as its data by default for one transaction), a confirmed
//! one with its block, `/transactions/types`, `/transactions/fees`, and the answer of
//! `POST /transactions`.

mod snapshot;

use std::sync::Arc;

use heartwood_api::controllers::transactions_pool;
use heartwood_api::resources::{self, TransactionBlock};
use heartwood_crypto::Transaction;
use heartwood_pool::{PoolCode, ProcessorResult, Refusal};
use indexmap::IndexMap;
use serde_json::{Value, json};

use snapshot::chain;

fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

fn transaction(id: &str) -> Arc<Transaction> {
    let all = chain().transactions();
    all.into_iter()
        .find(|transaction| transaction.id().to_hex() == id)
        .unwrap()
}

/// The reference's transformed and untransformed shapes of one transaction of each kept type but
/// the transfer (below), as its pool routes show them.
const SHAPES: [(&str, &str, &str); 5] = [
    (
        "90cc59708b192a718c4c4b63ec3a42cc01b42e845587d0ef9fae311d82d836f9",
        r#"{"id":"90cc59708b192a718c4c4b63ec3a42cc01b42e845587d0ef9fae311d82d836f9","version":3,"type":0,"typeGroup":2,"amount":"100000000000","fee":"10000000","sender":"dQ7BfoVGndGzTK8Bjb4Kqc6G5GaVXU7jTK","senderPublicKey":"029f8dc2a4002dea6a8cf685854a3fa109d38503cddf71a591b55a7190b44f2871","signature":"a41263b45deb2f3b51720c74b9470c51558546900a7f8b07cdfb58d7811be3055d015ea8cd16efaf0e13f2cfbf33d06858df383b5b23fc2dc9d8a0246bc88eb0","confirmations":0,"nonce":"1"}"#,
        r#"{"headerType":0,"version":3,"network":90,"typeGroup":2,"type":0,"nonce":"1","senderPublicKey":"029f8dc2a4002dea6a8cf685854a3fa109d38503cddf71a591b55a7190b44f2871","senderId":"dQ7BfoVGndGzTK8Bjb4Kqc6G5GaVXU7jTK","fee":"10000000","amount":"100000000000","signature":"a41263b45deb2f3b51720c74b9470c51558546900a7f8b07cdfb58d7811be3055d015ea8cd16efaf0e13f2cfbf33d06858df383b5b23fc2dc9d8a0246bc88eb0","id":"90cc59708b192a718c4c4b63ec3a42cc01b42e845587d0ef9fae311d82d836f9"}"#,
    ),
    (
        "87c752c1970b89b929f94b538c04cde4a28ba2ea0374b62c0d2f5dc7fb95d021",
        r#"{"id":"87c752c1970b89b929f94b538c04cde4a28ba2ea0374b62c0d2f5dc7fb95d021","version":3,"type":2,"typeGroup":2,"fee":"9000000","sender":"dPPBDPkvZsz1xs52r1cToidEbdmzbYpCSB","senderPublicKey":"02a109e32065ed1f48b49723f7e3d844dc84c2dc2f7fdba7cd2aecfc578fca10af","signature":"0ab91359ec5457e60e184e51181945c9bd62227ebfb1a5d7d360727459e71f9935597e979e5cda94fc8b0d7333c34b72baeb2ad7dd6f348bf94d0ac714b85742","asset":{"votes":{"genesis_5":60,"genesis_9":40}},"confirmations":0,"nonce":"2"}"#,
        r#"{"headerType":0,"version":3,"network":90,"typeGroup":2,"type":2,"nonce":"2","senderPublicKey":"02a109e32065ed1f48b49723f7e3d844dc84c2dc2f7fdba7cd2aecfc578fca10af","senderId":"dPPBDPkvZsz1xs52r1cToidEbdmzbYpCSB","fee":"9000000","asset":{"votes":{"genesis_5":60,"genesis_9":40}},"signature":"0ab91359ec5457e60e184e51181945c9bd62227ebfb1a5d7d360727459e71f9935597e979e5cda94fc8b0d7333c34b72baeb2ad7dd6f348bf94d0ac714b85742","id":"87c752c1970b89b929f94b538c04cde4a28ba2ea0374b62c0d2f5dc7fb95d021"}"#,
    ),
    (
        "a4bd4da989d2f8fab39aa45e80a85ff406dfad2310d100149875b6713e30d1c2",
        r#"{"id":"a4bd4da989d2f8fab39aa45e80a85ff406dfad2310d100149875b6713e30d1c2","version":3,"type":2,"typeGroup":1,"fee":"7500404882","sender":"dEmE3wAHWfpWxvnTMEMmXcB1rCUds6GCA8","senderPublicKey":"025662ee80f6e0a0e71c57c359e5692d4460cbbdb69b3408445c7776ba815f04c7","signature":"40f1372ea1297bab12dc33a92f55adf749e19acd59785ec4ed569bcc9ecce330da220c3f582887da7a9edcdabe8989e822e7703fb8ef16f795b779eacd2f7f5b","asset":{"delegate":{"username":"tx1n1160"}},"confirmations":0,"nonce":"1"}"#,
        r#"{"headerType":0,"version":3,"network":90,"typeGroup":1,"type":2,"nonce":"1","senderPublicKey":"025662ee80f6e0a0e71c57c359e5692d4460cbbdb69b3408445c7776ba815f04c7","senderId":"dEmE3wAHWfpWxvnTMEMmXcB1rCUds6GCA8","fee":"7500404882","asset":{"delegate":{"username":"tx1n1160"}},"signature":"40f1372ea1297bab12dc33a92f55adf749e19acd59785ec4ed569bcc9ecce330da220c3f582887da7a9edcdabe8989e822e7703fb8ef16f795b779eacd2f7f5b","id":"a4bd4da989d2f8fab39aa45e80a85ff406dfad2310d100149875b6713e30d1c2"}"#,
    ),
    (
        "47ceaf1ece83b07f866d1f2f4a1fa5e03c673d3447baa0402f58f10b986963c3",
        r#"{"id":"47ceaf1ece83b07f866d1f2f4a1fa5e03c673d3447baa0402f58f10b986963c3","version":3,"type":1,"typeGroup":1,"fee":"5000000","sender":"dDsmrDBibyfMtDreqHVTNix1sixL74Km2P","senderPublicKey":"025e1a3fea8d4763b6898ced2e356047d334f11cf3c799ea22b02848f608c729b1","signature":"051a9026cbd2cfd93999ac3a3b2bd7c0f19cd2757f299c8bce82e0e13547a2cdaa87f2320d67f5ec139c2b835d7ca3ea08521fc16694d30d5ad65286a094901f","asset":{"signature":{"publicKey":"0318b677beadb87f35e29150a9bfdb20e1bb934bd70f1208383f4bff7ad6be5d67"}},"confirmations":0,"nonce":"1"}"#,
        r#"{"headerType":0,"version":3,"network":90,"typeGroup":1,"type":1,"nonce":"1","senderPublicKey":"025e1a3fea8d4763b6898ced2e356047d334f11cf3c799ea22b02848f608c729b1","senderId":"dDsmrDBibyfMtDreqHVTNix1sixL74Km2P","fee":"5000000","asset":{"signature":{"publicKey":"0318b677beadb87f35e29150a9bfdb20e1bb934bd70f1208383f4bff7ad6be5d67"}},"signature":"051a9026cbd2cfd93999ac3a3b2bd7c0f19cd2757f299c8bce82e0e13547a2cdaa87f2320d67f5ec139c2b835d7ca3ea08521fc16694d30d5ad65286a094901f","id":"47ceaf1ece83b07f866d1f2f4a1fa5e03c673d3447baa0402f58f10b986963c3"}"#,
    ),
    (
        "ab239c2c5509fa74122d836f2320ceb9975b8c8626ae07eda3c7246a81bcb94e",
        r#"{"id":"ab239c2c5509fa74122d836f2320ceb9975b8c8626ae07eda3c7246a81bcb94e","version":3,"type":7,"typeGroup":1,"fee":"710000","sender":"dEmE3wAHWfpWxvnTMEMmXcB1rCUds6GCA8","senderPublicKey":"025662ee80f6e0a0e71c57c359e5692d4460cbbdb69b3408445c7776ba815f04c7","signature":"4fcdf0a6131e4017e8d7081cb75a1b8df2f442c47284bacc1362c4550d43148c98e419e81a9867437735dad38563941aaa0243a77f3c5ebda9dc286aa87aac70","confirmations":0,"nonce":"2"}"#,
        r#"{"headerType":0,"version":3,"network":90,"typeGroup":1,"type":7,"nonce":"2","senderPublicKey":"025662ee80f6e0a0e71c57c359e5692d4460cbbdb69b3408445c7776ba815f04c7","senderId":"dEmE3wAHWfpWxvnTMEMmXcB1rCUds6GCA8","fee":"710000","signature":"4fcdf0a6131e4017e8d7081cb75a1b8df2f442c47284bacc1362c4550d43148c98e419e81a9867437735dad38563941aaa0243a77f3c5ebda9dc286aa87aac70","id":"ab239c2c5509fa74122d836f2320ceb9975b8c8626ae07eda3c7246a81bcb94e"}"#,
    ),
];

const TRANSFER: &str = "47d5889c16ca8398665ac91b4fba20d6477e1590390c2e9636db48116b48d3f7";

#[test]
fn a_pool_transaction_of_each_type_is_shown_as_the_reference_shows_it() {
    let milestones = chain().milestones();
    for (id, transformed, raw) in SHAPES {
        let transaction = transaction(id);
        let shown = resources::transaction(&transaction, None, milestones);
        assert_eq!(text(&shown), transformed, "{id}");
        assert_eq!(
            text(&resources::raw_transaction(&transaction, milestones)),
            raw,
            "{id}"
        );
    }
    // A transfer's amount is the sum of its items; its memo and items as the data holds them.
    let transfer = resources::transaction(&transaction(TRANSFER), None, milestones);
    let head = r#"{"id":"47d5889c16ca8398665ac91b4fba20d6477e1590390c2e9636db48116b48d3f7","version":3,"type":6,"typeGroup":1,"amount":"100000000000000","fee":"50000000","sender":"dZ1W1GsDCSyhR148oMhuHy3PkhnnSGCqVn","senderPublicKey":"021c65ff25a5bb3b4a5b104ed7d09f7006be53b77687a0ee36d0e8206fae1e8677","signature":"68b62d960f72614a2701529c6621e2cb8220c0b2e565e7e41531e3ac8cd40686384fed8ddc57e3a9ea8f4c86331f4c30ce0194525c9433caa813dd2274a2b519","memo":"solar-devnet: fund 10 test wallet(s)","asset":{"transfers":[{"amount":"10000000000000","recipientId":"dPPBDPkvZsz1xs52r1cToidEbdmzbYpCSB"},"#;
    assert!(text(&transfer).starts_with(head), "{transfer}");
    assert!(text(&transfer).ends_with(r#"}]},"confirmations":0,"nonce":"1"}"#));
}

#[test]
fn a_confirmed_transaction_has_its_block_burned_fee_and_timestamp() {
    let chain = chain();
    let block = chain.block(4);
    let at = TransactionBlock {
        id: *block.id(),
        height: 4,
        timestamp: Some(block.header().timestamp),
        confirmations: 262,
    };
    let shown = resources::transaction(&transaction(TRANSFER), Some(&at), chain.milestones());
    let expected = [
        (
            r#"{"id":"47d5889c16ca8398665ac91b4fba20d6477e1590390c2e9636db48116b48d3f7","blockHeight":4,"blockId":"f0a615175471dd7fe0c57d73a931aff951fd47d66ec0bf7e5fedff664d84a206","version":3,"type":6,"typeGroup":1,"amount":"100000000000000","fee":"50000000","burnedFee":"45000000","sender":"#,
            true,
        ),
        (
            r#""confirmations":262,"timestamp":{"epoch":24,"unix":1790443698,"human":"2026-09-26T17:28:18.000Z"},"nonce":"1"}"#,
            false,
        ),
    ];
    for (part, head) in expected {
        let text = text(&shown);
        assert!(
            if head {
                text.starts_with(part)
            } else {
                text.ends_with(part)
            },
            "{text}"
        );
    }
    // A block's transactions route shows them without the timestamp, with 0 confirmations.
    let listed = TransactionBlock {
        timestamp: None,
        confirmations: 0,
        ..at
    };
    let shown = resources::transaction(&transaction(TRANSFER), Some(&listed), chain.milestones());
    assert!(text(&shown).ends_with(r#""confirmations":0,"nonce":"1"}"#));
}

#[test]
fn a_block_is_shown_as_the_reference_shows_it() {
    let chain = chain();
    // Block 4 holds two transfers of 100,000,000,000,000 in all.
    let shown = resources::block(&chain.row(4), 200_000_000_000_000, 265, chain.milestones());
    assert_eq!(
        text(&shown),
        r#"{"id":"f0a615175471dd7fe0c57d73a931aff951fd47d66ec0bf7e5fedff664d84a206","version":0,"height":4,"previous":"5d008c5d427329ca7c0c6a8d339635c05c160bd905c9236aca8ff961c7767663","forged":{"reward":"200000000","donations":{"dPiLD2Fi1dnQhnskBkSnDEPraaWtbTcYPj":"10000000","dZDbtMv86KMqo8iPMtAJLhoT5BVKfyVokA":"10000000"},"fee":"100000000","burnedFee":"90000000","amount":"200000000000000","total":"190000000"},"payload":{"hash":"98301dde94631fd8e03431d873bad23d8c0fb7f6d33c07490e587c71dca4e350","length":64},"generator":{"username":"genesis_51","publicKey":"02e69cb2cbed90a4362838765c775a26aacc2c6ebc96fb5979be2fe3fc189ce73d"},"signature":"5ced2c3b8a892a3d3f8e7c98fd0256cd85139ecad76c41247d494a396f70713965f8ee4b8bee550d4e27535d62755f3ff928640527a9ca1ed0d5fdfb08101737","confirmations":261,"transactions":2,"timestamp":{"epoch":24,"unix":1790443698,"human":"2026-09-26T17:28:18.000Z"}}"#
    );
}

#[test]
fn types_and_fees_are_the_reference_answers_at_the_tip() {
    let params = chain().milestones().at(265);
    assert_eq!(
        text(&transactions_pool::types_data(params)),
        r#"{"1":{"SecondSignature":1,"DelegateRegistration":2,"Transfer":6,"DelegateResignation":7},"2":{"Burn":0,"Vote":2}}"#
    );
    assert_eq!(
        text(&transactions_pool::fees_data(params).unwrap()),
        r#"{"1":{"secondSignature":"5000000","delegateRegistration":"7500000000","transfer":"50000000","delegateResignation":"0"},"2":{"burn":"0","vote":"9000000"}}"#
    );
}

#[test]
fn a_submission_answers_its_lists_and_its_errors_when_there_are_some() {
    let accepted = ProcessorResult {
        accept: vec!["a".into()],
        broadcast: vec!["a".into()],
        ..ProcessorResult::default()
    };
    assert_eq!(
        text(&transactions_pool::store_body(&accepted)),
        r#"{"data":{"accept":["a"],"broadcast":["a"],"excess":[],"invalid":[]}}"#
    );
    let refusal = |message: &str| Refusal {
        code: PoolCode::BadData,
        message: message.to_owned(),
    };
    let mut errors = IndexMap::new();
    errors.insert("x".to_owned(), refusal("second"));
    errors.insert("1".to_owned(), refusal("first"));
    let refused = ProcessorResult {
        invalid: vec!["1".into(), "x".into()],
        errors: Some(errors),
        ..ProcessorResult::default()
    };
    let body = transactions_pool::store_body(&refused);
    let expected = r#"{"data":{"accept":[],"broadcast":[],"excess":[],"invalid":["1","x"]},"errors":{"1":{"type":"ERR_BAD_DATA","message":"first"},"x":{"type":"ERR_BAD_DATA","message":"second"}}}"#;
    assert_eq!(heartwood_crypto::utils::js::stringify(&body, 0), expected);
}

#[tokio::test]
#[ignore = "integration"]
async fn the_pool_routes_over_http() {
    let chain = chain();
    let (address, server) =
        snapshot::serve_chain("pool", chain.pool(chain.transactions()), Vec::new()).await;
    let milestones = chain.milestones();
    let page = snapshot::get(address, "/api/transactions/unconfirmed?limit=2&page=3").await;
    assert_eq!(page.body["meta"]["totalCount"], 42);
    let expected: Vec<Value> = chain.transactions()[4..6]
        .iter()
        .map(|transaction| resources::transaction(transaction, None, milestones))
        .collect();
    assert_eq!(page.body["data"], Value::Array(expected));
    let (id, transformed, raw) = SHAPES[1];
    let default = snapshot::get(address, &format!("/api/transactions/unconfirmed/{id}")).await;
    assert_eq!(text(&default.body["data"]), raw);
    // Any non-empty `transform`, `false` included, transforms.
    let target = format!("/api/transactions/unconfirmed/{id}?transform=false");
    let shown = snapshot::get(address, &target).await;
    assert_eq!(text(&shown.body["data"]), transformed);
    let missing = snapshot::get(
        address,
        &format!("/api/transactions/unconfirmed/{}", "0".repeat(64)),
    )
    .await;
    assert_eq!(
        (missing.status, &missing.body["message"]),
        (404, &json!("Transaction not found"))
    );
    let types = snapshot::get(address, "/api/transactions/types").await;
    assert_eq!(types.body["data"]["2"]["Vote"], 2);
    let fees = snapshot::get(address, "/api/transactions/fees").await;
    assert_eq!(fees.body["data"]["1"]["transfer"], "50000000");
    // A submission goes to the node's pool; more than the request limit is refused.
    let one = br#"{"transactions":[{"type":6}]}"#;
    let json_type = [("content-type", "application/json")];
    let answer = snapshot::http(address, "POST", "/api/transactions", &json_type, one).await;
    assert_eq!(
        (answer.status, text(&answer.body)),
        (
            200,
            r#"{"data":{"accept":["0"],"broadcast":["0"],"excess":[],"invalid":[]}}"#.to_owned()
        )
    );
    let many = serde_json::to_vec(&json!({ "transactions": vec![json!({}); 41] })).unwrap();
    let answer = snapshot::http(address, "POST", "/api/transactions", &json_type, &many).await;
    assert_eq!(answer.status, 422);
    server.stop().await;
}
