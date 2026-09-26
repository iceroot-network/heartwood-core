//! The canonical state digest (format `csd1`): the committed state at a height as text sections
//! and their hashes, identical to the oracle's digest of the reference's state.
//!
//! Each section is its tag line (`#B`, `#S`, `#R`, `#M`, `#W`) and its records, every line ending
//! in `\n`. A record is one JSON value without whitespace; an object has its keys in the order
//! below, with a key left out when its value is absent. Amounts, balances, nonces and vote
//! balances are decimal strings; heights, rounds, ranks, counts, timestamps, basis points and
//! type numbers are integers; keys, ids and hashes are lowercase hex; addresses are Base58 text.
//! Text is escaped as `JSON.stringify` escapes it.
//!
//! - `#B`, one record for the last block: `h`, `id`, `ts`, `gen` (the generator key), `reward`,
//!   `fee` (total fee), `burned` (burned fee), `don` (`[[address, amount], ...]`, every donation
//!   of the block, zero amounts included, sorted by the address text).
//! - `#S`, one record: `issued`, `supply` (the sum of all balances).
//! - `#R`, one record: `round` and `rh` (the round of the last block and its first height),
//!   `order` (the public keys of the forging list that schedules the next block), `prod` (the
//!   names of the delegates that forged the round's blocks, in order), and, only after the
//!   genesis block and after a block that ends a round, `active` (`[[publicKey, voteBalance],
//!   ...]`, the new round's delegates in rank order).
//! - `#M`: one record `[timestamp, username]` for each slot missed before the block, in slot
//!   order; none for a revert.
//! - `#W`: one record for every wallet with a balance, a nonce, a public key, a second key, a
//!   delegate record or votes, sorted by the address text: `a`, `pk`, `bal`, `n`, `spk`, then for
//!   a delegate `u`, `vb`, `vc` (voters), `rk` (rank), `rs` (0 temporary, 1 permanent), `pb`
//!   (produced blocks), `ff` (forged fees), `fr` (forged rewards), `bf` (burned fees), `dn`
//!   (donations), `lb` (last block id), and last `v` (`[[username, bps], ...]` in the order the
//!   votes are kept), left out when the wallet votes for no one. The height of the latest
//!   resignation is not part of the digest.
//!
//! Hashes use the registry's tags ([`heartwood_crypto::crypto::domain`]): each section's hash
//! `S = tagged_hash(StateDigestSection, section)`; the height digest
//! `SD = tagged_hash(StateDigestHeight, S_B || S_S || S_R || S_M || S_W)`; and over the lines of a
//! replay, `CD = tagged_hash(StateDigestChain, CD_previous || SD)`, starting from 32 zero bytes.

use std::fmt::Display;

use heartwood_crypto::crypto::domain::{DomainTag, tagged_hash};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockId, Milestones};
use heartwood_kernel::{calculate_round, is_new_round};

use crate::machine::State;
use crate::round_state::{MissedBlock, RoundState};
use crate::stores::state::Totals;
use crate::supply::total_balance;
use crate::wallets::{Wallet, WalletRepository};

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
        let hash =
            |section: &str| tagged_hash(DomainTag::StateDigestSection, &[section.as_bytes()]);
        SectionHashes {
            block: hash(&self.block),
            supply: hash(&self.supply),
            round: hash(&self.round),
            missed: hash(&self.missed),
            wallets: hash(&self.wallets),
        }
    }
}

impl SectionHashes {
    /// The height digest `SD` over the section hashes.
    pub fn state_digest(&self) -> [u8; 32] {
        tagged_hash(
            DomainTag::StateDigestHeight,
            &[
                &self.block,
                &self.supply,
                &self.round,
                &self.missed,
                &self.wallets,
            ],
        )
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
        let hashes = &self.sections;
        Record::new()
            .integer("h", self.height)
            .text("step", self.step.as_str())
            .text("id", &self.id.to_hex())
            .maybe(self.reward, |record, reward| record.amount("rw", reward))
            .text("b", &hex::encode(&hashes.block))
            .text("s", &hex::encode(&hashes.supply))
            .text("r", &hex::encode(&hashes.round))
            .text("m", &hex::encode(&hashes.missed))
            .text("w", &hex::encode(&hashes.wallets))
            .text("sd", &hex::encode(&self.state_digest))
            .text("cd", &hex::encode(&self.chain_digest))
            .close()
    }
}

/// The `#B` section of `block`.
pub fn block_section(block: &Block, milestones: &Milestones) -> String {
    let header = block.header();
    let mut donations: Vec<(String, u64)> = block
        .donations(milestones)
        .into_iter()
        .map(|(address, amount)| (address.to_base58(), amount))
        .collect();
    donations.sort_by(|a, b| a.0.cmp(&b.0));
    let donations = array(
        donations
            .iter()
            .map(|(address, amount)| array([json_string(address), quoted(amount)])),
    );
    let record = Record::new()
        .integer("h", header.height)
        .text("id", &block.id().to_hex())
        .integer("ts", header.timestamp)
        .text("gen", &header.generator_public_key.to_hex())
        .amount("reward", header.reward)
        .amount("fee", header.total_fee)
        .amount("burned", block.burned_fee(milestones))
        .raw("don", &donations)
        .close();
    section("#B", [record])
}

/// The `#S` section.
pub fn supply_section(totals: &Totals, wallets: &WalletRepository) -> String {
    let record = Record::new()
        .amount("issued", totals.issued)
        .amount("supply", total_balance(wallets))
        .close();
    section("#S", [record])
}

/// The `#R` section of `round` after the block at `height`.
pub fn round_section(round: &RoundState, height: u32, milestones: &Milestones) -> String {
    let info = calculate_round(height, milestones);
    let order = array(
        round
            .forging_delegates()
            .iter()
            .map(|delegate| json_string(&delegate.public_key.to_hex())),
    );
    let produced = array(round.blocks().iter().map(|username| json_string(username)));
    let mut record = Record::new()
        .integer("round", info.round)
        .integer("rh", info.round_height)
        .raw("order", &order)
        .raw("prod", &produced);
    if height == 1 || is_new_round(height.saturating_add(1), milestones) {
        let active = array(round.round_delegates().iter().map(|delegate| {
            array([
                json_string(&delegate.public_key.to_hex()),
                quoted(delegate.vote_balance),
            ])
        }));
        record = record.raw("active", &active);
    }
    section("#R", [record.close()])
}

/// The `#M` section of `missed`.
pub fn missed_section(missed: &[MissedBlock]) -> String {
    section(
        "#M",
        missed
            .iter()
            .map(|slot| array([slot.timestamp.to_string(), json_string(&slot.username)])),
    )
}

/// The `#W` section of `wallets`.
pub fn wallet_section(wallets: &WalletRepository) -> String {
    let mut records: Vec<(String, String)> = wallets
        .iter()
        .filter(|wallet| is_visible(wallet))
        .map(|wallet| {
            let address = wallet.address().to_base58();
            let record = wallet_record(&address, wallet);
            (address, record)
        })
        .collect();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    section("#W", records.into_iter().map(|(_, record)| record))
}

/// Every section of `state`, with `missed` as the missed slots of its last step.
pub fn records(state: &State, milestones: &Milestones, missed: &[MissedBlock]) -> Sections {
    Sections {
        block: block_section(state.last_block(), milestones),
        supply: supply_section(state.totals(), state.wallets()),
        round: round_section(state.round(), state.height(), milestones),
        missed: missed_section(missed),
        wallets: wallet_section(state.wallets()),
    }
}

/// The running digest after a line with height digest `state_digest`.
pub fn chain_digest(previous: &[u8; 32], state_digest: &[u8; 32]) -> [u8; 32] {
    tagged_hash(DomainTag::StateDigestChain, &[previous, state_digest])
}

/// The digest line of `state` after `step`, with the reward computed before the step (apply and
/// reapply), the missed slots of the step (none for a revert), and the running digest before it.
pub fn height_digest(
    state: &State,
    milestones: &Milestones,
    step: Step,
    missed: &[MissedBlock],
    reward: Option<u64>,
    previous: &[u8; 32],
) -> DigestLine {
    let sections = records(state, milestones, missed).hashes();
    let state_digest = sections.state_digest();
    DigestLine {
        height: state.height(),
        step,
        id: *state.last_block().id(),
        reward,
        sections,
        state_digest,
        chain_digest: chain_digest(previous, &state_digest),
    }
}

/// Whether the digest lists `wallet`: a wallet that nothing has happened to, or that is back to
/// that, is left out.
fn is_visible(wallet: &Wallet) -> bool {
    wallet.balance() != 0
        || wallet.nonce() != 0
        || wallet.public_key().is_some()
        || wallet.second_public_key().is_some()
        || wallet.is_delegate()
        || wallet.has_voted()
}

/// The `#W` record of `wallet`, whose address text is `address`.
fn wallet_record(address: &str, wallet: &Wallet) -> String {
    let mut record = Record::new()
        .text("a", address)
        .maybe(wallet.public_key(), |record, key| {
            record.text("pk", &key.to_hex())
        })
        .amount("bal", wallet.balance())
        .amount("n", wallet.nonce())
        .maybe(wallet.second_public_key(), |record, key| {
            record.text("spk", &key.to_hex())
        });
    if let Some(delegate) = wallet.delegate() {
        record = record
            .text("u", delegate.username())
            .amount("vb", delegate.vote_balance)
            .integer("vc", delegate.voters)
            .maybe(delegate.rank, |record, rank| record.integer("rk", rank))
            .maybe(delegate.resigned, |record, resigned| {
                record.integer("rs", resigned.number())
            })
            .integer("pb", delegate.produced_blocks)
            .amount("ff", delegate.forged_fees)
            .amount("fr", delegate.forged_rewards)
            .amount("bf", delegate.burned_fees)
            .amount("dn", delegate.donations)
            .maybe(delegate.last_block, |record, id| {
                record.text("lb", &id.to_hex())
            });
    }
    if wallet.has_voted() {
        let votes = array(
            wallet
                .votes()
                .iter()
                .map(|(username, bps)| array([json_string(username), bps.to_string()])),
        );
        record = record.raw("v", &votes);
    }
    record.close()
}

/// A section: its tag line, then one line per record.
fn section(tag: &str, records: impl IntoIterator<Item = String>) -> String {
    let mut text = format!("{tag}\n");
    for record in records {
        text.push_str(&record);
        text.push('\n');
    }
    text
}

/// A JSON object written key by key, in the order the keys are given.
struct Record(String);

impl Record {
    fn new() -> Record {
        Record(String::from("{"))
    }

    /// Append `key` and `value`, which is JSON text already.
    fn raw(mut self, key: &str, value: &str) -> Record {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        self.0.push_str(&json_string(key));
        self.0.push(':');
        self.0.push_str(value);
        self
    }

    /// Append a JSON integer.
    fn integer(self, key: &str, value: impl Display) -> Record {
        self.raw(key, &value.to_string())
    }

    /// Append a JSON string.
    fn text(self, key: &str, value: &str) -> Record {
        self.raw(key, &json_string(value))
    }

    /// Append an amount: its decimal digits as a JSON string.
    fn amount(self, key: &str, value: impl Display) -> Record {
        self.raw(key, &quoted(value))
    }

    /// Append what `write` appends when `value` is present.
    fn maybe<T>(self, value: Option<T>, write: impl FnOnce(Record, T) -> Record) -> Record {
        match value {
            Some(value) => write(self, value),
            None => self,
        }
    }

    fn close(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

/// A JSON array of `items`, which are JSON text already.
fn array(items: impl IntoIterator<Item = String>) -> String {
    let items: Vec<String> = items.into_iter().collect();
    format!("[{}]", items.join(","))
}

/// `value` as a JSON string of its decimal digits.
fn quoted(value: impl Display) -> String {
    format!("\"{value}\"")
}

/// `text` as a JSON string, escaped as `JSON.stringify` escapes it.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
