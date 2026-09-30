//! A relay's start after `snapshot rollback`, as its peers see it (run with the integration tests:
//! it needs the whole relay and its peer-to-peer server): the store of the committed one-node
//! chain rolled back from 265 with `--height 133`, then `heartwood relay run`, whose server
//! answers the peer-to-peer client's getStatus, getBlocks and getCommonBlocks at the rollback's
//! target, 106, with the committed chain's blocks; the start wrote round 3's rows again, and SIGINT
//! stops the relay.
//!
//! The relay listens at .41 of the tests' loopback block (`HEARTWOOD_TEST_LOOPBACK`, else
//! 127.13.0), P2P 6002 and API 6003.

mod common;

use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::{Setup, corpus};
use heartwood_blockchain::{ChainStore, RestoreMode, restore};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockId};
use heartwood_database::{Database, Settings};
use heartwood_kernel::config::crypto;
use heartwood_p2p::connector::Connector;
use heartwood_p2p_wire::Proto;
use heartwood_p2p_wire::codecs::blocks::get_blocks_decode_response;
use heartwood_p2p_wire::codecs::peer::get_common_blocks_decode_response;
use heartwood_p2p_wire::messages::peer::GetStatusResponse;
use heartwood_p2p_wire::requests::{ClientRequest, CommonBlockIds, Headers};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

/// A process killed when dropped.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The committed chain's blocks: (height, id, bytes).
fn blocks() -> Vec<(u32, BlockId, Vec<u8>)> {
    let text = fs::read_to_string(corpus().join("blocks.jsonl")).unwrap();
    let block = |line: &str| {
        let line: Value = serde_json::from_str(line).unwrap();
        let height = u32::try_from(line["h"].as_u64().unwrap()).unwrap();
        let id = BlockId::from_hex(line["id"].as_str().unwrap()).unwrap();
        (
            height,
            id,
            hex::decode(line["hex"].as_str().unwrap()).unwrap(),
        )
    };
    text.lines().map(block).collect()
}

/// A store of the committed chain at its tip in the data directory `data`.
fn store_at_tip(data: &Path) {
    let files = crypto::load(&corpus().join("crypto")).unwrap();
    let mut store = ChainStore::open(&Settings::in_data_dir(data)).unwrap();
    let mut state = restore(&mut store, &files, &Handlers, RestoreMode::Auto)
        .unwrap()
        .state;
    let milestones = &files.milestones;
    let mut chunk = store.chunk().unwrap();
    for (_, _, bytes) in blocks() {
        let block = Block::receive(&bytes, milestones).unwrap();
        let applied = state.apply_block(&block, milestones, &Handlers).unwrap();
        chunk.applied(&state, &block, &applied, milestones).unwrap();
    }
    chunk.commit().unwrap();
}

/// `request` to the relay at `address`, its answer's bytes.
fn ask(
    runtime: &tokio::runtime::Runtime,
    connector: &Connector,
    address: SocketAddr,
    request: &ClientRequest,
) -> Option<Vec<u8>> {
    let payload = request.encode(&Headers::new("4.3.1", 4002).unwrap());
    let answer = connector.emit(
        address,
        request.route(),
        payload,
        Duration::from_secs(10),
        20 * 1024 * 1024,
    );
    runtime.block_on(answer).ok()
}

#[test]
#[ignore = "integration"]
fn a_relay_started_after_a_rollback_serves_the_chain_at_its_target() {
    let host = common::loopback("127.13.0", 41);
    let env = format!(
        "CORE_P2P_HOST={host}\nCORE_P2P_PORT=6002\nCORE_API_HOST={host}\nCORE_API_PORT=6003\nCORE_P2P_MIN_NETWORK_REACH=0\n"
    );
    let setup = Setup::new("rollback-serve", &env);
    setup.write(
        "peers.json",
        &json!({"list": [{"ip": host, "port": 6002}]}).to_string(),
    );
    let data = setup.root.join("data");
    store_at_tip(&data);
    let heartwood = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
        command
            .args(args)
            .args(["--token", "t", "--network", "devnet"])
            .env_clear()
            .env("HOME", setup.home())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(setup.process(&[]))
            .stdin(Stdio::null());
        command
    };
    let rolled = heartwood(&["snapshot", "rollback", "--height", "133"])
        .output()
        .unwrap();
    assert!(rolled.status.success(), "{rolled:?}");
    let log = fs::File::create(setup.root.join("relay.log")).unwrap();
    let relay = heartwood(&["relay", "run", "--networkStart"])
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let mut relay = Killed(relay);
    let output = || fs::read_to_string(setup.root.join("relay.log")).unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let connector = Connector::new();
    let address: SocketAddr = format!("{host}:6002").parse().unwrap();
    let chain = blocks();
    let id_at = |height: u32| chain[usize::try_from(height - 2).unwrap()].1;
    // getStatus, once the relay has started: its tip is the target.
    let mut polls = 0;
    let status = loop {
        assert!(relay.0.try_wait().unwrap().is_none(), "{}", output());
        polls += 1;
        assert!(polls <= 360, "no status within 90 s: {}", output());
        let answer = ask(&runtime, &connector, address, &ClientRequest::GetStatus);
        match answer.and_then(|bytes| GetStatusResponse::decode(&bytes).ok()) {
            Some(status) if status.state.is_some() => break status,
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    };
    let state = status.state.unwrap();
    assert_eq!(state.height, Some(106));
    let header = state.header.unwrap();
    assert_eq!(header.id, Some(id_at(106).to_hex()));
    // getBlocks after 100: the blocks up to the target, and nothing above it.
    let request = ClientRequest::GetBlocks {
        last_block_height: 100.try_into().unwrap(),
        block_limit: 400.try_into().unwrap(),
        headers_only: false,
    };
    let listed = ask(&runtime, &connector, address, &request).unwrap();
    let listed = get_blocks_decode_response(&listed).unwrap();
    let heights: Vec<u32> = listed.iter().filter_map(|b| b.header.height).collect();
    assert_eq!(heights, (101..=106).collect::<Vec<_>>());
    for (block, height) in listed.iter().zip(101..) {
        assert_eq!(block.header.id, Some(id_at(height).to_hex()), "{height}");
    }
    // getCommonBlocks: the highest stored block of the ids, which a block above the target is not.
    let ids = CommonBlockIds::new(vec![id_at(2), id_at(106), id_at(107)]).unwrap();
    let request = ClientRequest::GetCommonBlocks { ids };
    let common = ask(&runtime, &connector, address, &request).unwrap();
    let common = get_common_blocks_decode_response(&common).unwrap();
    let common = common.common.unwrap();
    assert_eq!(common.height, Some(106));
    assert_eq!(common.id, Some(id_at(106).to_hex()));
    // SIGINT: a graceful stop; the start wrote round 3's rows again.
    let pid = relay.0.id().to_string();
    let signalled = Command::new("kill").args(["-INT", &pid]).status().unwrap();
    assert!(signalled.success());
    let mut waits = 0;
    let stopped = loop {
        if let Some(status) = relay.0.try_wait().unwrap() {
            break status;
        }
        waits += 1;
        assert!(waits <= 300, "the relay did not stop within 30 s");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(stopped.code(), Some(0), "{}", output());
    let database = Database::open(&Settings::in_data_dir(&data)).unwrap();
    assert_eq!(database.last_block().unwrap().unwrap().row.height, 106);
    assert_eq!(database.round(3).unwrap().len(), 53);
    assert!(database.round(4).unwrap().is_empty());
}
