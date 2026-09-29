//! The cases of the onboarding check: each names the check, the view, the options and the findings
//! it must give. The registry walk requires a case for each status a built check declares.

use heartwood_cli::doctor::report::{Fact, Status};
use heartwood_cli::doctor::seams::Subject;
use heartwood_cli::doctor::sources::api::{ApiBlock, ApiSyncing};
use heartwood_cli::doctor::sources::procfs::{SocketRow, TcpState};
use heartwood_cli::doctor::view::{Reason, RolePresence, Source, Unknown};

use super::Case;
use super::peers::{peer, with_peers};
use super::process::{case, run};
use crate::doctor_support::FakeView;

const RELAY: Subject = Subject::Relay;

/// A listening socket at `address`.
fn listening(address: &str) -> SocketRow {
    let local = address.parse().expect("an address");
    let remote = "0.0.0.0:0".parse().expect("an address");
    SocketRow {
        local,
        remote,
        state: TcpState::Listen,
        uid: 1000,
        inode: 7,
    }
}

/// A block of height `height`.
fn block(height: u32, digit: &str) -> ApiBlock {
    let id = digit.repeat(64);
    ApiBlock {
        id,
        height,
        timestamp: None,
        generator_public_key: None,
        username: None,
    }
}

/// A relay at height 100 whose peers stand at `median`, listening at `address`.
fn proof(median: u32, address: &str) -> FakeView {
    let mut view = with_peers(vec![peer("203.0.113.5", Some(median))], 1, 100);
    view.api_syncing = Ok(ApiSyncing {
        syncing: false,
        blocks: 0,
        height: 100,
    });
    view.api_last_block = Ok(block(100, "a"));
    view.api_blocks.insert(2, Ok(Some(block(2, "b"))));
    view.api_address = Ok("127.0.0.1:4003".parse().expect("an address"));
    view.sockets = Ok(vec![listening(address)]);
    view
}

/// The proof over `view`.
fn sync_proof(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "onboarding.sync-proof", view, RELAY, status, advice)
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let mut syncing = proof(100, "0.0.0.0:4003");
    syncing.api_syncing = Ok(ApiSyncing {
        syncing: true,
        blocks: 5,
        height: 100,
    });
    let mut no_block = proof(100, "0.0.0.0:4003");
    no_block.api_blocks.insert(2, Ok(None));
    let mut block_unread = proof(100, "0.0.0.0:4003");
    block_unread.api_blocks.clear();
    let mut elsewhere = proof(100, "0.0.0.0:4003");
    elsewhere.sockets = Ok(vec![listening("0.0.0.0:4001")]);
    let mut no_sockets = proof(100, "0.0.0.0:4003");
    no_sockets.sockets = Err(Unknown::new(Source::Sockets, Reason::NotBuilt));
    let mut not_here = FakeView::new();
    not_here.roles_present = Ok(RolePresence {
        relay: false,
        forger: true,
    });
    let mut both = proof(100, "127.0.0.1:4003");
    both.sockets = Ok(vec![listening("127.0.0.1:4003"), listening("[::1]:4003")]);
    let mut beyond = proof(100, "127.0.0.1:4003");
    beyond.sockets = Ok(vec![
        listening("127.0.0.1:4003"),
        listening("10.0.0.5:4003"),
    ]);
    vec![
        sync_proof(
            "proof: synced and reachable",
            proof(100, "0.0.0.0:4003"),
            Status::Pass,
            "ready",
        ),
        sync_proof(
            "proof: 2 behind",
            proof(102, "0.0.0.0:4003"),
            Status::Pass,
            "ready",
        ),
        sync_proof(
            "proof: ahead of the peers",
            proof(90, "0.0.0.0:4003"),
            Status::Pass,
            "ready",
        ),
        sync_proof(
            "proof: 3 behind",
            proof(103, "0.0.0.0:4003"),
            Status::Fail,
            "not-ready",
        ),
        sync_proof("proof: still syncing", syncing, Status::Fail, "not-ready"),
        sync_proof(
            "proof: block 2 is not there",
            no_block,
            Status::Fail,
            "not-ready",
        ),
        sync_proof(
            "proof: the API listens on loopback only",
            proof(100, "127.0.0.1:4003"),
            Status::Warn,
            "loopback-only",
        ),
        sync_proof(
            "proof: the API listens on loopback twice",
            both,
            Status::Warn,
            "loopback-only",
        ),
        sync_proof(
            "proof: one listener beyond loopback",
            beyond,
            Status::Pass,
            "ready",
        ),
        sync_proof(
            "proof: block 2 is not readable",
            block_unread,
            Status::Unknown,
            "unknown",
        ),
        sync_proof(
            "proof: no listener on the API port",
            elsewhere,
            Status::Unknown,
            "unknown",
        ),
        sync_proof(
            "proof: the sockets are not readable",
            no_sockets,
            Status::Unknown,
            "unknown",
        ),
        sync_proof(
            "proof: the peers are not readable",
            FakeView::new(),
            Status::Unknown,
            "unknown",
        ),
        sync_proof(
            "proof: no relay here",
            not_here,
            Status::Skipped,
            "skipped-absent",
        ),
    ]
}

#[test]
fn the_proof_prints_the_facts_and_no_text_to_sign() {
    let findings = run("onboarding.sync-proof", &proof(100, "0.0.0.0:4003"));
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert!(finding.facts.contains(&("height", Fact::Int(100))));
    assert!(finding.facts.iter().any(|(name, _)| *name == "tip_id"));
    assert!(finding.facts.iter().any(|(name, _)| *name == "block_2_id"));
    assert!(
        finding
            .facts
            .contains(&("api", Fact::Word("beyond loopback")))
    );
}
