//! The canonical state digest (format `csd1`): the committed state at a height as text sections
//! and their hashes, identical to the oracle's digest of the reference's state.
//!
//! Each section is its tag line (`#B`, `#S`, `#R`, `#M`, `#W`) and its records, every line ending
//! in `\n`. A record is one JSON object without whitespace, with its keys in the order below and a
//! key left out when its value is absent. Amounts, balances, nonces and vote balances are decimal
//! strings; heights, rounds, ranks, counts, timestamps, basis points and type numbers are
//! integers; hex is lowercase.
//!
//! - `#B`, the last block: `h`, `id`, `ts`, `gen`, `reward`, `fee` (total fee), `burned`
//!   (burned fee), `don` (`[[address, amount], ...]`, every donation of the block, zero amounts
//!   included, sorted by the address text).
//! - `#S`: `issued`, `supply` (the sum of all balances).
//! - `#R`: `round` and `rh` (the round of the last block and its first height), `order` (the
//!   public keys of the forging list that schedules the next block), `prod` (the names of the
//!   delegates that forged the round's blocks, in order), and, only after the genesis block and
//!   after a block that ends a round, `active` (`[[publicKey, voteBalance], ...]`, the new
//!   round's delegates in rank order).
//! - `#M`: `[timestamp, username]` for each slot missed before the block, in slot order; no
//!   record for a revert.
//! - `#W`: every wallet with a balance, a nonce, a public key, a second key, a delegate record or
//!   votes, sorted by the address text: `a`, `pk`, `bal`, `n`, `spk`, `u`, `vb`, `vc`, `rk`, `rs`
//!   (0 temporary, 1 permanent), `pb`, `ff`, `fr`, `bf`, `dn`, `lb`, `v` (`[[username, bps],
//!   ...]` in the order the votes are kept). The height of the latest resignation is not part of
//!   the digest.
//!
//! Hashes use the registry's tags ([`heartwood_crypto::crypto::domain`]): each section's hash
//! `S = tagged_hash(StateDigestSection, section)`; the height digest
//! `SD = tagged_hash(StateDigestHeight, S_B || S_S || S_R || S_M || S_W)`; and over the lines of a
//! replay, `CD = tagged_hash(StateDigestChain, CD_previous || SD)`, starting from 32 zero bytes.

use heartwood_crypto::{Block, BlockId, Milestones};

use crate::machine::State;
use crate::round_state::{MissedBlock, RoundState};
use crate::stores::state::Totals;
use crate::wallets::WalletRepository;

/// Which step of a replay a digest line describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// The state after the genesis block.
    Genesis,
    /// The state after applying a block.
    Apply,
    /// The state after reverting a block (the line's height is the new last block's).
    Revert,
    /// The state after applying again a block that a revert took back.
    Reapply,
}

impl Step {
    /// The step's name in a digest line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Step::Genesis => "genesis",
            Step::Apply => "apply",
            Step::Revert => "revert",
            Step::Reapply => "reapply",
        }
    }
}

/// The five sections, each its tag line and records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sections {
    /// `#B`, the last block.
    pub block: String,
    /// `#S`, the supply.
    pub supply: String,
    /// `#R`, the round.
    pub round: String,
    /// `#M`, the missed slots.
    pub missed: String,
    /// `#W`, the wallets.
    pub wallets: String,
}

/// The hashes of the five sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionHashes {
    /// Of `#B`.
    pub block: [u8; 32],
    /// Of `#S`.
    pub supply: [u8; 32],
    /// Of `#R`.
    pub round: [u8; 32],
    /// Of `#M`.
    pub missed: [u8; 32],
    /// Of `#W`.
    pub wallets: [u8; 32],
}

impl Sections {
    /// The hash of each section.
    pub fn hashes(&self) -> SectionHashes {
        todo!()
    }
}

impl SectionHashes {
    /// The height digest `SD` over the section hashes.
    pub fn state_digest(&self) -> [u8; 32] {
        todo!()
    }
}

/// One line of a replay's digest file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestLine {
    /// The height of the last block.
    pub height: u32,
    /// The step.
    pub step: Step,
    /// The id of the last block.
    pub id: BlockId,
    /// The reward the state computed for the block before applying it (apply and reapply only).
    pub reward: Option<u64>,
    /// The section hashes.
    pub sections: SectionHashes,
    /// The height digest `SD`.
    pub state_digest: [u8; 32],
    /// The running digest `CD` after this line.
    pub chain_digest: [u8; 32],
}

impl DigestLine {
    /// The line as JSON text without whitespace: `h`, `step`, `id`, `rw` (when present), `b`,
    /// `s`, `r`, `m`, `w`, `sd`, `cd`, hashes in lowercase hex.
    pub fn to_json(&self) -> String {
        todo!()
    }
}

/// The `#B` section of `block`.
pub fn block_section(block: &Block, milestones: &Milestones) -> String {
    let _ = (block, milestones);
    todo!()
}

/// The `#S` section.
pub fn supply_section(totals: &Totals, wallets: &WalletRepository) -> String {
    let _ = (totals, wallets);
    todo!()
}

/// The `#R` section of `round` after the block at `height`.
pub fn round_section(round: &RoundState, height: u32, milestones: &Milestones) -> String {
    let _ = (round, height, milestones);
    todo!()
}

/// The `#M` section of `missed`.
pub fn missed_section(missed: &[MissedBlock]) -> String {
    let _ = missed;
    todo!()
}

/// The `#W` section of `wallets`.
pub fn wallet_section(wallets: &WalletRepository) -> String {
    let _ = wallets;
    todo!()
}

/// Every section of `state`, with `missed` as the missed slots of its last step.
pub fn records(state: &State, milestones: &Milestones, missed: &[MissedBlock]) -> Sections {
    let _ = (state, milestones, missed);
    todo!()
}

/// The running digest after a line with height digest `state_digest`.
pub fn chain_digest(previous: &[u8; 32], state_digest: &[u8; 32]) -> [u8; 32] {
    let _ = (previous, state_digest);
    todo!()
}

/// The digest line of `state` after `step`, with the reward computed before the step (apply and
/// reapply), the missed slots of the step, and the running digest before it.
pub fn height_digest(
    state: &State,
    milestones: &Milestones,
    step: Step,
    missed: &[MissedBlock],
    reward: Option<u64>,
    previous: &[u8; 32],
) -> DigestLine {
    let _ = (state, milestones, step, missed, reward, previous);
    todo!()
}
