//! Heartwood's subjects: the oracle's operations answered with Heartwood's crates.
//!
//! Each subject turns a case's input into the crates' types, runs them, and writes the result in
//! the JSON shape the oracle writes (`tools/oracle/lib/ops.js` and `tools/oracle/lib/ops/`), so
//! that L0 and L1 can compare them value for value.
//!
//! With `heartwood-crypto`:
//!
//! - `keys`: passphrase keys, public keys and addresses.
//! - `signatures`: BIP340 and legacy Schnorr, transaction preimages, the published vector files
//!   and BIP39 mnemonics.
//! - `transactions`: the builders, both decoders, the JSON reader, the handler table and the vote
//!   rules.
//! - `blocks`: the block factory and decoder, rewards, donations, the fee burn and the milestone
//!   loader.
//! - `genesis`: the devnet chain-definition builder.
//!
//! With the kernel, the logger, the state and the transaction handlers:
//!
//! - `kernel`: rounds, forging slots, block chaining, the status canonical JSON, `.env` text,
//!   timestamps, IP lists, node paths and log lines.
//! - `state`: the forging shuffle, the delegate ranking and a voter's shares.
//! - `digest`: the canonical state digest of a wallet fixture.
//! - `handlers`: the dynamic fee floor, and a transaction applied to a wallet fixture and
//!   reverted.
//! - `config`: the crypto files, loaded with the strict schema.
//! - `text`: the reference's formatting of amounts, counts and transactions in messages.
//! - `pool`: the transaction pool.
//!
//! With the peer-to-peer wire codec: `wire`, the codecs, the nes frames and the peer statuses.
//!
//! `wire`, `config`, `text` and `pool` each list their operations in a table of their own
//! (`OPS`), which `register` adds to this module's table.
//!
//! With the peer-to-peer client:
//!
//! - `peer_rules`: the address rules, the version gate, the subnet test and the rate limiter.
//! - `peers`: the status attestations of a ping.
//! - `verifier`: the common-block search and the peer verifier.
//! - `monitor`: the network height, the fork decision, the download plan and a listed block's
//!   path to a block.
//!
//! The wire's reply checks and the requests the client writes are in `wire`.
//!
//! With the forger:
//!
//! - `forging`: a delegate's block from its inputs, and from a pool session's collation.
//! - `forger_decide`: the forger's decisions in a slot, and the internal routes' bodies.
//!
//! With the peer-to-peer server:
//!
//! - `server`: the plugin chain's verdicts and the inbound rate limiter.
//! - `relay_round`: the round and slot answers of the forger's internal routes.
//! - `quorum`: the network state the forger reads.
//! - `status_serve`: the served status.
//! - `served`: the public routes' controllers (`p2p.controller`).
//!
//! With the store: `rollback`, `snapshot rollback` of a chain's store (`chain.rollback`).
//!
//! With the API's listings: `state_index` (the state's index orders, `state.indexOrders`),
//! `api_state` (the delegate and wallet listings), `api_page` (the pagination service and its
//! tree), `api_js` (the wallet search and its collation), `api_spec` (the OpenAPI document, the
//! transaction schemas, the fee statistics and the query fuzz target's inputs) and `sql_records`
//! (the store's arithmetic against the reference's PostgreSQL).
//!
//! Most operations run under the devnet configuration the oracle was set up with, which the
//! [`Devnet`] context carries; a case that brings its own milestone file runs under that file.

mod api;
mod api_js;
mod api_page;
mod api_spec;
mod api_state;
mod blocks;
mod config;
mod config_app;
mod digest;
mod forger_decide;
mod forging;
mod genesis;
mod handlers;
mod job;
mod kernel;
mod keys;
mod monitor;
mod operator;
mod peer_rules;
mod peers;
mod pool;
mod processor;
mod quorum;
mod relay_round;
mod rollback;

pub use relay_round::masked as round_wallets_masked;
mod served;
mod server;
mod signatures;
mod sql_records;
mod state;
mod state_index;

pub use state_index::masked as index_orders_masked;
mod status_serve;
mod text;
mod transactions;
mod verifier;
mod wire;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::errors::{BlockDecodeError, BuildError, DecodeError, EncodeError, MemoError};
use heartwood_crypto::managers::{Milestones, Network, Params};
use heartwood_crypto::transactions::deserialiser;
use heartwood_crypto::utils::hex;
use serde_json::Value;

use crate::errors::{Error, Result};
use crate::subject::{Case, Documented, Registry, Subject, SubjectError, Verdict};
use crate::vectors::VectorSet;

pub(crate) use kernel::escaped_line;

/// The vector class whose chain definition holds the devnet's network and milestone files.
const GENESIS_CLASS: &str = "V10-genesis";

/// The devnet configuration a vector set was generated under.
#[derive(Debug, Clone)]
pub struct Devnet {
    /// The network description.
    pub network: Network,
    /// The milestones, loaded.
    pub milestones: Milestones,
    /// The aux the oracle signs transactions with.
    pub aux: [u8; 32],
    /// The time blocks are verified at, in seconds after the epoch. The oracle verifies with its
    /// own clock; [`Devnet::from_vectors`] sets [`VECTOR_VERIFY_TIME`], which is later than every
    /// block of the vectors, as the oracle's clock was when it wrote them. A replay against the
    /// running oracle sets the current time ([`Devnet::at_current_time`]).
    pub verify_time: i64,
}

/// The time the vectors' blocks are verified at, in seconds after the devnet epoch.
pub const VECTOR_VERIFY_TIME: i64 = 1_000_000;

impl Devnet {
    /// The devnet configuration of `set`: the network and milestone files of the chain definition
    /// in the genesis class whose milestone file has the SHA-256 that the meta records name
    /// (`network.milestonesSha256`), and the meta records' `aux`.
    ///
    /// `None` when the set has no genesis class (the oracle writes it only when it is given the
    /// devnet generator). An error when the files disagree about the configuration.
    pub fn from_vectors(set: &VectorSet) -> Result<Option<Devnet>> {
        let problem = |message: String| Error::Vectors {
            path: set.dir.clone(),
            message,
        };
        let Some(genesis) = set.files.iter().find(|f| f.meta.class == GENESIS_CLASS) else {
            return Ok(None);
        };
        let mut wanted = None;
        let mut aux = None;
        for file in set.files.iter().filter(|f| f.meta.class != GENESIS_CLASS) {
            let digest = file.meta.network["milestonesSha256"].as_str();
            let file_aux = file.meta.extra.get("aux").and_then(Value::as_str);
            if wanted.is_some_and(|w| Some(w) != digest) || aux.is_some_and(|a| Some(a) != file_aux)
            {
                return Err(problem(format!(
                    "{} names another devnet configuration than the other classes",
                    file.file_name()
                )));
            }
            wanted = wanted.or(digest);
            aux = aux.or(file_aux);
        }
        let wanted = wanted.ok_or_else(|| problem("no meta record names the milestones".into()))?;
        let aux = aux.ok_or_else(|| problem("no meta record names the aux".into()))?;
        let aux = hex::decode_array::<32>(aux).map_err(|e| problem(format!("aux: {e}")))?;
        let files = genesis
            .records
            .iter()
            .filter_map(|record| match &record.expected {
                crate::vectors::Expected::Output(output) => Some(&output["files"]),
                crate::vectors::Expected::Error(_) => None,
            })
            .find(|files| {
                files["crypto/milestones.json"]
                    .as_str()
                    .is_some_and(|text| hex::encode(&sha256(text.as_bytes())) == wanted)
            })
            .ok_or_else(|| {
                problem(format!(
                    "no chain definition in {GENESIS_CLASS} has the milestone file {wanted}"
                ))
            })?;
        let text = |name: &str| {
            files[name]
                .as_str()
                .ok_or_else(|| problem(format!("{GENESIS_CLASS}: {name} is not text")))
        };
        let network = Network::load(text("crypto/network.json")?)
            .map_err(|e| problem(format!("the devnet network.json: {e}")))?;
        let milestones = Milestones::load(text("crypto/milestones.json")?, &network)
            .map_err(|e| problem(format!("the devnet milestones.json: {e}")))?;
        Ok(Some(Devnet {
            network,
            milestones,
            aux,
            verify_time: VECTOR_VERIFY_TIME,
        }))
    }

    /// The same configuration with blocks verified at the current time, as a running oracle
    /// verifies them.
    pub fn at_current_time(self) -> Result<Devnet> {
        let epoch = self.milestones.at(1).epoch();
        let epoch_ms = slots::epoch_unix_ms(epoch)
            .ok_or_else(|| Error::Usage(format!("the devnet epoch {epoch} does not parse")))?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .ok_or_else(|| Error::Usage("the system clock is before 1970".into()))?;
        Ok(Devnet {
            verify_time: slots::epoch_time(now_ms, epoch_ms),
            ..self
        })
    }
}

/// What a subject reads from: the devnet configuration, when known.
struct Env<'a> {
    devnet: Option<&'a Devnet>,
}

impl Env<'_> {
    fn devnet(&self) -> Outcome<&Devnet> {
        self.devnet.ok_or_else(|| {
            SubjectError::Unsupported(
                "the devnet configuration is unknown: the vector set has no genesis class".into(),
            )
        })
    }

    /// The milestones a case runs under: its own milestone file (`input.milestones`, JSON text),
    /// or the devnet's.
    fn milestones(&self, case: &Case<'_>) -> Outcome<Cow<'_, Milestones>> {
        let devnet = self.devnet()?;
        match case.input.get("milestones") {
            None | Some(Value::Null) => Ok(Cow::Borrowed(&devnet.milestones)),
            Some(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| failed("input.milestones is not text"))?;
                Milestones::load(text, &devnet.network)
                    .map(Cow::Owned)
                    .map_err(|e| failed(format!("the case's milestones do not load: {e}")))
            }
        }
    }
}

/// A subject's result.
type Outcome<T> = std::result::Result<T, SubjectError>;

/// The function that answers one operation.
type Run = fn(&Env<'_>, &Case<'_>) -> Outcome<Verdict>;

/// One registered operation.
struct Op {
    op: &'static str,
    devnet: Option<Arc<Devnet>>,
    run: Run,
}

impl Subject for Op {
    fn op(&self) -> &'static str {
        self.op
    }

    fn run(&self, case: &Case<'_>) -> Outcome<Verdict> {
        let env = Env {
            devnet: self.devnet.as_deref(),
        };
        (self.run)(&env, case)
    }
}

/// Every operation, with the function that answers it.
const OPS: &[(&str, Run)] = &[
    ("keys.fromPassphrase", keys::from_passphrase),
    ("address.fromPublicKey", keys::address_from_public_key),
    ("address.validate", keys::address_validate),
    ("address.toBuffer", keys::address_to_buffer),
    ("publicKey.verify", keys::public_key_verify),
    ("sig.bip340.sign", signatures::bip340_sign),
    ("sig.bip340.verify", signatures::bip340_verify),
    ("sig.legacy.sign", signatures::legacy_sign),
    ("sig.legacy.verify", signatures::legacy_verify),
    ("tx.preimages", signatures::transaction_preimages),
    ("status.preimage", signatures::status_preimage),
    ("external.file", signatures::external_file),
    ("bip39.entropyToMnemonic", signatures::entropy_to_mnemonic),
    ("tx.build", transactions::build),
    ("tx.fromBytes", transactions::from_bytes),
    ("tx.fromJson", transactions::from_json),
    ("handler.activation", transactions::handler_activation),
    ("vote.validate", transactions::vote_validate),
    ("vote.sort", transactions::vote_sort),
    ("vote.splits", transactions::vote_splits),
    ("vote.shares", transactions::vote_shares),
    ("block.make", blocks::make),
    ("block.fromBytes", blocks::from_bytes),
    ("block.receive", blocks::receive),
    ("fee.burned", blocks::fee_burned),
    ("reward.calculate", blocks::reward_calculate),
    ("reward.donations", blocks::reward_donations),
    ("genesis.generate", genesis::generate),
    ("round.calculate", kernel::round_calculate),
    ("round.isNew", kernel::round_is_new),
    ("forging.info", kernel::forging_info),
    ("block.isChained", kernel::block_is_chained),
    ("kernel.stringify", kernel::stringify),
    ("kernel.dotenv", kernel::dotenv),
    ("kernel.formatTimestamp", kernel::format_timestamp),
    ("kernel.ip", kernel::ip),
    ("kernel.paths", kernel::paths),
    ("logger.format", kernel::logger_format),
    ("state.shuffle", state::shuffle),
    ("state.rank", state::rank),
    ("wallet.voteAmounts", state::vote_amounts),
    ("state.digest", digest::state_digest),
    ("fee.minimum", handlers::fee_minimum),
    ("state.applyTransaction", handlers::apply_transaction),
];

/// Every operation: [`OPS`], then the tables of the wire, `config`, `text`, `pool`, the peer
/// network, the processor and the job, the API and the node's configuration, the forger, the
/// server, the store's rollback, the API's listings and the operator commands' files.
fn all_ops() -> impl Iterator<Item = &'static (&'static str, Run)> {
    OPS.iter()
        .chain(wire::OPS)
        .chain(config::OPS)
        .chain(text::OPS)
        .chain(pool::OPS)
        .chain(peer_rules::OPS)
        .chain(peers::OPS)
        .chain(verifier::OPS)
        .chain(monitor::OPS)
        .chain(processor::OPS)
        .chain(job::OPS)
        .chain(api::OPS)
        .chain(config_app::OPS)
        .chain(forging::OPS)
        .chain(forger_decide::OPS)
        .chain(server::OPS)
        .chain(relay_round::OPS)
        .chain(quorum::OPS)
        .chain(status_serve::OPS)
        .chain(served::OPS)
        .chain(rollback::OPS)
        .chain(state_index::OPS)
        .chain(api_state::OPS)
        .chain(api_page::OPS)
        .chain(api_js::OPS)
        .chain(api_spec::OPS)
        .chain(sql_records::OPS)
        .chain(operator::OPS)
}

/// Register every operation of [`all_ops`] in `registry`.
pub(crate) fn register(registry: &mut Registry, devnet: Option<Devnet>) {
    let devnet = devnet.map(Arc::new);
    for &(op, run) in all_ops() {
        // The table names each operation once (a unit test checks it), so this cannot fail.
        let _ = registry.register(Box::new(Op {
            op,
            devnet: devnet.clone(),
            run,
        }));
    }
}

// Reading a case's input. A malformed input is a harness problem, not a Heartwood verdict.

fn failed(message: impl Into<String>) -> SubjectError {
    SubjectError::Failed(message.into())
}

fn text<'a>(input: &'a Value, key: &str) -> Outcome<&'a str> {
    input[key]
        .as_str()
        .ok_or_else(|| failed(format!("input.{key} is not text")))
}

/// The hex text of `input.key` as bytes; `None` when it is not hex, which the reference refuses.
fn hex_bytes(input: &Value, key: &str) -> Outcome<Option<Vec<u8>>> {
    Ok(hex::decode(text(input, key)?).ok())
}

fn bytes32(input: &Value, key: &str) -> Outcome<[u8; 32]> {
    hex::decode_array(text(input, key)?).map_err(|e| failed(format!("input.{key}: {e}")))
}

fn integer<T: TryFrom<u64>>(input: &Value, key: &str) -> Outcome<T> {
    input[key]
        .as_u64()
        .and_then(|n| T::try_from(n).ok())
        .ok_or_else(|| failed(format!("input.{key} is not an integer in range")))
}

fn amount(input: &Value, key: &str) -> Outcome<u64> {
    text(input, key)?
        .parse()
        .map_err(|_| failed(format!("input.{key} is not an amount")))
}

fn height(case: &Case<'_>) -> Outcome<u32> {
    u32::try_from(case.height).map_err(|_| failed("the height is over 2^32 - 1"))
}

/// The parameters at the case's height under the milestones it runs under.
fn params<'m>(milestones: &'m Milestones, case: &Case<'_>) -> Outcome<&'m Params> {
    Ok(milestones.at(height(case)?))
}

/// A rejection with the reference's error class `class`, which L0 compares.
fn rejected(class: &'static str) -> Verdict {
    Verdict::Rejected {
        class: Some(class),
        detail: class.to_owned(),
    }
}

/// A rejection whose error class is not observable: only the verdict is compared.
fn unclassified(error: impl std::fmt::Debug) -> Verdict {
    Verdict::Rejected {
        class: None,
        detail: format!("{error:?}"),
    }
}

// The reference's error classes for the crate's errors, where the class is observable.

/// The error class the reference raises when it refuses transaction bytes.
fn decode_class(error: &DecodeError) -> &'static str {
    match error {
        DecodeError::Schema(_) => "TransactionSchemaError",
        DecodeError::UnsupportedVersion(_) => "TransactionVersionError",
        _ => "InvalidTransactionBytesError",
    }
}

/// The error class the reference raises when it refuses to build a transaction or read its JSON,
/// where it is observable.
fn build_class(error: &BuildError) -> Option<&'static str> {
    match error {
        BuildError::Schema(_) => Some("TransactionSchemaError"),
        BuildError::Decode(error) => Some(decode_class(error)),
        BuildError::Memo(MemoError::TooLong { .. }) => Some("MemoLengthExceededError"),
        BuildError::TooManyTransfers { .. } => Some("MaximumTransferCountExceededError"),
        BuildError::Encode(EncodeError::VoteAssetTooLarge { .. }) => Some("VoteAssetTooLargeError"),
        _ => None,
    }
}

/// A refused build or JSON read: its class when it is observable, else the verdict only.
fn build_refused(error: &BuildError) -> Verdict {
    match build_class(error) {
        Some(class) => rejected(class),
        None => unclassified(error),
    }
}

/// The documented difference of a decode refusal, if it is one. The reference decodes these
/// forms, and all but the memo it refuses only when the transaction is applied.
fn documented(error: &DecodeError) -> Option<Documented> {
    match error {
        DecodeError::RemovedType { .. } => Some(Documented::RemovedType),
        DecodeError::UnsupportedHeaderType(1) => Some(Documented::ExtendedHeader),
        DecodeError::InvalidMemo => Some(Documented::NonUtf8Memo),
        DecodeError::MultiSignature => Some(Documented::MultiSignature),
        _ => None,
    }
}

/// A refused transaction decode of `bytes`: a documented difference, with its form checked on
/// `bytes`, or the reference's error class.
fn decode_refused(error: &DecodeError, bytes: &[u8]) -> Verdict {
    match documented(error) {
        Some(difference) => Verdict::RefusedByDesign {
            difference,
            holds: has_form(difference, bytes),
        },
        None => rejected(decode_class(error)),
    }
}

// The documented forms, checked on the transaction bytes with the fixed offsets of the wire
// format rather than with the decoder whose refusal they explain.

/// The header type byte of the standard header and of the extended header.
const STANDARD_HEADER: u8 = 0xff;
const EXTENDED_HEADER: u8 = 0xfe;
/// Where the type group, the type and the memo length are in a transaction with the standard
/// header.
const TYPE_GROUP_AT: usize = 3;
const TYPE_AT: usize = 7;
const MEMO_AT: usize = 58;
/// Bytes of a multi-signature entry.
const MULTI_SIGNATURE_ENTRY: usize = 65;

/// Whether the transaction `bytes` have the form of `difference`.
fn has_form(difference: Documented, bytes: &[u8]) -> bool {
    let standard = bytes.first() == Some(&STANDARD_HEADER);
    match difference {
        // The wire's differences are shown by the oracle's answer, not by a transaction.
        Documented::PluginsPrototype | Documented::DeepGroups | Documented::WideAmount => false,
        Documented::ExtendedHeader => bytes.first() == Some(&EXTENDED_HEADER),
        Documented::RemovedType => {
            let group = bytes
                .get(TYPE_GROUP_AT..TYPE_GROUP_AT + 4)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map(u32::from_le_bytes);
            let kind = bytes
                .get(TYPE_AT..TYPE_AT + 2)
                .and_then(|b| <[u8; 2]>::try_from(b).ok())
                .map(u16::from_le_bytes);
            standard && group == Some(1) && matches!(kind, Some(0 | 3 | 4 | 5 | 8 | 9 | 10))
        }
        Documented::NonUtf8Memo => {
            let length = bytes.get(MEMO_AT).map(|&n| usize::from(n));
            let memo = length.and_then(|n| bytes.get(MEMO_AT + 1..MEMO_AT + 1 + n));
            standard && memo.is_some_and(|memo| std::str::from_utf8(memo).is_err())
        }
        Documented::MultiSignature => (1..=bytes.len() / MULTI_SIGNATURE_ENTRY).any(|entries| {
            let end = bytes.len() - entries * MULTI_SIGNATURE_ENTRY;
            bytes
                .get(..end)
                .is_some_and(|prefix| deserialiser::deserialise(prefix).is_ok())
        }),
        // The other differences are not forms of a transaction's bytes.
        Documented::VotesWithIndices
        | Documented::FeeKeyValueType
        | Documented::DecodeReason
        | Documented::FullPoolEviction
        | Documented::SavedStateUsername
        | Documented::RoundRowsAfterRevert
        | Documented::Locale
        | Documented::OnlineView
        | Documented::StrictSchema
        | Documented::UnlinkedForkSegment
        | Documented::OwnForkSegment
        | Documented::MissedRowsOfRefusedBlock
        | Documented::EscapedLogText
        | Documented::Api(_)
        | Documented::OverHeightOwnBlock
        | Documented::RoundWallets
        | Documented::ReplayIndexOrder
        | Documented::CliEnvParser => false,
    }
}

/// The bytes of transaction `index` in the block `bytes`, read with the fixed offsets of the
/// block format: the header and signature, one `u32` length per transaction, then the
/// transactions.
fn block_transaction(bytes: &[u8], index: usize) -> Option<&[u8]> {
    const COUNT_AT: usize = 44;
    const TRANSACTIONS_AT: usize = 141 + 64;
    let count = usize::try_from(u32::from_le_bytes(
        bytes.get(COUNT_AT..COUNT_AT + 4)?.try_into().ok()?,
    ))
    .ok()?;
    let length_at = |i: usize| -> Option<usize> {
        let at = TRANSACTIONS_AT + 4 * i;
        usize::try_from(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?)).ok()
    };
    let mut start = TRANSACTIONS_AT.checked_add(count.checked_mul(4)?)?;
    for i in 0..index {
        start = start.checked_add(length_at(i)?)?;
    }
    bytes.get(start..start.checked_add(length_at(index)?)?)
}

/// A refused block decode of `bytes`: a transaction's documented difference, with its form
/// checked on that transaction's bytes, or a refusal whose class is not observable.
fn block_refused(error: &BlockDecodeError, bytes: &[u8]) -> Verdict {
    if let BlockDecodeError::Transaction { index, source } = error
        && let Some(difference) = documented(source)
    {
        return Verdict::RefusedByDesign {
            difference,
            holds: block_transaction(bytes, *index)
                .is_some_and(|transaction| has_form(difference, transaction)),
        };
    }
    unclassified(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_is_registered_once() {
        let mut ops: Vec<&str> = all_ops().map(|(op, _)| *op).collect();
        let registry = crate::subject::heartwood(None);
        assert_eq!(registry.ops().count(), ops.len());
        let count = ops.len();
        ops.sort_unstable();
        ops.dedup();
        assert_eq!(ops.len(), count);
    }

    #[test]
    fn without_the_devnet_rules_cases_are_skipped() {
        let registry = crate::subject::heartwood(None);
        let input = serde_json::json!({"hex": "ff"});
        let case = Case {
            op: "tx.fromBytes",
            network: "devnet",
            height: 2,
            input: &input,
        };
        let subject = registry.get("tx.fromBytes").unwrap();
        assert!(matches!(
            subject.run(&case),
            Err(SubjectError::Unsupported(_))
        ));
    }
}
