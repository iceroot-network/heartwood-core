//! The onboarding proof: the facts an applicant shows that the relay is synced and its API is
//! reachable.

use std::net::IpAddr;

use crate::doctor::checks::peers::behind;
use crate::doctor::checks::process::{Facts, either, ints, relay};
use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::Chain;
use crate::doctor::report::Status::{Fail, Pass, Warn};
use crate::doctor::report::{Fact, Finding, Id, IdKind};
use crate::doctor::sources::procfs::TcpState;
use crate::doctor::view::{Reason, Source, Unknown};

/// How many blocks behind the peers' median still counts as synced.
const TOLERANCE: i64 = 2;
/// The height whose block the proof shows: the first after genesis.
const FIRST_BLOCK: u32 = 2;

/// `onboarding.sync-proof`: prints the facts and no text to sign.
pub fn sync_proof(input: &CheckInput<'_>) -> Vec<Finding> {
    relay(input, |view| {
        let (behind, height) = behind(view)?;
        let (syncing, tip) = (view.api_syncing()?.syncing, view.api_last_block()?);
        let block = view.api_block(FIRST_BLOCK)?;
        let (behind, block_2) = (behind.max(0), block.is_some());
        let mut facts: Facts = ints!(height, behind);
        facts.extend([
            ("syncing", Fact::Bool(syncing)),
            ("has_block_2", Fact::Bool(block_2)),
        ]);
        if syncing || behind > TOLERANCE || !block_2 {
            return Ok((Chain, (Fail, "not-ready"), facts));
        }
        let port = view.api_address()?.port();
        let inodes = view.role_socket_inodes(crate::seams::Role::Relay)?;
        let rows = view.sockets()?.iter();
        let listening = rows.filter(|row| {
            row.state == TcpState::Listen && row.local.port() == port && inodes.contains(&row.inode)
        });
        let addresses: Vec<IpAddr> = listening.map(|row| row.local.ip()).collect();
        if addresses.is_empty() {
            return Err(Unknown::new(Source::Sockets, Reason::Missing));
        }
        // The proof is its block ids: an id the API gives out of its grammar leaves it unknown.
        let unparsed = Unknown::new(Source::Api, Reason::Unparsed);
        let hex = |text: &str| Id::new(IdKind::Hex64, text).map(Fact::Id).ok_or(unparsed);
        facts.push(("tip_id", hex(&tip.id)?));
        let block_2 = block.map_or(Err(unparsed), |b| hex(&b.id))?;
        facts.push(("block_2_id", block_2));
        let loopback = addresses.iter().all(|ip| ip.to_canonical().is_loopback());
        let word = if loopback {
            "loopback-only"
        } else {
            "beyond-loopback"
        };
        facts.push(("api", Fact::Word(word)));
        let verdict = either(loopback, (Warn, "loopback-only"), (Pass, "ready"));
        Ok((Chain, verdict, facts))
    })
}
