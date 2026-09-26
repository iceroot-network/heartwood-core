//! Heartwood's subjects: the oracle's operations answered with `heartwood-crypto`.
//!
//! Each subject turns a case's input into the crate's types, runs the crate, and writes the result
//! in the JSON shape the oracle writes (`tools/oracle/lib/ops.js`), so that L0 can compare them
//! value for value.
//!
//! - [`keys`]: passphrase keys, public keys and addresses.
//! - [`signatures`]: BIP340 and legacy Schnorr, transaction preimages, the published vector files
//!   and BIP39 mnemonics.
//! - [`transactions`]: the builders, both decoders, the JSON reader, the handler table and the vote
//!   rules.
//! - [`blocks`]: the block factory and decoder, rewards, donations, the fee burn and the milestone
//!   loader.
//! - [`genesis`]: the devnet chain-definition builder.
//!
//! Most operations run under the devnet configuration the oracle was set up with, which the
//! [`Devnet`] context carries; a case that brings its own milestone file runs under that file.

mod blocks;
mod genesis;
mod keys;
mod signatures;
mod transactions;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::errors::{BuildError, DecodeError, EncodeError, MemoError};
use heartwood_crypto::managers::{Milestones, Network, Params};
use heartwood_crypto::utils::hex;
use serde_json::Value;

use crate::errors::{Error, Result};
use crate::subject::{Case, Registry, Subject, SubjectError, Verdict};
use crate::vectors::VectorSet;

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
    ("block.make", blocks::make),
    ("block.fromBytes", blocks::from_bytes),
    ("fee.burned", blocks::fee_burned),
    ("fee.minimum", blocks::fee_minimum),
    ("reward.calculate", blocks::reward_calculate),
    ("reward.donations", blocks::reward_donations),
    ("config.load", blocks::config_load),
    ("genesis.generate", genesis::generate),
];

/// Register every operation of [`OPS`] in `registry`.
pub(crate) fn register(registry: &mut Registry, devnet: Option<Devnet>) {
    let devnet = devnet.map(Arc::new);
    for &(op, run) in OPS {
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

fn rejected(class: impl Into<String>) -> Verdict {
    Verdict::Rejected {
        class: class.into(),
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

/// The error class the reference raises when it refuses to build a transaction or read its JSON.
fn build_class(error: &BuildError) -> String {
    match error {
        BuildError::Schema(_) => "TransactionSchemaError".into(),
        BuildError::Decode(error) => decode_class(error).into(),
        BuildError::Memo(MemoError::TooLong { .. }) => "MemoLengthExceededError".into(),
        BuildError::TooManyTransfers { .. } => "MaximumTransferCountExceededError".into(),
        BuildError::Encode(EncodeError::VoteAssetTooLarge { .. }) => {
            "VoteAssetTooLargeError".into()
        }
        other => format!("{other:?}"),
    }
}

/// The decode refusals that are documented differences from the reference, with the reference's
/// behaviour: it decodes these forms and refuses them only when the transaction is applied, so a
/// block that holds one is refused either way.
fn refused_by_design(error: &DecodeError) -> Option<&'static str> {
    match error {
        DecodeError::RemovedType { .. } => Some(
            "a removed type: the reference decodes it and refuses it when it is applied (its \
             handler is inactive)",
        ),
        DecodeError::UnsupportedHeaderType(_) => {
            Some("the extended header: the reference decodes it and refuses it when it is applied")
        }
        DecodeError::InvalidMemo => Some(
            "a memo that is not valid UTF-8: the reference replaces the bad bytes, which changes \
             the bytes it hashes, and accepts it",
        ),
        DecodeError::MultiSignature => Some(
            "multi-signature entries after the signatures: the reference reads them and refuses \
             the transaction when it is applied (no wallet can register a multi-signature)",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_is_registered_once() {
        let mut ops: Vec<&str> = OPS.iter().map(|(op, _)| *op).collect();
        let registry = crate::subject::heartwood(None);
        assert_eq!(registry.ops().count(), ops.len());
        ops.sort_unstable();
        ops.dedup();
        assert_eq!(ops.len(), OPS.len());
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
