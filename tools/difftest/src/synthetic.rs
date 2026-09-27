//! Blocks built on top of a corpus chain for the synthetic L2 cases.
//!
//! The builder signs blocks and transactions with keys derived at run time from the corpus's
//! public devnet seed, and the same bytes go to the oracle's session and to Heartwood's state.
//! Each case starts from the corpus's last block on both sides (the oracle replays the corpus in
//! a chain session; Heartwood applies it to a fresh state), runs its blocks and reverts, and
//! compares after every step: the verdict, the error class where both refuse a transaction, the
//! missed slots detected before the block, the reward, and the digest line of an accepted block
//! or the records of a refused one (which must leave the state as it was).
//!
//! Blocks are built as the scheduled delegate of their slot builds them: the reward of the block
//! in its round, the donations and the fee burn of the milestone at its height, and a timestamp
//! in the first slot at or after the requested gap whose delegate's key the seed derives.

use heartwood_crypto::blocks::{BlockHeader, serialiser};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use heartwood_crypto::genesis::DerivedKey;
use heartwood_crypto::genesis::derive::role_key;
use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, Block, Milestones, Transaction};
use heartwood_kernel::utils::calculate_forging_info::calculate_forging_info;
use heartwood_kernel::utils::round_calculator::is_new_round;
use heartwood_state::State;
use heartwood_state::digest::{self, Step};
use heartwood_state::errors::ApplyError;
use heartwood_state::round_state::MissedBlock;
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use crate::compare;
use crate::corpus::Corpus;
use crate::digests;
use crate::errors::{Error, Result};
use crate::levels::l2::{call, corpus_crypto};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};

/// A synthetic case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntheticCase {
    /// A block whose second transaction overspends, after missed slots: refused, wallets
    /// unchanged.
    OverspendAfterGap,
    /// A rule that changes at the next height: accepted at one height and refused at the next.
    RuleHeight,
    /// A vote in the last block of a round, which changes the next round's ranks and schedule.
    VoteAtRoundEnd,
    /// Burns at the minimum amount and one below it.
    BurnMinimum,
    /// A delegate registered in a round, then a revert across the round's end.
    RegistrationAcrossRoundEnd,
    /// A block by a key that is not a delegate: refused.
    NonDelegateGenerator,
    /// A second-signature registration, then a transfer without the second signature.
    MissingSecondSignature,
    /// Fees at the dynamic fee floor and one below it, in a block.
    FeeFloorInBlock,
    /// The fee floor, the burn minimum and the revoke delay change at a height: the same
    /// transactions are accepted at the height before and refused at it, or the other way round.
    HandlerRuleHeight,
    /// Two blocks after a round end, then a revert of three blocks across it and a competing
    /// chain from there, with other slots and other transactions.
    ForkAcrossRoundEnd,
}

impl SyntheticCase {
    /// Every case.
    pub const ALL: [SyntheticCase; 10] = [
        SyntheticCase::OverspendAfterGap,
        SyntheticCase::RuleHeight,
        SyntheticCase::VoteAtRoundEnd,
        SyntheticCase::BurnMinimum,
        SyntheticCase::RegistrationAcrossRoundEnd,
        SyntheticCase::NonDelegateGenerator,
        SyntheticCase::MissingSecondSignature,
        SyntheticCase::FeeFloorInBlock,
        SyntheticCase::HandlerRuleHeight,
        SyntheticCase::ForkAcrossRoundEnd,
    ];

    /// The case's name in reports.
    pub const fn name(self) -> &'static str {
        match self {
            SyntheticCase::OverspendAfterGap => "overspend-after-gap",
            SyntheticCase::RuleHeight => "rule-height",
            SyntheticCase::VoteAtRoundEnd => "vote-at-round-end",
            SyntheticCase::BurnMinimum => "burn-minimum",
            SyntheticCase::RegistrationAcrossRoundEnd => "registration-across-round-end",
            SyntheticCase::NonDelegateGenerator => "non-delegate-generator",
            SyntheticCase::MissingSecondSignature => "missing-second-signature",
            SyntheticCase::FeeFloorInBlock => "fee-floor-in-block",
            SyntheticCase::HandlerRuleHeight => "handler-rule-height",
            SyntheticCase::ForkAcrossRoundEnd => "fork-across-round-end",
        }
    }
}

/// The key-derivation tag of the devnet tooling.
pub const DERIVATION_TAG: &str = "solar-devnet-v1/";
/// The devnet tooling's default key-derivation seed, which is public.
pub const DEVNET_SEED: &str = "solar-devnet";
/// The aux of every synthetic signature.
const AUX: [u8; 32] = [0x5a; 32];
/// A fee above every floor of the devnet for the small transactions of the cases.
const FEE: u64 = 20_000_000;
/// The fee of a delegate registration (the devnet's surcharge is about 75 coins).
const REGISTRATION_FEE: u64 = 7_600_000_000;

/// The keys the seed derives: the generator, the delegates, the genesis and test wallets.
pub struct Keys {
    keys: Vec<DerivedKey>,
}

impl Keys {
    /// Derive the keys of `seed` for the network byte `network`.
    pub fn derive(seed: &str, network: u8) -> Result<Keys> {
        let mut roles: Vec<String> = vec![
            "generator".into(),
            "genesis-1".into(),
            "genesis-2".into(),
            "team-placeholder-1".into(),
        ];
        roles.extend((1..=53).map(|k| format!("delegate/{k}")));
        roles.extend((1..=20).map(|k| format!("wallet/{k}")));
        let keys = roles
            .iter()
            .map(|role| role_key(DERIVATION_TAG, role, seed, network))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Keys { keys })
    }

    fn by_public_key(&self, key: &PublicKeyBytes) -> Option<&DerivedKey> {
        self.keys
            .iter()
            .find(|k| &k.keys.public_key().to_compressed() == key)
    }

    fn role(&self, role: &str) -> Option<&DerivedKey> {
        self.keys.iter().find(|k| k.role == role)
    }

    /// The generator's public key.
    pub fn generator(&self) -> Option<PublicKeyBytes> {
        self.role("generator")
            .map(|k| k.keys.public_key().to_compressed())
    }

    fn wallets(&self) -> impl Iterator<Item = &DerivedKey> {
        self.keys.iter().filter(|k| k.role.starts_with("wallet/"))
    }

    fn delegates(&self) -> impl Iterator<Item = &DerivedKey> {
        self.keys.iter().filter(|k| k.role.starts_with("delegate/"))
    }
}

/// The seed a corpus's keys derive from: `corpus.json`'s `seed`, checked against the genesis
/// generator.
pub fn corpus_keys(corpus: &Corpus) -> Result<Keys> {
    let seed = corpus
        .meta
        .get("seed")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Corpus {
            path: corpus.dir.join("corpus.json"),
            message: "no seed: the synthetic cases need the chain's public seed".into(),
        })?;
    let keys = Keys::derive(seed, corpus.network.pub_key_hash)?;
    if keys.generator() != Some(corpus.genesis.header().generator_public_key) {
        return Err(Error::Corpus {
            path: corpus.dir.join("corpus.json"),
            message: "the seed does not derive the genesis block's generator".into(),
        });
    }
    Ok(keys)
}

/// What one side did with a block.
#[derive(Debug, Clone, PartialEq)]
struct Verdict {
    accepted: bool,
    class: Option<String>,
    missed: Vec<Value>,
    reward: Option<String>,
    line: Option<Value>,
    detail: String,
}

/// Both sides of a case, at the same state.
struct Sides<'a> {
    corpus: &'a Corpus,
    milestones: Milestones,
    state: State,
    session: Value,
    case: SyntheticCase,
    keys: &'a Keys,
    results: Vec<CaseResult>,
    step: usize,
    last_missed: Vec<MissedBlock>,
    oracle_missed: usize,
}

fn missed_json(rows: &[MissedBlock]) -> Vec<Value> {
    rows.iter()
        .map(|m| json!({"timestamp": m.timestamp, "height": m.height, "username": m.username}))
        .collect()
}

fn without_chain_digest(line: &Value) -> Value {
    let mut line = line.clone();
    if let Some(o) = line.as_object_mut() {
        o.shift_remove("cd");
    }
    line
}

impl<'a> Sides<'a> {
    fn open(
        corpus: &'a Corpus,
        keys: &'a Keys,
        server: &mut OracleServer,
        case: SyntheticCase,
        milestones_text: Option<String>,
    ) -> Result<Sides<'a>> {
        let milestones = match &milestones_text {
            Some(text) => Milestones::load(text, &corpus.network).map_err(|e| Error::Corpus {
                path: corpus.dir.join("crypto/milestones.json"),
                message: format!("the case's milestones: {e}"),
            })?,
            None => corpus.milestones.clone(),
        };
        let handlers = Handlers;
        let mut state = State::from_genesis(&corpus.genesis, &milestones, &handlers)
            .map_err(|e| Error::Oracle(format!("Heartwood refused the genesis block: {e}")))?;
        for block in &corpus.blocks {
            let decoded = Block::receive(&block.bytes, &milestones)
                .map_err(|e| Error::Oracle(format!("block {}: {e}", block.height)))?;
            state
                .apply_block(&decoded, &milestones, &handlers)
                .map_err(|e| {
                    Error::Oracle(format!("Heartwood refused block {}: {e}", block.height))
                })?;
        }
        let mut args = json!({"crypto": corpus_crypto(corpus)?});
        if let Some(text) = milestones_text {
            args["milestones"] = json!(text);
        }
        let opened = call(server, "chain.open", 1, &args)?;
        let session = opened.get("session").cloned().unwrap_or(Value::Null);
        for block in &corpus.blocks {
            let r = call(
                server,
                "chain.apply",
                1,
                &json!({"session": session, "hex": hex::encode(&block.bytes)}),
            )?;
            if r.get("accepted") != Some(&Value::Bool(true)) {
                return Err(Error::Oracle(format!(
                    "the oracle refused block {}: {r}",
                    block.height
                )));
            }
        }
        Ok(Sides {
            corpus,
            milestones,
            state,
            session,
            case,
            keys,
            results: Vec::new(),
            step: 0,
            last_missed: Vec::new(),
            oracle_missed: 0,
        })
    }

    fn record(&mut self, what: &str, outcome: Outcome) {
        self.step += 1;
        self.results.push(CaseResult {
            file: format!("{}/synthetic", self.corpus.name()),
            line: self.step,
            op: self.case.name().to_owned(),
            label: format!("{} {}: {what}", self.case.name(), self.step),
            height: u64::from(self.state.height()),
            outcome,
        });
    }

    fn params_height(&self) -> u32 {
        self.state.height().saturating_add(1)
    }

    fn nonce(&self, address: &Address) -> u64 {
        self.state
            .wallets()
            .find_by_address(address)
            .map_or(0, heartwood_state::Wallet::nonce)
    }

    fn balance(&self, address: &Address) -> u64 {
        self.state
            .wallets()
            .find_by_address(address)
            .map_or(0, heartwood_state::Wallet::balance)
    }

    /// A test wallet that `accept` accepts, by the seed's order.
    fn wallet(&self, accept: impl Fn(&Sides<'_>, &DerivedKey) -> bool) -> Result<&'a DerivedKey> {
        self.keys
            .wallets()
            .find(|k| accept(self, k))
            .ok_or_else(|| {
                Error::Oracle(format!(
                    "{}: no test wallet fits the case",
                    self.case.name()
                ))
            })
    }

    /// A delegate of the seed that `accept` accepts, by the seed's order.
    fn delegate(&self, accept: impl Fn(&Sides<'_>, &DerivedKey) -> bool) -> Result<&'a DerivedKey> {
        self.keys
            .delegates()
            .find(|k| accept(self, k))
            .ok_or_else(|| {
                Error::Oracle(format!(
                    "{}: no delegate of the seed fits the case",
                    self.case.name()
                ))
            })
    }

    fn funded(&self, k: &DerivedKey, at_least: u64) -> bool {
        let wallet = self.state.wallets().find_by_address(&k.address);
        wallet.is_some_and(|w| w.balance() >= at_least && w.public_key().is_some())
    }

    fn build(&self, builder: TransactionBuilder, key: &DerivedKey) -> Result<Transaction> {
        let params = self.milestones.at(self.params_height());
        builder
            .network(params.network())
            .build(&key.keys, Aux::fixed(AUX), params)
            .map_err(|e| {
                Error::Oracle(format!("{}: building a transaction: {e}", self.case.name()))
            })
    }

    /// The next block on the current state, with `transactions`, in the first slot at least
    /// `gap` slots after the next one whose delegate the seed derives, signed by `signer` (the
    /// scheduled delegate when `None`).
    fn next_block(
        &self,
        transactions: Vec<Transaction>,
        gap: u32,
        signer: Option<&DerivedKey>,
    ) -> Result<Vec<u8>> {
        let last = self.state.last_block().header().clone();
        let height = last.height + 1;
        let params = self.milestones.at(height);
        let block_time = u32::try_from(params.block_time()).unwrap_or(8);
        for extra in gap..gap + 200 {
            let timestamp = last.timestamp + block_time * (1 + extra);
            let info = calculate_forging_info(i64::from(timestamp), height, &self.milestones);
            let Some(scheduled) = usize::try_from(info.current_forger)
                .ok()
                .and_then(|i| self.state.forging_delegates().get(i))
            else {
                continue;
            };
            let Some(forger) = self.keys.by_public_key(&scheduled.public_key) else {
                continue;
            };
            let signer = signer.unwrap_or(forger);
            let reward = self
                .state
                .reward_for_block_in_round(height, &scheduled.public_key, &self.milestones)
                .map(|r| r.reward)
                .unwrap_or(0);
            let mut ids = Vec::new();
            let (mut total_amount, mut total_fee) = (0u64, 0u64);
            for t in &transactions {
                ids.extend_from_slice(t.id().as_bytes());
                total_amount = total_amount.saturating_add(t.amount());
                total_fee = total_fee.saturating_add(t.fee());
            }
            let count = u32::try_from(transactions.len()).unwrap_or(u32::MAX);
            // The block is assembled and signed here rather than by the block factory, which
            // refuses a transaction the rules of the block's height refuse: a case may need such
            // a block, and the receiving sides judge it.
            let mut header = BlockHeader {
                version: params.block_version(),
                timestamp,
                height,
                previous_block: *self.state.last_block().id().as_bytes(),
                number_of_transactions: count,
                total_amount,
                total_fee,
                reward,
                payload_length: count.saturating_mul(32),
                payload_hash: sha256(&ids),
                generator_public_key: signer.keys.public_key().to_compressed(),
                block_signature: Signature::from_array([0; 64]),
            };
            header.block_signature = sig::sign(
                SchemeId::Secp256k1Bip340,
                SigningDomain::Block,
                &serialiser::signing_hash(&header),
                signer.keys.secret_key(),
                Aux::fixed(AUX),
            )
            .map_err(|e| Error::Oracle(format!("{}: signing a block: {e}", self.case.name())))?;
            return Ok(serialiser::serialise_with_transactions(
                &header,
                &transactions,
            ));
        }
        Err(Error::Oracle(format!(
            "{}: no slot has a derived delegate",
            self.case.name()
        )))
    }

    /// Heartwood's verdict on block bytes; the state changes only when the block is accepted.
    fn heartwood_apply(&mut self, bytes: &[u8], step: Step) -> Verdict {
        let refused = |class: Option<String>, detail: String, missed: Vec<Value>| Verdict {
            accepted: false,
            class,
            missed,
            reward: None,
            line: None,
            detail,
        };
        let block = match Block::receive(bytes, &self.milestones) {
            Ok(block) => block,
            Err(e) => return refused(None, format!("decode: {e}"), Vec::new()),
        };
        let failures: Vec<String> = block
            .verify(&self.milestones, i64::from(u32::MAX))
            .failures
            .iter()
            .map(|f| format!("{f:?}"))
            .collect();
        if !failures.is_empty() {
            return refused(
                None,
                format!("verification: {}", failures.join(", ")),
                Vec::new(),
            );
        }
        let header = block.header();
        let missed = self.state.detect_missed_blocks(header, &self.milestones);
        let reward = self
            .state
            .reward_for_block_in_round(
                header.height,
                &header.generator_public_key,
                &self.milestones,
            )
            .ok()
            .map(|r| r.reward);
        let previous = self.state.clone();
        match self.state.apply_block(&block, &self.milestones, &Handlers) {
            Ok(applied) => {
                self.last_missed = applied.missed.clone();
                let line = digest::height_digest(
                    &self.state,
                    &self.milestones,
                    step,
                    &applied.missed,
                    reward,
                    &[0; 32],
                );
                Verdict {
                    accepted: true,
                    class: None,
                    missed: missed_json(&applied.missed),
                    reward: reward.map(|r| r.to_string()),
                    line: Some(digests::line_value(&line)),
                    detail: String::new(),
                }
            }
            Err(e) => {
                if self.state != previous {
                    self.state = previous;
                    self.record(
                        "a refused block left Heartwood's state changed",
                        Outcome::HarnessError(e.to_string()),
                    );
                }
                let class = match &e {
                    ApplyError::Transaction { rejected, .. } => Some(rejected.class().to_owned()),
                    _ => None,
                };
                refused(class, e.to_string(), missed_json(&missed))
            }
        }
    }

    /// Send block bytes to both sides and compare; `expect_accepted` is the case's premise.
    fn apply(
        &mut self,
        what: &str,
        bytes: &[u8],
        expect_accepted: bool,
        server: &mut OracleServer,
    ) -> Result<bool> {
        let before = self.dump(server)?;
        let theirs = call(
            server,
            "chain.apply",
            1,
            &json!({"session": self.session, "hex": hex::encode(bytes)}),
        )?;
        let ours = self.heartwood_apply(bytes, Step::Apply);
        let oracle_accepted = theirs.get("accepted") == Some(&Value::Bool(true));
        if oracle_accepted != ours.accepted {
            self.record(
                what,
                Outcome::VerdictMismatch {
                    oracle: if oracle_accepted {
                        "accepted".into()
                    } else {
                        format!("refused ({})", theirs["error"])
                    },
                    heartwood: if ours.accepted {
                        "accepted".into()
                    } else {
                        format!("refused ({})", ours.detail)
                    },
                },
            );
            return Ok(false);
        }
        if oracle_accepted != expect_accepted {
            self.record(
                what,
                Outcome::HarnessError(format!(
                    "both sides {} the block, the case expects the opposite",
                    if oracle_accepted {
                        "accepted"
                    } else {
                        "refused"
                    }
                )),
            );
        }
        let their_missed = theirs.get("missed").cloned().unwrap_or(Value::Null);
        self.oracle_missed = their_missed.as_array().map_or(0, Vec::len);
        let missed_differences = compare::diff(&their_missed, &Value::Array(ours.missed.clone()));
        if !missed_differences.is_empty() {
            self.record(
                &format!("{what}: missed slots"),
                Outcome::Mismatch(missed_differences),
            );
        }
        if oracle_accepted {
            let their_line = theirs
                .get("line")
                .map(without_chain_digest)
                .unwrap_or(Value::Null);
            let our_line = ours
                .line
                .as_ref()
                .map(without_chain_digest)
                .unwrap_or(Value::Null);
            let differences = compare::diff(&their_line, &our_line);
            self.record(
                what,
                if differences.is_empty() {
                    Outcome::Pass
                } else {
                    Outcome::Mismatch(differences)
                },
            );
        } else {
            let their_class = theirs
                .pointer("/error/class")
                .and_then(Value::as_str)
                .unwrap_or("");
            match &ours.class {
                Some(class) if class != their_class => self.record(
                    what,
                    Outcome::ClassMismatch {
                        oracle: their_class.to_owned(),
                        heartwood: class.clone(),
                    },
                ),
                _ => self.record(what, Outcome::Pass),
            }
            let after = self.dump(server)?;
            if after != before {
                self.record(
                    &format!("{what}: the oracle's state changed"),
                    Outcome::HarnessError("a refused block changed the oracle's records".into()),
                );
            }
            self.compare_records(&format!("{what}: state after the refusal"), server)?;
        }
        Ok(oracle_accepted)
    }

    fn dump(&self, server: &mut OracleServer) -> Result<Value> {
        let mut sections = call(server, "chain.dump", 1, &json!({"session": self.session}))?
            .get("sections")
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(o) = sections.as_object_mut() {
            o.shift_remove("M");
        }
        Ok(sections)
    }

    /// The oracle's records against Heartwood's, the missed slots aside.
    fn compare_records(&mut self, what: &str, server: &mut OracleServer) -> Result<()> {
        let theirs = self.dump(server)?;
        let ours = digest::records(&self.state, &self.milestones, &self.last_missed);
        let mut ours_json = digests::sections_to_json(&ours);
        if let Some(o) = ours_json.as_object_mut() {
            o.shift_remove("M");
        }
        let differences = compare::diff(&theirs, &ours_json);
        self.record(
            what,
            if differences.is_empty() {
                Outcome::Pass
            } else {
                Outcome::Mismatch(differences)
            },
        );
        Ok(())
    }

    /// Revert `count` blocks on both sides and compare every revert line.
    fn revert(&mut self, what: &str, count: u32, server: &mut OracleServer) -> Result<()> {
        let theirs = call(
            server,
            "chain.revert",
            1,
            &json!({"session": self.session, "count": count}),
        )?;
        let their_lines = theirs
            .get("lines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for i in 0..count as usize {
            let ours = match self.state.revert_block(&self.milestones, &Handlers) {
                Ok(_) => digest::height_digest(
                    &self.state,
                    &self.milestones,
                    Step::Revert,
                    &[],
                    None,
                    &[0; 32],
                ),
                Err(e) => {
                    self.record(
                        what,
                        Outcome::HarnessError(format!("Heartwood refused the revert: {e}")),
                    );
                    return Ok(());
                }
            };
            self.last_missed.clear();
            let their_line = their_lines
                .get(i)
                .map(without_chain_digest)
                .unwrap_or(Value::Null);
            let differences = compare::diff(
                &their_line,
                &without_chain_digest(&digests::line_value(&ours)),
            );
            self.record(
                &format!("{what} ({} of {count})", i + 1),
                if differences.is_empty() {
                    Outcome::Pass
                } else {
                    Outcome::Mismatch(differences)
                },
            );
        }
        Ok(())
    }

    /// Record a failed premise: the case did not set up what it tests.
    fn premise(&mut self, holds: bool, what: &str) {
        if !holds {
            self.record(
                what,
                Outcome::HarnessError(format!("the case's premise does not hold: {what}")),
            );
        }
    }

    /// The rank of the delegate `username` in Heartwood's state.
    fn rank(&self, username: &str) -> Option<u32> {
        self.state
            .wallets()
            .find_by_username(username)
            .and_then(|w| w.delegate())
            .and_then(|d| d.rank)
    }

    /// Revert the corpus's last blocks until the state ends a round minus `before_end` blocks.
    fn back_to_round_end(&mut self, before_end: u32, server: &mut OracleServer) -> Result<()> {
        let mut steps = 0;
        while !is_new_round(self.state.height() + 1 + before_end, &self.milestones) {
            steps += 1;
            if steps > 200 {
                return Err(Error::Oracle(
                    "no round end within 200 blocks of the tip".into(),
                ));
            }
            self.revert("revert towards a round end", 1, server)?;
        }
        Ok(())
    }
}

fn transfer(items: &[(Address, u64)]) -> TransactionBuilder {
    TransactionBuilder::transfer(
        items
            .iter()
            .map(|(recipient, amount)| TransferItem {
                amount: *amount,
                recipient: *recipient,
            })
            .collect(),
    )
}

fn run_case(
    case: SyntheticCase,
    corpus: &Corpus,
    keys: &Keys,
    server: &mut OracleServer,
) -> Result<Vec<CaseResult>> {
    let extra = match case {
        SyntheticCase::RuleHeight => {
            Some(json!({"height": corpus.tip() + 2, "transfer": {"maximum": 2}}))
        }
        SyntheticCase::HandlerRuleHeight => Some(Value::Null),
        _ => None,
    };
    let milestones_text = match extra {
        Some(extra) => {
            let path = corpus.dir.join("crypto/milestones.json");
            let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
            let mut list: Value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
                path: path.clone(),
                message: e.to_string(),
            })?;
            let not_a_list = || Error::Corpus {
                path: path.clone(),
                message: "not a milestone list".into(),
            };
            let list = list.as_array_mut().ok_or_else(not_a_list)?;
            let extra = match extra {
                Value::Null => {
                    handler_rules_at(list.first().ok_or_else(not_a_list)?, corpus.tip() + 3)
                }
                extra => extra,
            };
            list.push(extra);
            Some(Value::Array(list.clone()).to_string())
        }
        None => None,
    };
    let mut s = Sides::open(corpus, keys, server, case, milestones_text.clone())?;
    let recipients: Vec<Address> = keys.wallets().map(|k| k.address).collect();
    match case {
        SyntheticCase::OverspendAfterGap => {
            let a = s.wallet(|s, k| s.funded(k, 1_000_000_000))?;
            let b = s.wallet(|s, k| s.funded(k, 1) && k.address != a.address)?;
            let t1 = s.build(
                transfer(&[(b.address, 1_000)])
                    .nonce(s.nonce(&a.address) + 1)
                    .fee(FEE),
                a,
            )?;
            let over = s.balance(&b.address);
            let t2 = s.build(
                transfer(&[(a.address, over)])
                    .nonce(s.nonce(&b.address) + 1)
                    .fee(FEE),
                b,
            )?;
            let bytes = s.next_block(vec![t1, t2], 3, None)?;
            s.apply(
                "a block whose second transaction overspends, after missed slots",
                &bytes,
                false,
                server,
            )?;
        }
        SyntheticCase::RuleHeight => {
            let a = s.wallet(|s, k| s.funded(k, 1_000_000_000))?;
            let items: Vec<(Address, u64)> =
                recipients.iter().take(3).map(|r| (*r, 1_000)).collect();
            let t1 = s.build(transfer(&items).nonce(s.nonce(&a.address) + 1).fee(FEE), a)?;
            let bytes = s.next_block(vec![t1], 0, None)?;
            s.apply(
                "three transfer items the height before the limit",
                &bytes,
                true,
                server,
            )?;
            let t2 = s.build(transfer(&items).nonce(s.nonce(&a.address) + 1).fee(FEE), a);
            // At the new height the builder itself refuses three items: build under the rules
            // before it, as a node that has not seen the milestone would.
            let t2 = match t2 {
                Ok(t) => t,
                Err(_) => {
                    let params = s.milestones.at(s.state.height());
                    transfer(&items)
                        .nonce(s.nonce(&a.address) + 1)
                        .fee(FEE)
                        .network(params.network())
                        .build(&a.keys, Aux::fixed(AUX), params)
                        .map_err(|e| Error::Oracle(format!("building the transfer: {e}")))?
                }
            };
            let bytes = s.next_block(vec![t2], 0, None)?;
            s.apply(
                "three transfer items at the height of the limit",
                &bytes,
                false,
                server,
            )?;
        }
        SyntheticCase::VoteAtRoundEnd => {
            s.back_to_round_end(1, server)?;
            let voter = s.wallet(|s, k| s.funded(k, 10_000_000_000))?;
            let target = s
                .state
                .wallets()
                .delegates()
                .filter_map(|w| w.delegate())
                .filter(|d| d.resigned.is_none())
                .max_by_key(|d| d.rank.unwrap_or(u32::MAX))
                .map(|d| d.username().to_owned())
                .ok_or_else(|| Error::Oracle("no delegate to vote for".into()))?;
            let mut votes = Votes::new();
            votes.insert(target, 10_000);
            let t = s.build(
                TransactionBuilder::vote(votes)
                    .nonce(s.nonce(&voter.address) + 1)
                    .fee(FEE),
                voter,
            )?;
            let bytes = s.next_block(vec![t], 0, None)?;
            s.apply("a vote in the last block of a round", &bytes, true, server)?;
            s.compare_records("the next round's ranks and schedule", server)?;
            s.revert("the revert of the round's last block", 1, server)?;
        }
        SyntheticCase::BurnMinimum => {
            let a = s.wallet(|s, k| s.funded(k, 1_000_000_000))?;
            let minimum = s.milestones.at(s.params_height()).burn_tx_amount();
            let low = s.build(
                TransactionBuilder::burn(minimum - 1)
                    .nonce(s.nonce(&a.address) + 1)
                    .fee(FEE),
                a,
            )?;
            let bytes = s.next_block(vec![low], 0, None)?;
            s.apply("a burn one below the minimum", &bytes, false, server)?;
            let exact = s.build(
                TransactionBuilder::burn(minimum)
                    .nonce(s.nonce(&a.address) + 1)
                    .fee(FEE),
                a,
            )?;
            let bytes = s.next_block(vec![exact], 0, None)?;
            s.apply("a burn at the minimum", &bytes, true, server)?;
        }
        SyntheticCase::RegistrationAcrossRoundEnd => {
            s.back_to_round_end(3, server)?;
            let w = s.wallet(|s, k| {
                s.funded(k, REGISTRATION_FEE * 2)
                    && s.state
                        .wallets()
                        .find_by_address(&k.address)
                        .is_some_and(|w| !w.is_delegate())
            })?;
            let t = s.build(
                TransactionBuilder::delegate_registration("synthreg")
                    .nonce(s.nonce(&w.address) + 1)
                    .fee(REGISTRATION_FEE),
                w,
            )?;
            let bytes = s.next_block(vec![t], 0, None)?;
            s.apply(
                "a delegate registration in the round's last blocks",
                &bytes,
                true,
                server,
            )?;
            for n in 0..2 {
                let bytes = s.next_block(Vec::new(), 0, None)?;
                s.apply(
                    &format!("an empty block {} to the round's end", 2 - n),
                    &bytes,
                    true,
                    server,
                )?;
            }
            s.compare_records("the round started with the new delegate", server)?;
            let ranked = s.rank("synthreg").is_some();
            s.premise(ranked, "the new delegate is ranked at the round start");
            s.revert("the revert across the round's end", 1, server)?;
            s.compare_records("the ranks of the round start again", server)?;
            let unranked = s.rank("synthreg").is_none();
            s.premise(unranked, "the revert takes the new delegate's rank back");
        }
        SyntheticCase::NonDelegateGenerator => {
            let stranger = s.wallet(|s, k| {
                s.state
                    .wallets()
                    .find_by_address(&k.address)
                    .is_none_or(|w| !w.is_delegate())
            })?;
            let bytes = s.next_block(Vec::new(), 0, Some(stranger))?;
            s.apply(
                "a block by a key that is no delegate",
                &bytes,
                false,
                server,
            )?;
        }
        SyntheticCase::MissingSecondSignature => {
            let w = s.wallet(|s, k| {
                s.funded(k, 1_000_000_000)
                    && s.state
                        .wallets()
                        .find_by_address(&k.address)
                        .is_some_and(|w| w.second_public_key().is_none())
            })?;
            let second =
                heartwood_crypto::identities::KeyPair::from_passphrase("synthetic second secret")
                    .map_err(|e| Error::Oracle(e.to_string()))?;
            let t = s.build(
                TransactionBuilder::second_signature(second.public_key().to_compressed())
                    .nonce(s.nonce(&w.address) + 1)
                    .fee(FEE * 30),
                w,
            )?;
            let bytes = s.next_block(vec![t], 0, None)?;
            s.apply("a second-signature registration", &bytes, true, server)?;
            let t = s.build(
                transfer(&[(recipients[0], 1)])
                    .nonce(s.nonce(&w.address) + 1)
                    .fee(FEE),
                w,
            )?;
            let bytes = s.next_block(vec![t], 0, None)?;
            s.apply(
                "a transfer without the second signature",
                &bytes,
                false,
                server,
            )?;
        }
        SyntheticCase::FeeFloorInBlock => {
            let a = s.wallet(|s, k| s.funded(k, 1_000_000_000))?;
            let nonce = s.nonce(&a.address) + 1;
            let probe = s.build(transfer(&[(recipients[1], 1)]).nonce(nonce).fee(1), a)?;
            let floor = call(
                server,
                "fee.minimum",
                u64::from(s.params_height()),
                &json!({"hex": hex::encode(probe.bytes())}),
            )?
            .get("minimumFee")
            .and_then(Value::as_str)
            .and_then(|m| m.parse::<u64>().ok())
            .ok_or_else(|| Error::Oracle("fee.minimum sent no floor".into()))?;
            let low = s.build(
                transfer(&[(recipients[1], 1)]).nonce(nonce).fee(floor - 1),
                a,
            )?;
            let bytes = s.next_block(vec![low], 0, None)?;
            s.apply("a transfer one below the fee floor", &bytes, false, server)?;
            let exact = s.build(transfer(&[(recipients[1], 1)]).nonce(nonce).fee(floor), a)?;
            let bytes = s.next_block(vec![exact], 0, None)?;
            s.apply("a transfer at the fee floor", &bytes, true, server)?;
        }
        SyntheticCase::HandlerRuleHeight => {
            handler_rule_height(&mut s, milestones_text.as_deref(), server)?;
        }
        SyntheticCase::ForkAcrossRoundEnd => {
            fork_across_round_end(&mut s, &recipients, server)?;
        }
    }
    call(server, "chain.close", 1, &json!({"session": s.session}))?;
    Ok(s.results)
}

/// The milestone that [`SyntheticCase::HandlerRuleHeight`] adds at `height`, from the chain's
/// first milestone `first`: twice the fee floor's `minFee`, a burn minimum 1,000,000 higher, and
/// a revoke delay of 1 block.
fn handler_rules_at(first: &Value, height: u32) -> Value {
    let mut fees = first["dynamicFees"].clone();
    let min_fee = fees["minFee"].as_i64().unwrap_or(1).max(1);
    fees["minFee"] = json!(min_fee.saturating_mul(2));
    let mut burn = first["burn"].clone();
    let minimum = burn["txAmount"].as_u64().unwrap_or(0);
    burn["txAmount"] = json!(minimum.saturating_add(1_000_000));
    json!({
        "height": height,
        "dynamicFees": fees,
        "burn": burn,
        "blocksToRevokeDelegateResignation": 1,
    })
}

/// [`SyntheticCase::HandlerRuleHeight`]: with the rules of [`handler_rules_at`] from the tip plus
/// 3, a burn at the old minimum and a transfer at the old fee floor are accepted at the tip plus 2
/// and refused at the tip plus 3, and a revoke one block after its resignation is refused at the
/// tip plus 2 and accepted at the tip plus 3. Each accepted block is reverted, so the next try
/// runs at the same height.
fn handler_rule_height(
    s: &mut Sides<'_>,
    milestones_text: Option<&str>,
    server: &mut OracleServer,
) -> Result<()> {
    // The tip plus 1: a new delegate, so that a delegate of the seed may resign temporarily.
    let w = s.wallet(|s, k| {
        s.funded(k, REGISTRATION_FEE * 2)
            && s.state
                .wallets()
                .find_by_address(&k.address)
                .is_some_and(|w| !w.is_delegate())
    })?;
    let d = s.delegate(|s, k| {
        s.funded(k, FEE * 4)
            && s.state
                .wallets()
                .find_by_address(&k.address)
                .and_then(|w| w.delegate())
                .is_some_and(|d| d.resigned.is_none())
    })?;
    let a = s.wallet(|s, k| s.funded(k, 1_000_000_000) && k.address != w.address)?;
    let register = s.build(
        TransactionBuilder::delegate_registration("synthrules")
            .nonce(s.nonce(&w.address) + 1)
            .fee(REGISTRATION_FEE),
        w,
    )?;
    let resign = s.build(
        TransactionBuilder::delegate_resignation(None)
            .nonce(s.nonce(&d.address) + 1)
            .fee(FEE),
        d,
    )?;
    let bytes = s.next_block(vec![register, resign], 0, None)?;
    s.apply(
        "a delegate registration and a temporary resignation",
        &bytes,
        true,
        server,
    )?;

    // The same three transactions at both heights.
    let revoke = s.build(
        TransactionBuilder::delegate_resignation(Some(ResignationType::NotResigned))
            .nonce(s.nonce(&d.address) + 1)
            .fee(FEE),
        d,
    )?;
    let minimum = s.milestones.at(s.params_height()).burn_tx_amount();
    let burn = s.build(
        TransactionBuilder::burn(minimum)
            .nonce(s.nonce(&a.address) + 1)
            .fee(FEE),
        a,
    )?;
    let nonce = s.nonce(&a.address) + 1;
    let probe = s.build(transfer(&[(w.address, 1)]).nonce(nonce).fee(1), a)?;
    let mut args = json!({"hex": hex::encode(probe.bytes())});
    if let Some(text) = milestones_text {
        args["milestones"] = json!(text);
    }
    let floor = call(server, "fee.minimum", u64::from(s.params_height()), &args)?
        .get("minimumFee")
        .and_then(Value::as_str)
        .and_then(|m| m.parse::<u64>().ok())
        .ok_or_else(|| Error::Oracle("fee.minimum sent no floor".into()))?;
    let at_floor = s.build(transfer(&[(w.address, 1)]).nonce(nonce).fee(floor), a)?;

    let tries: [(&str, &Transaction, bool, bool); 3] = [
        ("a burn at the old minimum", &burn, true, false),
        ("a transfer at the old fee floor", &at_floor, true, false),
        (
            "a revoke one block after its resignation",
            &revoke,
            false,
            true,
        ),
    ];
    for (height, old_rules) in [
        ("the height before the new rules", true),
        ("the height of the new rules", false),
    ] {
        for (what, transaction, before, after) in &tries {
            let accepted = if old_rules { *before } else { *after };
            let bytes = s.next_block(vec![(*transaction).clone()], 0, None)?;
            let applied = s.apply(&format!("{what}, at {height}"), &bytes, accepted, server)?;
            if applied {
                s.revert(&format!("{what}, taken back"), 1, server)?;
            }
        }
        if old_rules {
            let bytes = s.next_block(Vec::new(), 0, None)?;
            s.apply("an empty block before the new rules", &bytes, true, server)?;
        }
    }
    Ok(())
}

/// [`SyntheticCase::ForkAcrossRoundEnd`]: from a round's last block, two blocks of the next
/// round, then a revert of three blocks (the round's last block included) and a competing chain
/// of three blocks in other slots, the first with a vote that changes the next round's ranks.
fn fork_across_round_end(
    s: &mut Sides<'_>,
    recipients: &[Address],
    server: &mut OracleServer,
) -> Result<()> {
    s.back_to_round_end(0, server)?;
    let end = s.state.last_block().clone();
    // The first branch: two blocks after the round end.
    let a = s.wallet(|s, k| s.funded(k, 1_000_000_000))?;
    let t = s.build(
        transfer(&[(recipients[2], 1_000)])
            .nonce(s.nonce(&a.address) + 1)
            .fee(FEE),
        a,
    )?;
    let bytes = s.next_block(vec![t], 0, None)?;
    s.apply(
        "the first branch, one block into the round",
        &bytes,
        true,
        server,
    )?;
    let bytes = s.next_block(Vec::new(), 0, None)?;
    s.apply(
        "the first branch, two blocks into the round",
        &bytes,
        true,
        server,
    )?;
    let first_order: Vec<PublicKeyBytes> = s
        .state
        .forging_delegates()
        .iter()
        .map(|d| d.public_key)
        .collect();

    // Back across the round end.
    s.revert("the revert across the round end", 3, server)?;

    // The competing branch: the round's last block again, in a later slot, with a vote for the
    // lowest ranked delegate, then two more blocks with other gaps.
    let voter = s.wallet(|s, k| s.funded(k, 10_000_000_000) && k.address != a.address)?;
    let target = s
        .state
        .wallets()
        .delegates()
        .filter_map(|w| w.delegate())
        .filter(|d| d.resigned.is_none())
        .max_by_key(|d| d.rank.unwrap_or(u32::MAX))
        .map(|d| d.username().to_owned())
        .ok_or_else(|| Error::Oracle("no delegate to vote for".into()))?;
    let mut votes = Votes::new();
    votes.insert(target, 10_000);
    let vote = s.build(
        TransactionBuilder::vote(votes)
            .nonce(s.nonce(&voter.address) + 1)
            .fee(FEE),
        voter,
    )?;
    let bytes = s.next_block(vec![vote], 1, None)?;
    s.apply(
        "the competing round end, a slot later, with a vote",
        &bytes,
        true,
        server,
    )?;
    s.premise(
        s.state.last_block().id() != end.id()
            && s.state.last_block().header().timestamp != end.header().timestamp,
        "the competing block is another block in another slot",
    );
    let t = s.build(
        transfer(&[(recipients[3], 2_000)])
            .nonce(s.nonce(&a.address) + 1)
            .fee(FEE),
        a,
    )?;
    let bytes = s.next_block(vec![t], 2, None)?;
    s.apply(
        "the competing branch, one block into the round",
        &bytes,
        true,
        server,
    )?;
    let bytes = s.next_block(Vec::new(), 0, None)?;
    s.apply(
        "the competing branch, two blocks into the round",
        &bytes,
        true,
        server,
    )?;
    let order: Vec<PublicKeyBytes> = s
        .state
        .forging_delegates()
        .iter()
        .map(|d| d.public_key)
        .collect();
    s.premise(
        order != first_order,
        "the competing branch schedules the round differently",
    );
    s.compare_records("the state at the competing tip", server)?;
    Ok(())
}

/// Run every synthetic case on top of `corpus` against the oracle `server`.
pub fn run(corpus: &Corpus, server: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let keys = corpus_keys(corpus)?;
    let mut out = Vec::new();
    for case in SyntheticCase::ALL {
        match run_case(case, corpus, &keys, server) {
            Ok(results) => out.extend(results),
            Err(e) => out.push(CaseResult {
                file: format!("{}/synthetic", corpus.name()),
                line: 0,
                op: case.name().to_owned(),
                label: case.name().to_owned(),
                height: u64::from(corpus.tip()),
                outcome: Outcome::HarnessError(e.to_string()),
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_seed_derives_the_devnet_roles() {
        let keys = Keys::derive(DEVNET_SEED, 90).unwrap();
        assert_eq!(keys.keys.len(), 4 + 53 + 20);
        assert!(keys.role("delegate/53").is_some());
        assert_eq!(keys.wallets().count(), 20);
        let names: std::collections::BTreeSet<&str> =
            SyntheticCase::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), SyntheticCase::ALL.len());
    }
}
