//! The forge replay (`difftest run l2 DIR --forge`): every block of a corpus rebuilt from its own
//! inputs, and compared byte for byte with the stored block.
//!
//! - **Reference to Heartwood** (the default): each block's forger key is derived from the corpus
//!   seed, the forger aux is the manifest's when it records one, else the one derived from the seed
//!   (`heartwood_crypto::genesis::derive::aux(tag, "forger", seed)`, the devnet tooling's
//!   default); the previous block, the transactions in block order, the timestamp and the reward
//!   go through `heartwood_forger::Delegate::forge`. A corpus whose first forged block does not
//!   rebuild with that aux is reported as "aux unknown", never as passed, unless that aux
//!   reproduces the block's own signature: then the forging differs, the block is reported as a
//!   mismatch and the replay goes on.
//! - **Heartwood to reference** (`--oracle`): the oracle's `forger.forge` rebuilds every block of
//!   a chain Heartwood forged, from the same inputs, under the corpus's milestone file.
//!
//! The transactions are read back from each block's bytes as the forger reads its relay's answer
//! (`fromBytesUnsafe`, at the previous block's height). The manifest records a forger aux as
//! `forgerAux` (64 hex digits) in `corpus.json`.

use std::sync::Arc;

use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::crypto::sig::{self, SchemeId, SigningDomain};
use heartwood_crypto::genesis::derive;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, Block, BlockId, KeyPair, PublicKeyBytes, Transaction};
use heartwood_forger::{Delegate, ForgeInputs};
use serde_json::{Value, json};

use crate::compare::diff;
use crate::corpus::Corpus;
use crate::errors::{Error, Result};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome, Report};
use crate::synthetic::DERIVATION_TAG;

/// Which side rebuilds the corpus's blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Heartwood rebuilds a chain the reference forged.
    Heartwood,
    /// The reference rebuilds a chain Heartwood forged.
    Reference,
}

/// What a forge replay covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgeOptions {
    /// Which side rebuilds the blocks.
    pub direction: Direction,
    /// The last height replayed.
    pub to: Option<u32>,
}

/// The label of the forger aux in the devnet tooling's derivation.
const AUX_LABEL: &str = "forger";

/// The roles whose keys the devnet tooling derives from its seed, any of which can forge.
fn roles() -> Vec<String> {
    let fixed = ["generator", "genesis-1", "genesis-2", "team-placeholder-1"];
    (fixed.iter().map(|r| (*r).to_owned()))
        .chain((1..=53).map(|k| format!("delegate/{k}")))
        .chain((1..=20).map(|k| format!("wallet/{k}")))
        .collect()
}

/// Replay `corpus`'s blocks through the forging side of `options`; the oracle is needed for
/// [`Direction::Reference`].
pub fn run(
    corpus: &Corpus,
    options: &ForgeOptions,
    oracle: Option<&mut OracleServer>,
) -> Result<Report> {
    let problem = |message: String| Error::Usage(format!("{}: {message}", corpus.name()));
    let seed = (corpus.meta.get("seed").and_then(Value::as_str))
        .ok_or_else(|| problem("the manifest has no seed".into()))?;
    let (aux, aux_source) = match corpus.meta.get("forgerAux").and_then(Value::as_str) {
        Some(text) => (
            hex::decode_array::<32>(text).map_err(|e| problem(format!("forgerAux: {e}")))?,
            "the manifest's",
        ),
        None => (derive::aux(DERIVATION_TAG, AUX_LABEL, seed), "the seed's"),
    };
    let network = corpus.network.pub_key_hash;
    let keys: Vec<Arc<KeyPair>> = (roles().iter())
        .map(|role| derive::role_key(DERIVATION_TAG, role, seed, network).map(|k| Arc::new(k.keys)))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| problem(format!("the seed's keys: {e}")))?;
    let milestones_text = match options.direction {
        Direction::Heartwood => None,
        Direction::Reference => Some(
            std::fs::read_to_string(corpus.dir.join("crypto/milestones.json"))
                .map_err(|e| problem(format!("crypto/milestones.json: {e}")))?,
        ),
    };
    let mut oracle = match (options.direction, oracle) {
        (Direction::Heartwood, _) => None,
        (Direction::Reference, Some(oracle)) => Some(oracle),
        (Direction::Reference, None) => {
            return Err(problem("the reference's rebuild needs the oracle".into()));
        }
    };
    let last = options.to.unwrap_or(u32::MAX).min(corpus.tip());
    let mut results = Vec::new();
    for stored in corpus.blocks.iter().filter(|b| b.height <= last) {
        let block = Block::decode(&stored.bytes, &corpus.milestones)
            .map_err(|e| problem(format!("block {}: {e}", stored.height)))?;
        let generator = block.header().generator_public_key;
        let Some(key) = keys
            .iter()
            .find(|k| k.public_key().to_compressed() == generator)
        else {
            results.push(result(corpus, stored.height, unknown_key(&generator)));
            continue;
        };
        let outcome = match oracle.as_deref_mut() {
            None => rebuild(corpus, &stored.bytes, &block, key, aux),
            Some(server) => {
                let text = milestones_text.as_deref().unwrap_or_default();
                by_reference(server, &stored.bytes, &block, key, aux, text)?
            }
        };
        // A mismatch at the first block is the corpus's forger aux only when that aux does not
        // reproduce the stored signature over the stored header; when it does, the forging
        // differs, and the rest of the chain is examined.
        let first = results.is_empty() && stored.height == 2;
        match outcome {
            Outcome::Mismatch(_) if first && !signs_as_stored(&block, key, aux) => {
                results.push(result(
                    corpus,
                    stored.height,
                    Outcome::HarnessError(format!(
                        "aux unknown: block 2 does not rebuild with {aux_source} forger aux"
                    )),
                ));
                break;
            }
            outcome => results.push(result(corpus, stored.height, outcome)),
        }
    }
    let side = match options.direction {
        Direction::Heartwood => "Heartwood rebuilds the reference's blocks",
        Direction::Reference => "the reference rebuilds Heartwood's blocks",
    };
    Ok(Report {
        title: "L2 forge replay".into(),
        source: format!(
            "{}: {side}, blocks 2 to {last}, {aux_source} forger aux",
            corpus.name()
        ),
        files: 1,
        results,
    })
}

/// Whether signing `block`'s own header with `key` and `aux` gives the signature the block
/// stores: the aux is the one its forger used, whatever else differs in a rebuild of it.
fn signs_as_stored(block: &Block, key: &KeyPair, aux: [u8; 32]) -> bool {
    let header = block.header();
    let signed = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &serialiser::signing_hash(header),
        key.secret_key(),
        Aux::fixed(aux),
    );
    signed.is_ok_and(|signature| signature == header.block_signature)
}

fn result(corpus: &Corpus, height: u32, outcome: Outcome) -> CaseResult {
    CaseResult {
        file: corpus.name(),
        line: usize::try_from(height).unwrap_or(usize::MAX),
        op: "forger.forge".into(),
        label: format!("block {height}"),
        height: u64::from(height),
        outcome,
    }
}

fn unknown_key(generator: &PublicKeyBytes) -> Outcome {
    Outcome::HarnessError(format!(
        "the generator {} is no key the seed derives",
        generator.to_hex()
    ))
}

/// The block's transactions as the forger reads them from its relay: from their bytes, under the
/// rules of the previous block's height.
fn forger_transactions(
    corpus: &Corpus,
    block: &Block,
) -> std::result::Result<Vec<Transaction>, String> {
    let params = corpus.milestones.at(block.height().saturating_sub(1));
    (block.transactions().iter())
        .map(|t| Transaction::decode_trusted(t.bytes(), params).map_err(|e| e.to_string()))
        .collect()
}

/// Heartwood's rebuild of `block` with `key` and `aux`, compared with its stored bytes.
fn rebuild(
    corpus: &Corpus,
    stored: &[u8],
    block: &Block,
    key: &Arc<KeyPair>,
    aux: [u8; 32],
) -> Outcome {
    let transactions = match forger_transactions(corpus, block) {
        Ok(transactions) => transactions,
        Err(e) => return Outcome::HarnessError(format!("a transaction does not read back: {e}")),
    };
    let header = block.header();
    let inputs = ForgeInputs {
        transactions: &transactions,
        previous_id: BlockId::from_array(header.previous_block),
        previous_height: block.height().saturating_sub(1),
        timestamp: header.timestamp,
        reward: header.reward,
    };
    let delegate = Delegate::new(key.clone(), corpus.network.pub_key_hash);
    match delegate.forge(&inputs, Aux::fixed(aux), &corpus.milestones) {
        Ok(rebuilt) => compare(stored, block, &rebuilt.encode(), &rebuilt.id().to_hex()),
        Err(e) => Outcome::VerdictMismatch {
            oracle: "accepted".into(),
            heartwood: format!("refused ({e})"),
        },
    }
}

/// The reference's rebuild of `block` through the oracle's `forger.forge`.
fn by_reference(
    server: &mut OracleServer,
    stored: &[u8],
    block: &Block,
    key: &KeyPair,
    aux: [u8; 32],
    milestones: &str,
) -> Result<Outcome> {
    let header = block.header();
    let previous = block.height().saturating_sub(1);
    let transactions: Vec<String> = (block.transactions().iter())
        .map(|t| hex::encode(t.bytes()))
        .collect();
    let input = json!({
        "privateKey": key.secret_key().to_hex().as_str(),
        "aux": hex::encode(&aux),
        "previousBlock": {"id": hex::encode(&header.previous_block), "height": previous},
        "timestamp": header.timestamp,
        "reward": header.reward.to_string(),
        "transactions": transactions,
        "milestones": milestones,
    });
    Ok(
        match server.call("forger.forge", "devnet", u64::from(previous), &input)? {
            Ok(output) => {
                let bytes = (output["hex"].as_str())
                    .and_then(|text| hex::decode(text).ok())
                    .unwrap_or_default();
                compare(
                    stored,
                    block,
                    &bytes,
                    output["id"].as_str().unwrap_or_default(),
                )
            }
            Err(error) => Outcome::VerdictMismatch {
                oracle: format!("refused ({}: {})", error.class, error.message),
                heartwood: "forged".into(),
            },
        },
    )
}

/// The `stored` bytes of `block` against rebuilt `bytes` and `id`.
fn compare(stored: &[u8], block: &Block, bytes: &[u8], id: &str) -> Outcome {
    let expected = json!({"id": block.id().to_hex(), "hex": hex::encode(stored)});
    let actual = json!({"id": id, "hex": hex::encode(bytes)});
    let differences = diff(&expected, &actual);
    if differences.is_empty() {
        Outcome::Pass
    } else {
        Outcome::Mismatch(differences)
    }
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::blocks::factory;

    use super::*;

    fn committed() -> Corpus {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/txgen-one-node");
        Corpus::load(&dir).unwrap()
    }

    const HEARTWOOD: ForgeOptions = ForgeOptions {
        direction: Direction::Heartwood,
        to: Some(12),
    };

    #[test]
    fn the_committed_corpus_rebuilds_with_the_seeds_forger_aux() {
        let report = run(&committed(), &HEARTWOOD, None).unwrap();
        assert_eq!((report.compared(), report.failed()), (11, 0));
        assert!(
            report.source.contains("the seed's forger aux"),
            "{}",
            report.source
        );
    }

    #[test]
    fn another_aux_is_reported_as_unknown_and_never_passes() {
        let mut corpus = committed();
        corpus.meta["forgerAux"] = json!("00".repeat(32));
        let report = run(&corpus, &HEARTWOOD, None).unwrap();
        assert_eq!(report.results.len(), 1);
        assert!(!report.is_success());
        assert!(matches!(&report.results[0].outcome,
            Outcome::HarnessError(message) if message.starts_with("aux unknown")));
        corpus.meta["forgerAux"] = json!("zz");
        assert!(run(&corpus, &HEARTWOOD, None).is_err());
    }

    /// The committed corpus with block 2 replaced by a block its forger signed with the manifest's
    /// aux over a header that states a fee the transactions do not add up to: no forging of the
    /// same inputs gives it, and the aux is right.
    fn with_a_miscounted_block_two() -> Corpus {
        let mut corpus = committed();
        let seed = corpus.meta["seed"].as_str().unwrap().to_owned();
        let aux = derive::aux(DERIVATION_TAG, AUX_LABEL, &seed);
        let stored = corpus.blocks[0].bytes.clone();
        let block = Block::decode(&stored, &corpus.milestones).unwrap();
        let generator = block.header().generator_public_key;
        let network = corpus.network.pub_key_hash;
        let key = roles()
            .iter()
            .filter_map(|role| derive::role_key(DERIVATION_TAG, role, &seed, network).ok())
            .find(|k| k.keys.public_key().to_compressed() == generator)
            .expect("block 2's generator is a derived key")
            .keys;
        let mut header = block.header().clone();
        header.total_fee += 1;
        header.block_signature = sig::sign(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Block,
            &serialiser::signing_hash(&header),
            key.secret_key(),
            Aux::fixed(aux),
        )
        .unwrap();
        let changed = factory::from_data(header, block.transactions(), &corpus.milestones).unwrap();
        corpus.blocks[0].bytes = changed.encode();
        corpus
    }

    #[test]
    fn a_forging_defect_at_block_two_is_a_mismatch_and_the_replay_goes_on() {
        let report = run(&with_a_miscounted_block_two(), &HEARTWOOD, None).unwrap();
        assert!(report.results.len() > 1, "the replay stopped: {report}");
        assert!(matches!(&report.results[0].outcome, Outcome::Mismatch(_)));
        assert_eq!((report.compared(), report.failed()), (11, 1), "{report}");
    }

    #[test]
    fn the_references_rebuild_needs_the_oracle() {
        let options = ForgeOptions {
            direction: Direction::Reference,
            to: None,
        };
        assert!(run(&committed(), &options, None).is_err());
    }
}
