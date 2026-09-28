//! The node's side of its own forger's blocks, on the service over a fake network and a scripted
//! node, on tokio's paused time: a block of the forger received with less than 2,000 ms left in
//! its slot is discarded before anything else; one processed with less than 2,000 ms left is not
//! broadcast but removed through the recovery of one block; one in time is processed and
//! broadcast. The time left is read in the reference's whole seconds.

mod common;
mod service;

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use heartwood_blockchain::IncomingBlock;
use service::{Harness, Setup, block, c1, downloaded, time_of};

const MINUTE: Duration = Duration::from_secs(60);
const FORGER: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The chain's block at `height`, posted by the node's forger.
fn forged(height: u32) -> IncomingBlock {
    let mut block = downloaded(&block(height));
    block.ip = FORGER;
    IncomingBlock {
        block,
        from_forger: true,
        name: None,
    }
}

/// The end of the slot of the block at `height`, in milliseconds since the Unix epoch.
fn slot_end(height: u32) -> i64 {
    let block_ms = i64::try_from(c1().milestones().at(1).block_time()).unwrap() * 1000;
    time_of(height) + block_ms
}

/// A service on the chain at height 12, booted and idle, then waited until `now_ms`.
async fn idle_at(now_ms: i64) -> Harness {
    let h = Harness::new(Setup {
        tip: 12,
        peers: 0,
        now_ms: time_of(12) + 1_000,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    assert_eq!(h.state(), "idle");
    let wait = u64::try_from(now_ms - h.clock_now()).expect("the boot ends before the time");
    h.settle(Duration::from_millis(wait)).await;
    assert_eq!(h.clock_now(), now_ms);
    h
}

#[tokio::test(start_paused = true)]
async fn a_forgers_block_received_with_one_second_left_is_discarded_first() {
    let h = idle_at(slot_end(13) - 1_000).await;
    let log = h.node.log().len();
    h.service.handle_incoming_block(forged(13), true).await;
    assert_eq!(h.state(), "idle", "no NEWBLOCK");
    assert!(h.chain().ping.is_none(), "not pinged: discarded before");
    h.settle(Duration::from_secs(1)).await;
    assert_eq!(h.node.log()[log..], [] as [&str; 0]);
    // A block of the next slot, received early, is late too: its slot is not now.
    let h = idle_at(time_of(13) - 1).await;
    h.service.handle_incoming_block(forged(13), true).await;
    assert!(h.chain().ping.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_forgers_block_with_1_999_ms_left_reads_2_000_and_is_processed_and_broadcast() {
    let h = idle_at(slot_end(13) - 1_999).await;
    h.service.handle_incoming_block(forged(13), true).await;
    assert_eq!(h.state(), "newBlock");
    let ping = h.chain().ping.clone().unwrap();
    assert_eq!(
        (ping.ping.height, ping.count, ping.ping.from_forger),
        (13, 0, true)
    );
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    assert_eq!(h.network.state().broadcasts, [13]);
}

#[tokio::test(start_paused = true)]
async fn a_forgers_block_processed_with_one_second_left_is_removed_through_the_recovery() {
    let h = idle_at(time_of(13) + 1_000).await;
    // The job ends at the slot's last whole second.
    h.node.script().job_time =
        Duration::from_millis(u64::try_from(slot_end(13) - 1_000 - (time_of(13) + 1_000)).unwrap());
    h.service.handle_incoming_block(forged(13), true).await;
    h.wait_for(MINUTE, |h| {
        h.node.log().iter().any(|entry| entry == "remove 1")
    })
    .await;
    assert!(h.network.state().broadcasts.is_empty(), "never broadcast");
    h.wait_for(MINUTE, |h| h.chain().last_height() == 12).await;
    assert_eq!(
        h.chain().blocks_to_rollback,
        0,
        "the recovery took its one block"
    );
}

#[tokio::test(start_paused = true)]
async fn a_peers_block_is_never_checked_for_lateness() {
    let h = idle_at(slot_end(13) - 1_000).await;
    let block = IncomingBlock::downloaded(downloaded(&block(13)));
    h.service.handle_incoming_block(block, true).await;
    assert_eq!(h.state(), "newBlock");
    h.wait_for(MINUTE, |h| h.state() == "idle").await;
    assert_eq!(h.network.state().broadcasts, [13]);
}
