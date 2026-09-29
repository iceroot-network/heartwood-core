//! The delegate routes over HTTP on the committed devnet chain, with the real pagination service,
//! the state's index orders and a production table: the listing against the reference's
//! checkpoint, the productivity of the delegates that have one, the lookups and the voters.

mod snapshot;

use std::sync::Arc;

use heartwood_api::server::serve;
use heartwood_api::{ApiError, DelegateProduction, Hundredths, ProductivityTable};
use serde_json::{Value, json};

use snapshot::{chain, get};

/// A node whose table gives `genesis_1` two missed blocks and 99.5% productivity.
struct Table;

impl heartwood_api::Productivity for Table {
    fn table(&self, _tip: &heartwood_crypto::Block) -> Result<Arc<ProductivityTable>, ApiError> {
        let mut table = ProductivityTable::default();
        let production = DelegateProduction {
            missed: Some(2),
            productivity: Some(Hundredths(9950)),
        };
        table.by_username.insert("genesis_1".into(), production);
        Ok(Arc::new(table))
    }

    fn totals(
        &self,
        _tip: &heartwood_crypto::Block,
    ) -> Result<heartwood_api::ChainTotals, ApiError> {
        Err(ApiError::internal())
    }
}

async fn node(name: &str) -> (std::net::SocketAddr, heartwood_api::ServerHandle) {
    let context = snapshot::context(
        name,
        chain().pool(Vec::new()),
        Vec::new(),
        snapshot::local_config(),
    );
    let context = heartwood_api::ApiContext {
        productivity: Arc::new(Table),
        ..context
    };
    let address = context.config.address;
    let server = serve(Arc::new(context), &tokio::runtime::Handle::current()).unwrap();
    (address, server)
}

fn names(answer: &snapshot::Answer) -> Vec<String> {
    let data = answer.body["data"].as_array().unwrap();
    data.iter()
        .map(|d| d["username"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
#[ignore = "integration"]
async fn the_delegate_listing_over_http() {
    let (address, _server) = node("delegates-live").await;
    let all = get(address, "/api/delegates?limit=100").await;
    assert_eq!(all.status, 200);
    let meta = &all.body["meta"];
    assert_eq!(
        (
            meta["count"].clone(),
            meta["totalCount"].clone(),
            meta["pageCount"].clone()
        ),
        (json!(56), json!(56), json!(1))
    );
    let reference = chain().checkpoint["delegates"].as_array().unwrap();
    let data = all.body["data"].as_array().unwrap();
    for delegate in data {
        let name = delegate["username"].as_str().unwrap();
        let expected = reference.iter().find(|r| r["username"] == name).unwrap();
        assert_eq!(delegate["address"], expected["address"], "{name}");
        assert_eq!(
            delegate["votesReceived"]["votes"], expected["votes"],
            "{name}"
        );
        assert_eq!(delegate["forged"], expected["forged"], "{name}");
    }
    // By rank, then by name; the two without a rank last.
    let ranks: Vec<Option<u64>> = data.iter().map(|d| d["rank"].as_u64()).collect();
    assert!(ranks[..54].windows(2).all(|p| p[0] < p[1]));
    assert_eq!(ranks[54..], [None, None]);
    // The table's production shows on the delegate that has it, and only there.
    let first = data.iter().find(|d| d["username"] == "genesis_1").unwrap();
    assert_eq!(first["blocks"]["missed"], 2);
    assert_eq!(first["blocks"]["productivity"], 99.5);
    assert!(
        data.iter()
            .filter(|d| d["blocks"].get("missed").is_some())
            .count()
            == 1
    );
    let paged = get(address, "/api/delegates?limit=10&page=2").await;
    assert_eq!(names(&paged), names(&all)[10..20]);
    assert!(
        paged.body["meta"]["previous"]
            .as_str()
            .unwrap()
            .contains("page=1")
    );
    let ordered = get(address, "/api/delegates?orderBy=username:desc&limit=2").await;
    assert_eq!(names(&ordered), ["tx1n3233", "tx1n2290"]);
    let voters = get(
        address,
        "/api/delegates?votesReceived.voters.from=2&isResigned=false&limit=100",
    )
    .await;
    let expected = reference
        .iter()
        .filter(|d| d["voters"].as_u64() >= Some(2) && d["isResigned"] == false)
        .count();
    assert_eq!(voters.body["meta"]["totalCount"], expected);
    let refused = get(address, "/api/delegates?username=gen%25").await;
    assert_eq!(
        (refused.status, &refused.body["message"]),
        (
            422,
            &json!("\"username\" does not match any of the allowed types")
        )
    );
    let thrown = get(address, "/api/delegates?blocks.last.id=x").await;
    assert_eq!(thrown.status, 422);
    let thrown = get(address, "/api/delegates?blocks.last.height=1").await;
    assert_eq!(thrown.status, 500);
}

#[tokio::test]
#[ignore = "integration"]
async fn a_delegate_and_its_voters_over_http() {
    let (address, _server) = node("delegates-live-show").await;
    let by_name = get(address, "/api/delegates/genesis_34").await;
    assert_eq!(by_name.status, 200);
    let data = &by_name.body["data"];
    assert_eq!(data["address"], "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY");
    assert!(data["votesReceived"]["percent"].as_f64().is_some());
    let by_address = get(address, "/api/delegates/dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY").await;
    assert_eq!(by_address.body, by_name.body);
    let missing = get(address, "/api/delegates/nobody").await;
    assert_eq!(
        (missing.status, &missing.body["message"]),
        (404, &json!("Delegate not found"))
    );
    let voters = get(address, "/api/delegates/genesis_34/voters").await;
    assert_eq!(voters.status, 200);
    assert_eq!(voters.body["meta"]["totalCount"], 1);
    assert_eq!(
        voters.body["data"][0]["address"],
        "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY"
    );
    let none = get(
        address,
        "/api/delegates/genesis_34/voters?balance.from=999999999999",
    )
    .await;
    assert_eq!(none.body["meta"]["totalCount"], 0);
    let bad: Value = get(address, "/api/delegates/UPPER/voters").await.body;
    assert_eq!(bad["statusCode"], 422);
}
