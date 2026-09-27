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
//!
//! The restart cases (`restart-revoke` to `restart-round-end-revert-early`) run Heartwood's side
//! through its store: every applied or reverted block is committed, and a restart stops both
//! nodes and starts them again, the reference's by its saved state and then without it
//! (`chain.restart`, each from the same store), Heartwood's from its state tables and then by a
//! replay of the stored chain. Each of the four restart lines must be the line of the state
//! before the restart; the reference's lines may differ only as the oracle notes it: a delegate
//! name that reads as a number (the saved-state path), or no round row of the tip's round after a
//! revert to the last block of the round before (both paths). Where the store is still a stub,
//! Heartwood's restarts are skipped and the reference's are compared with Heartwood's state.

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
use heartwood_state::wallets::{Wallet, WalletRepository};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use heartwood_blockchain::{ChainStore, RestoreMode, rows};
use heartwood_kernel::config::crypto::{self as crypto_files, CryptoFiles, CryptoTexts};
use heartwood_state::machine::{Applied, Reverted};

use crate::compare;
use crate::corpus::Corpus;
use crate::digests;
use crate::errors::{Error, Result};
use crate::levels::l2::{call, corpus_crypto};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};
use crate::restart::HeartwoodMode;
use crate::store::{self, ScratchDir, StartProblem, StoreNode, guarded};
use crate::subject::Documented;

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
    /// A revoke in the first block after a restart.
    RestartRevoke,
    /// A resignation, a restart, then revokes one block before the delay, at it and after it.
    RestartRevokeDelay,
    /// A resignation, a revoke and a registration in one round, a restart in that round,
    /// then the round's end and the next round's start.
    RestartRoundRanks,
    /// Restarts at a round end and at the first block of the next round, among delegates
    /// with equal vote balances.
    RestartEqualBalances,
    /// Delegates named `0x10`, `1e5` and `0b101`, registered and voted for, then restarts
    /// (a name of digits only, such as `123`, is refused by the username rule on both sides).
    RestartNumericNames,
    /// A stored transfer raised above its sender's balance: a start without a saved state
    /// refuses the chain.
    RestartNegativeBalance,
    /// A restart, then a revert across a round end and a competing chain.
    RestartFork,
    /// A block after a round end, a revert to the round end and the block again, then a restart
    /// in the new round, whose rows the revert deleted, with missed-block rows in the store.
    RestartRoundEndRevert,
    /// A revert to the last round end below the first missed-block row, the next round's first
    /// blocks again, then a restart in that round, whose rows the revert deleted, with no
    /// missed-block row in the store.
    RestartRoundEndRevertEarly,
}

impl SyntheticCase {
    /// Every case.
    pub const ALL: [SyntheticCase; 19] = [
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
        SyntheticCase::RestartRevoke,
        SyntheticCase::RestartRevokeDelay,
        SyntheticCase::RestartRoundRanks,
        SyntheticCase::RestartEqualBalances,
        SyntheticCase::RestartNumericNames,
        SyntheticCase::RestartNegativeBalance,
        SyntheticCase::RestartFork,
        SyntheticCase::RestartRoundEndRevert,
        SyntheticCase::RestartRoundEndRevertEarly,
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
            SyntheticCase::RestartRevoke => "restart-revoke",
            SyntheticCase::RestartRevokeDelay => "restart-revoke-delay",
            SyntheticCase::RestartRoundRanks => "restart-round-ranks",
            SyntheticCase::RestartEqualBalances => "restart-equal-balances",
            SyntheticCase::RestartNumericNames => "restart-numeric-names",
            SyntheticCase::RestartNegativeBalance => "restart-negative-balance",
            SyntheticCase::RestartFork => "restart-fork",
            SyntheticCase::RestartRoundEndRevert => "restart-round-end-revert",
            SyntheticCase::RestartRoundEndRevertEarly => "restart-round-end-revert-early",
        }
    }

    /// Whether the case restarts, and so runs Heartwood's side through its store.
    pub const fn restarts(self) -> bool {
        matches!(
            self,
            SyntheticCase::RestartRevoke
                | SyntheticCase::RestartRevokeDelay
                | SyntheticCase::RestartRoundRanks
                | SyntheticCase::RestartEqualBalances
                | SyntheticCase::RestartNumericNames
                | SyntheticCase::RestartNegativeBalance
                | SyntheticCase::RestartFork
                | SyntheticCase::RestartRoundEndRevert
                | SyntheticCase::RestartRoundEndRevertEarly
        )
    }
}

/// The revoke delay of the restart cases' milestone, from the corpus's tip plus one.
const RESTART_REVOKE_DELAY: u32 = 3;

/// Heartwood's store in a case, with what it needs to start again.
struct StoreSide {
    scratch: ScratchDir,
    files: CryptoFiles,
    store: ChainStore,
}

/// The crypto files of `corpus` with the milestone file `milestones_text` when given, loaded by
/// the kernel's loader.
fn case_files(
    corpus: &Corpus,
    milestones_text: Option<&str>,
) -> std::result::Result<CryptoFiles, StartProblem> {
    let read = |name: &str| std::fs::read_to_string(corpus.dir.join("crypto").join(name)).ok();
    let texts = CryptoTexts {
        network: read("network.json"),
        milestones: milestones_text
            .map(str::to_owned)
            .or_else(|| read("milestones.json")),
        genesis: read("genesisBlock.json"),
        exceptions: read("exceptions.json"),
    };
    guarded(|| crypto_files::parse(&texts))?
        .map_err(|e| StartProblem::Failed(format!("the crypto files are refused: {e}")))
}

/// Heartwood's state after `corpus` written through a new store: the first start, then every
/// block committed, one per chunk.
fn store_side(
    corpus: &Corpus,
    milestones: &Milestones,
    milestones_text: Option<&str>,
) -> std::result::Result<(StoreSide, State), StartProblem> {
    let files = case_files(corpus, milestones_text)?;
    let scratch =
        ScratchDir::new("synthetic-store").map_err(|e| StartProblem::Failed(e.to_string()))?;
    let node = StoreNode::start(scratch.path(), &files, RestoreMode::Auto)?;
    let (mut store, mut state, _) = node.into_parts();
    for block in &corpus.blocks {
        let decoded = Block::receive(&block.bytes, milestones)
            .map_err(|e| StartProblem::Failed(format!("block {}: {e}", block.height)))?;
        let applied = state
            .apply_block(&decoded, milestones, &Handlers)
            .map_err(|e| {
                StartProblem::Failed(format!("Heartwood refused block {}: {e}", block.height))
            })?;
        commit(&mut store, |w| {
            w.applied(&state, &decoded, &applied, milestones)
        })?;
    }
    Ok((
        StoreSide {
            scratch,
            files,
            store,
        },
        state,
    ))
}

/// Commit one chunk that `write` fills.
fn commit(
    store: &mut ChainStore,
    write: impl FnOnce(
        &mut heartwood_blockchain::ChunkWriter<'_>,
    ) -> std::result::Result<(), heartwood_blockchain::StoreError>,
) -> std::result::Result<(), StartProblem> {
    let done = guarded(
        || -> std::result::Result<(), heartwood_blockchain::StoreError> {
            let mut writer = store.chunk()?;
            write(&mut writer)?;
            writer.commit()
        },
    )?;
    done.map_err(|e| StartProblem::Failed(format!("the store refused a chunk: {e}")))
}

/// The key-derivation tag of the devnet tooling.
pub const DERIVATION_TAG: &str = "solar-devnet-v1/";
/// The devnet tooling's default key-derivation seed, which is public.
pub const DEVNET_SEED: &str = "solar-devnet";
/// The aux of every synthetic signature.
const AUX: [u8; 32] = [0x5a; 32];
/// A fee above every floor of the devnet for the small transactions of the cases.
pub(crate) const FEE: u64 = 20_000_000;
/// The fee of a delegate registration (the devnet's surcharge is about 75 coins).
pub(crate) const REGISTRATION_FEE: u64 = 7_600_000_000;

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

    pub(crate) fn wallets(&self) -> impl Iterator<Item = &DerivedKey> {
        self.keys.iter().filter(|k| k.role.starts_with("wallet/"))
    }

    pub(crate) fn delegates(&self) -> impl Iterator<Item = &DerivedKey> {
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
pub(crate) struct Sides<'a> {
    pub(crate) corpus: &'a Corpus,
    pub(crate) milestones: Milestones,
    pub(crate) state: State,
    pub(crate) session: Value,
    name: &'static str,
    keys: &'a Keys,
    pub(crate) results: Vec<CaseResult>,
    step: usize,
    last_missed: Vec<MissedBlock>,
    oracle_missed: usize,
    /// Heartwood's store, for the restart cases.
    store: Option<StoreSide>,
    /// Why a restart case has no store (a stub): its restarts are skipped on Heartwood's side.
    store_problem: Option<StartProblem>,
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
    /// Both sides at the corpus's tip, the case `name` in reports: Heartwood's through a new
    /// store when `with_store`, the oracle's session with the reference's pool when `pool` gives
    /// its options.
    pub(crate) fn open(
        corpus: &'a Corpus,
        keys: &'a Keys,
        server: &mut OracleServer,
        name: &'static str,
        with_store: bool,
        milestones_text: Option<String>,
        pool: Option<Value>,
    ) -> Result<Sides<'a>> {
        let milestones = match &milestones_text {
            Some(text) => Milestones::load(text, &corpus.network).map_err(|e| Error::Corpus {
                path: corpus.dir.join("crypto/milestones.json"),
                message: format!("the case's milestones: {e}"),
            })?,
            None => corpus.milestones.clone(),
        };
        let handlers = Handlers;
        let (mut store, mut store_problem) = (None, None);
        let mut built = None;
        if with_store {
            match store_side(corpus, &milestones, milestones_text.as_deref()) {
                Ok((side, state)) => {
                    store = Some(side);
                    built = Some(state);
                }
                Err(problem) => store_problem = Some(problem),
            }
        }
        let state = match built {
            Some(state) => state,
            None => {
                let mut state = State::from_genesis(&corpus.genesis, &milestones, &handlers)
                    .map_err(|e| {
                        Error::Oracle(format!("Heartwood refused the genesis block: {e}"))
                    })?;
                for block in &corpus.blocks {
                    let decoded = Block::receive(&block.bytes, &milestones)
                        .map_err(|e| Error::Oracle(format!("block {}: {e}", block.height)))?;
                    state
                        .apply_block(&decoded, &milestones, &handlers)
                        .map_err(|e| {
                            Error::Oracle(format!("Heartwood refused block {}: {e}", block.height))
                        })?;
                }
                state
            }
        };
        let mut args = json!({"crypto": corpus_crypto(corpus)?});
        if let Some(text) = milestones_text {
            args["milestones"] = json!(text);
        }
        if let Some(options) = pool {
            args["pool"] = json!({ "options": options });
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
        let mut sides = Sides {
            corpus,
            milestones,
            state,
            session,
            name,
            keys,
            results: Vec::new(),
            step: 0,
            last_missed: Vec::new(),
            oracle_missed: 0,
            store,
            store_problem: None,
        };
        if let Some(problem) = store_problem {
            let outcome = problem.outcome();
            sides.record("Heartwood's store for the restarts", outcome);
            sides.store_problem = Some(problem);
        }
        Ok(sides)
    }

    /// Commit what applying `block` changed, when the case has a store.
    fn commit_applied(&mut self, block: &Block, applied: &Applied) {
        let Some(side) = &mut self.store else {
            return;
        };
        let (state, milestones) = (&self.state, &self.milestones);
        if let Err(problem) = commit(&mut side.store, |w| {
            w.applied(state, block, applied, milestones)
        }) {
            let outcome = problem.outcome();
            self.record("the commit of an applied block", outcome);
        }
    }

    /// Commit what reverting a block changed, when the case has a store.
    fn commit_reverted(&mut self, reverted: &Reverted) {
        let Some(side) = &mut self.store else {
            return;
        };
        let (state, milestones) = (&self.state, &self.milestones);
        if let Err(problem) = commit(&mut side.store, |w| w.reverted(state, reverted, milestones)) {
            let outcome = problem.outcome();
            self.record("the commit of a reverted block", outcome);
        }
    }

    pub(crate) fn record(&mut self, what: &str, outcome: Outcome) {
        self.step += 1;
        self.results.push(CaseResult {
            file: format!("{}/synthetic", self.corpus.name()),
            line: self.step,
            op: self.name.to_owned(),
            label: format!("{} {}: {what}", self.name, self.step),
            height: u64::from(self.state.height()),
            outcome,
        });
    }

    pub(crate) fn params_height(&self) -> u32 {
        self.state.height().saturating_add(1)
    }

    pub(crate) fn nonce(&self, address: &Address) -> u64 {
        self.state
            .wallets()
            .find_by_address(address)
            .map_or(0, heartwood_state::Wallet::nonce)
    }

    pub(crate) fn balance(&self, address: &Address) -> u64 {
        self.state
            .wallets()
            .find_by_address(address)
            .map_or(0, heartwood_state::Wallet::balance)
    }

    /// A test wallet that `accept` accepts, by the seed's order.
    pub(crate) fn wallet(
        &self,
        accept: impl Fn(&Sides<'_>, &DerivedKey) -> bool,
    ) -> Result<&'a DerivedKey> {
        self.keys
            .wallets()
            .find(|k| accept(self, k))
            .ok_or_else(|| Error::Oracle(format!("{}: no test wallet fits the case", self.name)))
    }

    /// A delegate of the seed that `accept` accepts, by the seed's order.
    pub(crate) fn delegate(
        &self,
        accept: impl Fn(&Sides<'_>, &DerivedKey) -> bool,
    ) -> Result<&'a DerivedKey> {
        self.keys
            .delegates()
            .find(|k| accept(self, k))
            .ok_or_else(|| {
                Error::Oracle(format!(
                    "{}: no delegate of the seed fits the case",
                    self.name
                ))
            })
    }

    pub(crate) fn funded(&self, k: &DerivedKey, at_least: u64) -> bool {
        let wallet = self.state.wallets().find_by_address(&k.address);
        wallet.is_some_and(|w| w.balance() >= at_least && w.public_key().is_some())
    }

    pub(crate) fn build(
        &self,
        builder: TransactionBuilder,
        key: &DerivedKey,
    ) -> Result<Transaction> {
        let params = self.milestones.at(self.params_height());
        builder
            .network(params.network())
            .build(&key.keys, Aux::fixed(AUX), params)
            .map_err(|e| Error::Oracle(format!("{}: building a transaction: {e}", self.name)))
    }

    /// The next block on the current state, with `transactions`, in the first slot at least
    /// `gap` slots after the next one whose delegate the seed derives, signed by `signer` (the
    /// scheduled delegate when `None`).
    pub(crate) fn next_block(
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
            .map_err(|e| Error::Oracle(format!("{}: signing a block: {e}", self.name)))?;
            return Ok(serialiser::serialise_with_transactions(
                &header,
                &transactions,
            ));
        }
        Err(Error::Oracle(format!(
            "{}: no slot has a derived delegate",
            self.name
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
                self.commit_applied(&block, &applied);
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
    pub(crate) fn apply(
        &mut self,
        what: &str,
        bytes: &[u8],
        expect_accepted: bool,
        server: &mut OracleServer,
    ) -> Result<bool> {
        Ok(self
            .apply_by(what, bytes, expect_accepted, server, "chain.apply")?
            .0)
    }

    /// [`Sides::apply`] with the oracle operation `op` (`chain.apply`, or `pool.blockApplied`,
    /// which also runs the pool's steps after a block); whether both accepted the block, and the
    /// oracle's answer.
    pub(crate) fn apply_by(
        &mut self,
        what: &str,
        bytes: &[u8],
        expect_accepted: bool,
        server: &mut OracleServer,
        op: &str,
    ) -> Result<(bool, Value)> {
        let before = self.dump(server)?;
        let theirs = call(
            server,
            op,
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
            return Ok((false, theirs));
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
        Ok((oracle_accepted, theirs))
    }

    pub(crate) fn dump(&self, server: &mut OracleServer) -> Result<Value> {
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
    pub(crate) fn compare_records(&mut self, what: &str, server: &mut OracleServer) -> Result<()> {
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
    pub(crate) fn revert(
        &mut self,
        what: &str,
        count: u32,
        server: &mut OracleServer,
    ) -> Result<()> {
        self.revert_by(what, count, server, "chain.revert")?;
        Ok(())
    }

    /// [`Sides::revert`] with the oracle operation `op` (`chain.revert`, or `pool.revert`, which
    /// also re-adds the reverted transactions to the pool); the oracle's answer and the blocks
    /// Heartwood reverted, the last one first.
    pub(crate) fn revert_by(
        &mut self,
        what: &str,
        count: u32,
        server: &mut OracleServer,
        op: &str,
    ) -> Result<(Value, Vec<Block>)> {
        let theirs = call(
            server,
            op,
            1,
            &json!({"session": self.session, "count": count}),
        )?;
        let mut reverted_blocks = Vec::new();
        let their_lines = theirs
            .get("lines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for i in 0..count as usize {
            let ours = match self.state.revert_block(&self.milestones, &Handlers) {
                Ok(reverted) => {
                    self.commit_reverted(&reverted);
                    reverted_blocks.push(reverted.block.clone());
                    digest::height_digest(
                        &self.state,
                        &self.milestones,
                        Step::Revert,
                        &[],
                        None,
                        &[0; 32],
                    )
                }
                Err(e) => {
                    self.record(
                        what,
                        Outcome::HarnessError(format!("Heartwood refused the revert: {e}")),
                    );
                    return Ok((theirs, reverted_blocks));
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
        Ok((theirs, reverted_blocks))
    }

    /// The line of Heartwood's state as a restart line: what every restart of this state must
    /// give.
    fn restart_line(&self, state: &State) -> Value {
        digests::line_value(&digest::height_digest(
            state,
            &self.milestones,
            Step::Restart,
            &[],
            None,
            &[0; 32],
        ))
    }

    /// Stop both nodes and start them again: the reference's from its saved state, then without
    /// it, each from the store before the restart; Heartwood's from its state tables, then by a
    /// replay. Every start must give the state before it (`expect` names the starts that must be
    /// refused instead), apart from the reference's documented differences, which the returned
    /// [`Shown`] lists; the case goes on with the node each side started last.
    fn restart(&mut self, what: &str, expect: Expect, server: &mut OracleServer) -> Result<Shown> {
        let want = self.restart_line(&self.state);
        let continuous = self.state.wallets().clone();
        let mut shown = Shown::default();
        let keys = ["h", "step", "id", "b", "s", "r", "m", "w", "sd"];
        for mode in ["saved", "rebuild"] {
            // The saved path is probed, so that both of the reference's starts begin from the
            // same store (a start that rolls the chain back changes it); the case goes on with the
            // node the rebuild started.
            let r = call(
                server,
                "chain.restart",
                1,
                &json!({"session": self.session, "mode": mode, "probe": mode == "saved"}),
            )?;
            let refusal = expect == Expect::RefusedWithoutSavedState && mode == "rebuild";
            let noted = mode == "saved" && r["notes"]["savedStateUsername"] == Value::Bool(true);
            let no_round_rows = r["notes"]["roundRowsMissing"] == Value::Bool(true);
            let round_rows = |shown: &mut Shown, height: Option<u64>| {
                shown.round_rows.push(height);
                Outcome::Documented(Documented::RoundRowsAfterRevert.reason().to_owned())
            };
            let outcome = match (
                r.pointer("/refused/reason").and_then(Value::as_str),
                refusal,
            ) {
                (Some(reason), true) if reason == NEGATIVE_BALANCE => Outcome::Pass,
                // A name that its saved state loads as a number can stop the reference's start.
                (Some(_), false) if noted => {
                    shown.saved_state_username = true;
                    Outcome::Documented(Documented::SavedStateUsername.reason().to_owned())
                }
                (Some(_), false) if no_round_rows => round_rows(&mut shown, None),
                (Some(reason), _) => {
                    Outcome::HarnessError(format!("the reference refused to start: {reason}"))
                }
                (None, true) => Outcome::HarnessError(
                    "the reference started; the case expects it to refuse the chain".into(),
                ),
                (None, false) => {
                    let line = r.get("line").cloned().unwrap_or(Value::Null);
                    if crate::restart::differing_keys(&line, &want, &keys).is_empty() {
                        Outcome::Pass
                    } else if noted {
                        shown.saved_state_username = true;
                        Outcome::Documented(Documented::SavedStateUsername.reason().to_owned())
                    } else if no_round_rows {
                        round_rows(&mut shown, line["h"].as_u64())
                    } else {
                        Outcome::Mismatch(compare::diff(&want, &line))
                    }
                }
            };
            self.record(
                &format!("{what}: the reference's restart by its {mode} path"),
                outcome,
            );
        }
        for mode in HeartwoodMode::ALL {
            let label = format!("{what}: Heartwood's restart by its {} path", mode.name());
            let Some(StoreSide {
                scratch,
                files,
                store,
            }) = self.store.take()
            else {
                let why = match &self.store_problem {
                    Some(StartProblem::Unbuilt(m)) => format!("not built yet: {m}"),
                    Some(StartProblem::Failed(m)) => m.clone(),
                    None => "the case has no store".into(),
                };
                self.record(&label, Outcome::Skipped(why));
                continue;
            };
            drop(store);
            let refusal =
                expect == Expect::RefusedWithoutSavedState && mode == HeartwoodMode::Replay;
            match StoreNode::start(scratch.path(), &files, store::restore_mode(mode)) {
                Ok(node) => {
                    let (store, state, path) = node.into_parts();
                    let line = self.restart_line(&state);
                    let outcome = if refusal {
                        Outcome::HarnessError(
                            "Heartwood started; the case expects it to refuse the chain".into(),
                        )
                    } else if !store::took_its_path(mode, &path) {
                        Outcome::HarnessError(format!("the start took the path {path:?}"))
                    } else if !crate::restart::differing_keys(&line, &want, &keys).is_empty() {
                        Outcome::Mismatch(compare::diff(&want, &line))
                    } else {
                        let hidden = undigested_differences(&continuous, state.wallets());
                        if hidden.is_empty() {
                            Outcome::Pass
                        } else {
                            Outcome::HarnessError(hidden.join("; "))
                        }
                    };
                    self.state = state;
                    self.store = Some(StoreSide {
                        scratch,
                        files,
                        store,
                    });
                    self.record(&label, outcome);
                }
                Err(problem) => {
                    let outcome = match &problem {
                        StartProblem::Failed(m) if refusal && m.contains(NEGATIVE_BALANCE) => {
                            Outcome::Pass
                        }
                        other => other.outcome(),
                    };
                    self.record(&label, outcome);
                    // Go on from the state tables.
                    match StoreNode::start(scratch.path(), &files, RestoreMode::Auto) {
                        Ok(node) => {
                            let (store, state, _) = node.into_parts();
                            self.state = state;
                            self.store = Some(StoreSide {
                                scratch,
                                files,
                                store,
                            });
                        }
                        Err(again) => {
                            let outcome = again.outcome();
                            self.record("a start from the state tables to go on", outcome);
                            self.store_problem = Some(again);
                        }
                    }
                }
            }
        }
        self.last_missed.clear();
        Ok(shown)
    }

    /// Replace the stored bytes of transaction `index` of the tip block by `bytes` on both sides,
    /// as a database changed on disk would hold them; the state in memory is unchanged.
    fn tamper_tip(
        &mut self,
        block: &Block,
        index: usize,
        bytes: &[u8],
        server: &mut OracleServer,
    ) -> Result<()> {
        call(
            server,
            "chain.replaceStored",
            1,
            &json!({"session": self.session, "height": block.height(), "index": index, "hex": hex::encode(bytes)}),
        )?;
        let Some(side) = &mut self.store else {
            return Ok(());
        };
        let milestones = &self.milestones;
        let done = guarded(|| rewrite_tip(&mut side.store, block, index, bytes, milestones));
        let outcome = match done {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(message)) => Outcome::HarnessError(message),
            Err(store::Unbuilt(message)) => Outcome::Skipped(format!("not built yet: {message}")),
        };
        self.record("the stored transaction changed on disk", outcome);
        Ok(())
    }

    /// Record a failed premise: the case did not set up what it tests.
    pub(crate) fn premise(&mut self, holds: bool, what: &str) {
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
    pub(crate) fn back_to_round_end(
        &mut self,
        before_end: u32,
        server: &mut OracleServer,
    ) -> Result<()> {
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

/// The reference's documented differences that a restart of both nodes showed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Shown {
    /// A restart by the saved state loaded a delegate name as a number.
    saved_state_username: bool,
    /// For each restart that found no round row of the tip's round, the height its start ended
    /// at (none when it refused to start).
    round_rows: Vec<Option<u64>>,
}

/// What a restart of both nodes must give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Every start gives the state before it.
    Started,
    /// The starts without a saved state (the reference's rebuild, Heartwood's replay) refuse the
    /// stored chain with the reference's text; the others give the state before it.
    RefusedWithoutSavedState,
}

/// What a restart restored of the values the digest leaves out, where it differs from the
/// continuous state: each delegate's `resigned_at` and each wallet's vote shares.
fn undigested_differences(
    continuous: &WalletRepository,
    restored: &WalletRepository,
) -> Vec<String> {
    let mut out = Vec::new();
    for wallet in continuous.iter() {
        let other = restored.find_by_address(wallet.address());
        let resigned_at =
            |w: Option<&Wallet>| w.and_then(Wallet::delegate).and_then(|d| d.resigned_at);
        if resigned_at(Some(wallet)) != resigned_at(other) {
            out.push(format!(
                "{}'s resigned_at is {:?} after the restart, {:?} before it",
                wallet.address().to_base58(),
                resigned_at(other),
                resigned_at(Some(wallet))
            ));
        }
        if other.map(Wallet::vote_shares) != Some(wallet.vote_shares())
            && !(other.is_none() && wallet.vote_shares().is_empty())
        {
            out.push(format!(
                "{}'s vote shares differ after the restart",
                wallet.address().to_base58()
            ));
        }
    }
    out.truncate(5);
    out
}

/// The reference's text when a start without a saved state meets a negative balance.
const NEGATIVE_BALANCE: &str = "State Generation failed: Wallet with negative balance";

/// Rewrite the stored tip `block` with the bytes of transaction `index` replaced by `bytes`,
/// through the database's own writes: everything above the block below it is deleted and written
/// again as it was, but for those bytes.
fn rewrite_tip(
    store: &mut ChainStore,
    block: &Block,
    index: usize,
    bytes: &[u8],
    milestones: &Milestones,
) -> std::result::Result<(), String> {
    let tip = block.height();
    let below = tip
        .checked_sub(1)
        .ok_or("the genesis block is not rewritten")?;
    let round_below = heartwood_kernel::calculate_round(below, milestones).round;
    let database = store.database();
    let failed = |e: heartwood_database::DatabaseError| e.to_string();
    let stored = database
        .blocks(tip, tip)
        .map_err(failed)?
        .into_iter()
        .next()
        .ok_or("the tip block is not stored")?;
    let undo = database
        .undo_records()
        .map_err(failed)?
        .into_iter()
        .find(|u| u.height == tip);
    let state_row = database.state_row().map_err(failed)?;
    let missed: Vec<_> = database
        .missed_blocks()
        .map_err(failed)?
        .into_iter()
        .filter(|m| m.height == tip)
        .collect();
    let entries: Vec<_> = database
        .milestone_entries()
        .map_err(failed)?
        .into_iter()
        .filter(|e| e.height == tip)
        .collect();
    let next_round = database.round(round_below + 1).map_err(failed)?;
    let block_row = rows::block_row(block, stored.row.username.as_deref(), milestones)
        .map_err(|e| e.to_string())?;
    let mut transactions = rows::transaction_rows(block, milestones).map_err(|e| e.to_string())?;
    transactions
        .get_mut(index)
        .ok_or("the block has no such transaction")?
        .serialised = bytes.to_vec();
    let mut chunk = store.database_mut().begin().map_err(failed)?;
    chunk.delete_above(below, round_below).map_err(failed)?;
    chunk
        .save_block(&block_row, &transactions)
        .map_err(failed)?;
    chunk.save_round(&next_round).map_err(failed)?;
    chunk.save_missed(&missed).map_err(failed)?;
    if let Some(undo) = undo {
        chunk.put_undo(tip, &undo.record).map_err(failed)?;
    }
    if let Some(row) = state_row {
        chunk.put_state(&row).map_err(failed)?;
    }
    for entry in &entries {
        chunk.put_milestone_entry(entry).map_err(failed)?;
    }
    chunk.commit().map_err(failed)
}

pub(crate) fn transfer(items: &[(Address, u64)]) -> TransactionBuilder {
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
        // The restart cases shorten the revoke delay from the tip plus one, so that a revoke
        // falls a few blocks after its resignation (restart-revoke-delay keeps 55, to restart 50
        // blocks after it).
        c if c.restarts() => {
            let delay = if c == SyntheticCase::RestartRevokeDelay {
                55
            } else {
                RESTART_REVOKE_DELAY
            };
            Some(json!({"height": corpus.tip() + 1, "blocksToRevokeDelegateResignation": delay}))
        }
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
    let mut s = Sides::open(
        corpus,
        keys,
        server,
        case.name(),
        case.restarts(),
        milestones_text.clone(),
        None,
    )?;
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
        SyntheticCase::RestartRevoke => restart_revoke(&mut s, server, false)?,
        SyntheticCase::RestartRevokeDelay => restart_revoke(&mut s, server, true)?,
        SyntheticCase::RestartRoundRanks => restart_round_ranks(&mut s, server)?,
        SyntheticCase::RestartEqualBalances => restart_equal_balances(&mut s, server)?,
        SyntheticCase::RestartNumericNames => restart_numeric_names(&mut s, server)?,
        SyntheticCase::RestartNegativeBalance => {
            restart_negative_balance(&mut s, &recipients, server)?
        }
        SyntheticCase::RestartFork => restart_fork(&mut s, &recipients, server)?,
        SyntheticCase::RestartRoundEndRevert => restart_round_end_revert(&mut s, server, false)?,
        SyntheticCase::RestartRoundEndRevertEarly => {
            restart_round_end_revert(&mut s, server, true)?;
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

/// The revoke delay in force at the next height.
fn revoke_delay(s: &Sides<'_>) -> u32 {
    s.milestones
        .at(s.params_height())
        .blocks_to_revoke_delegate_resignation()
        .and_then(|d| u32::try_from(d).ok())
        .unwrap_or(crate::strategies::transactions::REVOKE_DELAY)
}

/// A new delegate named `name` and a temporary resignation of a delegate of the seed, in one
/// block (the new delegate keeps enough delegates active); the resigned delegate's key.
fn register_and_resign<'k>(
    s: &mut Sides<'k>,
    name: &str,
    server: &mut OracleServer,
) -> Result<&'k DerivedKey> {
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
    let register = s.build(
        TransactionBuilder::delegate_registration(name)
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
    Ok(d)
}

/// Apply empty blocks until the tip is `height`.
fn empty_blocks_to(s: &mut Sides<'_>, height: u32, server: &mut OracleServer) -> Result<()> {
    while s.state.height() < height {
        let bytes = s.next_block(Vec::new(), 0, None)?;
        let h = s.state.height() + 1;
        if !s.apply(&format!("an empty block at {h}"), &bytes, true, server)? {
            return Err(Error::Oracle(format!("the empty block at {h} was refused")));
        }
    }
    Ok(())
}

/// `restart-revoke` and `restart-revoke-delay`: a temporary resignation, a restart, then
/// revokes. The first: the restart right before the first block that may revoke, and the revoke
/// in it. The second (`around`): the restart 50 blocks after the resignation, then revokes one
/// block before the delay, at it and after it (each accepted one taken back, so the next try runs
/// on the same state).
fn restart_revoke(s: &mut Sides<'_>, server: &mut OracleServer, around: bool) -> Result<()> {
    let d = register_and_resign(s, if around { "synthrb" } else { "synthra" }, server)?;
    let resigned_at = s.state.height();
    let delay = revoke_delay(s);
    // A revoke in block H needs (H - 1) - resigned_at >= delay.
    let first_allowed = resigned_at + delay + 1;
    let revoke = |s: &Sides<'_>| {
        s.build(
            TransactionBuilder::delegate_resignation(Some(ResignationType::NotResigned))
                .nonce(s.nonce(&d.address) + 1)
                .fee(FEE),
            d,
        )
    };
    if around {
        empty_blocks_to(s, resigned_at + 50, server)?;
        s.restart(
            "a restart 50 blocks after the resignation",
            Expect::Started,
            server,
        )?;
        empty_blocks_to(s, first_allowed - 2, server)?;
        for (offset, accepted) in [(0u32, false), (1, true), (2, true)] {
            let elapsed = delay - 1 + offset;
            let t = revoke(s)?;
            let bytes = s.next_block(vec![t], 0, None)?;
            let applied = s.apply(
                &format!("a revoke {elapsed} blocks after the resignation (delay {delay})"),
                &bytes,
                accepted,
                server,
            )?;
            if applied {
                s.revert("the revoke taken back", 1, server)?;
            }
            let bytes = s.next_block(Vec::new(), 0, None)?;
            s.apply("an empty block", &bytes, true, server)?;
        }
    } else {
        empty_blocks_to(s, first_allowed - 1, server)?;
        s.restart(
            "a restart before the first block that may revoke",
            Expect::Started,
            server,
        )?;
        let t = revoke(s)?;
        let bytes = s.next_block(vec![t], 0, None)?;
        s.apply(
            "a revoke in the first block after the restart",
            &bytes,
            true,
            server,
        )?;
        s.compare_records("the revoked delegate after the restart", server)?;
    }
    Ok(())
}

/// `restart-round-ranks`: in one round, a temporary resignation, its revoke and a registration, a
/// restart in that round, then the round's end and the first block of the next round.
fn restart_round_ranks(s: &mut Sides<'_>, server: &mut OracleServer) -> Result<()> {
    let d = register_and_resign(s, "synthrc", server)?;
    let resigned_at = s.state.height();
    let delay = revoke_delay(s);
    empty_blocks_to(s, resigned_at + delay, server)?;
    let t = s.build(
        TransactionBuilder::delegate_resignation(Some(ResignationType::NotResigned))
            .nonce(s.nonce(&d.address) + 1)
            .fee(FEE),
        d,
    )?;
    let bytes = s.next_block(vec![t], 0, None)?;
    s.apply("the revoke", &bytes, true, server)?;
    let w = s.wallet(|s, k| {
        s.funded(k, REGISTRATION_FEE * 2)
            && s.state
                .wallets()
                .find_by_address(&k.address)
                .is_some_and(|w| !w.is_delegate())
    })?;
    let t = s.build(
        TransactionBuilder::delegate_registration("synthrc2")
            .nonce(s.nonce(&w.address) + 1)
            .fee(REGISTRATION_FEE),
        w,
    )?;
    let bytes = s.next_block(vec![t], 0, None)?;
    s.apply(
        "a registration in the round of the revoke",
        &bytes,
        true,
        server,
    )?;
    let round_of =
        |h: u32, s: &Sides<'_>| heartwood_kernel::calculate_round(h, &s.milestones).round;
    s.premise(
        round_of(resigned_at + 1, s) <= round_of(s.state.height(), s)
            && round_of(resigned_at + delay + 1, s) == round_of(s.state.height(), s),
        "the revoke and the registration fall in one round",
    );
    s.restart(
        "a restart in the round of the revoke",
        Expect::Started,
        server,
    )?;
    s.compare_records("the ranks and the schedule after the restart", server)?;
    let round = round_of(s.state.height(), s);
    let mut steps = 0;
    while round_of(s.state.height() + 1, s) == round && steps < 60 {
        let bytes = s.next_block(Vec::new(), 0, None)?;
        s.apply("an empty block to the round's end", &bytes, true, server)?;
        steps += 1;
    }
    let bytes = s.next_block(Vec::new(), 0, None)?;
    s.apply("the first block of the next round", &bytes, true, server)?;
    s.compare_records("the ranks and the schedule of the next round", server)?;
    Ok(())
}

/// `restart-equal-balances`: restarts at a round end and at the first block of the next round,
/// whose delegates have equal vote balances.
fn restart_equal_balances(s: &mut Sides<'_>, server: &mut OracleServer) -> Result<()> {
    s.back_to_round_end(0, server)?;
    let balances: Vec<u64> = s
        .state
        .round()
        .round_delegates()
        .iter()
        .map(|d| d.vote_balance)
        .collect();
    let mut sorted = balances.clone();
    sorted.sort_unstable();
    sorted.dedup();
    s.premise(
        sorted.len() < balances.len(),
        "the next round's delegates include equal vote balances",
    );
    s.restart("a restart at a round end", Expect::Started, server)?;
    let bytes = s.next_block(Vec::new(), 0, None)?;
    s.apply("the first block of the next round", &bytes, true, server)?;
    s.restart(
        "a restart at the first block of a round",
        Expect::Started,
        server,
    )?;
    let bytes = s.next_block(Vec::new(), 0, None)?;
    s.apply("the second block of the round", &bytes, true, server)?;
    s.compare_records("the schedule after the restarts", server)?;
    Ok(())
}

/// `restart-numeric-names`: delegates whose names read as integers registered and voted for, then
/// restarts. First `0x10` and `0b101`, which the reference's saved state loads as numbers (a
/// documented difference on that path only); then `1e5`, which makes the saved state unloadable, so
/// the reference falls back to its rebuild and every path agrees.
fn restart_numeric_names(s: &mut Sides<'_>, server: &mut OracleServer) -> Result<()> {
    let mut used = Vec::new();
    for (names, shown) in [(&["0x10", "0b101"][..], true), (&["1e5"][..], false)] {
        let mut registrations = Vec::new();
        for name in names {
            let w = s.wallet(|s, k| {
                s.funded(k, REGISTRATION_FEE * 2)
                    && !used.contains(&k.address)
                    && s.state
                        .wallets()
                        .find_by_address(&k.address)
                        .is_some_and(|w| !w.is_delegate())
            })?;
            used.push(w.address);
            registrations.push(
                s.build(
                    TransactionBuilder::delegate_registration(name)
                        .nonce(s.nonce(&w.address) + 1)
                        .fee(REGISTRATION_FEE),
                    w,
                )?,
            );
        }
        let list = names.join(", ");
        let bytes = s.next_block(registrations, 0, None)?;
        s.apply(&format!("delegates named {list}"), &bytes, true, server)?;
        let voter = s.wallet(|s, k| s.funded(k, 10_000_000_000) && !used.contains(&k.address))?;
        used.push(voter.address);
        let mut votes = Votes::new();
        let shares = crate::strategies::split_now(names.len());
        for (name, bps) in names.iter().zip(shares) {
            votes.insert((*name).to_owned(), bps);
        }
        let t = s.build(
            TransactionBuilder::vote(votes)
                .nonce(s.nonce(&voter.address) + 1)
                .fee(FEE),
            voter,
        )?;
        let bytes = s.next_block(vec![t], 0, None)?;
        s.apply(&format!("a vote for {list}"), &bytes, true, server)?;
        let documented = s
            .restart(
                &format!("a restart with the names {list}"),
                Expect::Started,
                server,
            )?
            .saved_state_username;
        s.premise(
            documented == shown,
            if shown {
                "the reference's saved state loads these names as numbers"
            } else {
                "the reference's saved state falls back to its rebuild"
            },
        );
        let bytes = s.next_block(Vec::new(), 0, None)?;
        s.apply("a block after the restart", &bytes, true, server)?;
        s.compare_records("the named delegates after the restart", server)?;
    }
    Ok(())
}

/// `restart-negative-balance`: a transfer, then its stored bytes changed on both sides to an amount
/// above the sender's balance: a start without a saved state refuses the stored chain.
fn restart_negative_balance(
    s: &mut Sides<'_>,
    recipients: &[Address],
    server: &mut OracleServer,
) -> Result<()> {
    // A sender that votes for no one: its balance, not a delegate's vote balance, goes negative.
    let a = s.wallet(|s, k| {
        s.funded(k, 1_000_000_000)
            && s.state
                .wallets()
                .find_by_address(&k.address)
                .is_some_and(|w| !w.has_voted())
    })?;
    let nonce = s.nonce(&a.address) + 1;
    let t = s.build(transfer(&[(recipients[4], 1_000)]).nonce(nonce).fee(FEE), a)?;
    let bytes = s.next_block(vec![t], 0, None)?;
    s.apply("a transfer", &bytes, true, server)?;
    let block = Block::receive(&bytes, &s.milestones)
        .map_err(|e| Error::Oracle(format!("the transfer's block: {e}")))?;
    let raised = s
        .balance(&a.address)
        .saturating_mul(2)
        .max(1_000_000_000_000);
    let tampered = s.build(
        transfer(&[(recipients[4], raised)]).nonce(nonce).fee(FEE),
        a,
    )?;
    s.tamper_tip(&block, 0, tampered.bytes(), server)?;
    s.restart(
        "a restart over the changed transfer",
        Expect::RefusedWithoutSavedState,
        server,
    )?;
    Ok(())
}

/// `restart-fork`: two blocks after a round end, a restart, then a revert of three blocks across
/// the round end and a competing chain in other slots.
fn restart_fork(
    s: &mut Sides<'_>,
    recipients: &[Address],
    server: &mut OracleServer,
) -> Result<()> {
    s.back_to_round_end(0, server)?;
    let end = s.state.last_block().clone();
    let a = s.wallet(|s, k| s.funded(k, 1_000_000_000))?;
    let t = s.build(
        transfer(&[(recipients[5], 1_000)])
            .nonce(s.nonce(&a.address) + 1)
            .fee(FEE),
        a,
    )?;
    let bytes = s.next_block(vec![t], 0, None)?;
    s.apply("one block into the round", &bytes, true, server)?;
    let bytes = s.next_block(Vec::new(), 0, None)?;
    s.apply("two blocks into the round", &bytes, true, server)?;
    s.restart(
        "a restart two blocks into the round",
        Expect::Started,
        server,
    )?;
    s.revert(
        "the revert across the round end after the restart",
        3,
        server,
    )?;
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
        "the competing round end, a slot later",
        &bytes,
        true,
        server,
    )?;
    s.premise(
        s.state.last_block().id() != end.id(),
        "the competing block is another block",
    );
    let bytes = s.next_block(Vec::new(), 2, None)?;
    s.apply(
        "the competing branch, one block into the round",
        &bytes,
        true,
        server,
    )?;
    s.restart("a restart on the competing branch", Expect::Started, server)?;
    let bytes = s.next_block(Vec::new(), 0, None)?;
    s.apply(
        "the competing branch, two blocks into the round",
        &bytes,
        true,
        server,
    )?;
    s.compare_records("the state at the competing tip", server)?;
    Ok(())
}

/// A revert to the last block of a round, the next round's first two blocks applied again, then a
/// restart in that round. The reference's revert deletes the round rows above the new tip's round
/// (the rows of the next round, which the running node saved with the round's last block and does
/// not save again), and Heartwood's store deletes the same rows; the reference's restarts then read
/// no delegate for the round, Heartwood's restart from its state. At the corpus's tip (`early`
/// false) the store holds missed-block rows, and the reference keeps an empty forging list;
/// `early` reverts to the last round end below the store's first missed-block row, and the
/// reference, finding no missed row, rolls the chain back.
fn restart_round_end_revert(
    s: &mut Sides<'_>,
    server: &mut OracleServer,
    early: bool,
) -> Result<()> {
    let store_rows = |s: &Sides<'_>, round: u64| {
        s.store.as_ref().map(|side| {
            let database = side.store.database();
            let rows = database.round(round).map(|rows| rows.len());
            let missed = database.missed_blocks();
            (rows.ok(), missed.ok().unwrap_or_default())
        })
    };
    if early {
        let tip = s.state.height();
        let (_, missed) = store_rows(s, 0).unwrap_or_default();
        let first = missed.iter().map(|m| m.height).min().unwrap_or(tip);
        let mut end = first.saturating_sub(1).min(tip);
        while end > 1 && !is_new_round(end + 1, &s.milestones) {
            end -= 1;
        }
        if end <= 1 || end + 2 > tip {
            s.premise(
                false,
                "a round end below the first missed-block row, two blocks below the tip",
            );
            return Ok(());
        }
        s.revert(
            "the revert to the last round end below the first missed-block row",
            tip - end,
            server,
        )?;
        for (height, what) in [
            (end + 1, "the next round's first block again"),
            (end + 2, "the next round's second block again"),
        ] {
            let bytes = s
                .corpus
                .blocks
                .iter()
                .find(|b| b.height == height)
                .map(|b| b.bytes.clone())
                .ok_or_else(|| Error::Oracle(format!("the corpus has no block {height}")))?;
            s.apply(what, &bytes, true, server)?;
        }
    } else {
        s.back_to_round_end(0, server)?;
        let first = s.next_block(Vec::new(), 0, None)?;
        s.apply("the first block of the next round", &first, true, server)?;
        s.revert("the revert to the round's last block", 1, server)?;
        s.apply("the first block of the round again", &first, true, server)?;
        let second = s.next_block(Vec::new(), 0, None)?;
        s.apply("the second block of the round", &second, true, server)?;
    }
    let tip = s.state.height();
    let round = heartwood_kernel::calculate_round(tip, &s.milestones).round;
    let (rows, missed) = store_rows(s, round).unwrap_or_default();
    s.premise(
        rows == Some(0),
        "Heartwood's store holds no row of the round, as the reference's revert leaves its table",
    );
    s.premise(
        missed.is_empty() == early,
        if early {
            "the store holds no missed-block row"
        } else {
            "the store holds missed-block rows"
        },
    );
    let shown = s.restart(
        "a restart in the round whose rows the revert deleted",
        Expect::Started,
        server,
    )?;
    let tip = u64::from(tip);
    let as_documented = shown.round_rows.len() == 2
        && shown.round_rows.iter().all(|height| match height {
            Some(height) if early => *height < tip,
            Some(height) => *height == tip,
            None => false,
        });
    s.premise(
        as_documented,
        if early {
            "both of the reference's restarts roll the chain back"
        } else {
            "both of the reference's restarts keep the tip with no delegate for the round"
        },
    );
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
