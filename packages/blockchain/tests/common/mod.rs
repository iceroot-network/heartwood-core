//! The committed one-node chain as a node's configuration and blocks, a store in a fresh
//! directory, and a replay of the chain's steps that commits each one to the store.

#![allow(dead_code, reason = "each test file uses a part of the helpers")]

use std::fs;
use std::path::{Path, PathBuf};

use heartwood_blockchain::{ChainStore, RestoreMode, RestorePath, Restored, StartError, restore};
use heartwood_crypto::blocks::factory;
use heartwood_crypto::{Block, Milestones, Network};
use heartwood_database::Settings;
use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_state::State;
use heartwood_state::digest::{self, DigestLine, SectionHashes, Step};
use heartwood_state::round_state::MissedBlock;
use heartwood_transactions::{Handlers, TransactionError};
use serde_json::Value;

/// The committed one-node chain.
pub struct Chain {
    pub files: CryptoFiles,
    pub blocks: Vec<Block>,
    /// The revert plan: after the block at the first height, revert the second number of blocks
    /// and apply them again.
    pub plan: Vec<(u32, u32)>,
    /// The oracle's digest lines, one per step, the genesis step first.
    pub digests: Vec<Value>,
}

/// One step after the genesis step: a height to apply (`Some`, `true` for its first apply), or a
/// revert (`None`).
pub type ChainStep = (Option<u32>, bool);

pub fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/difftest/corpus/txgen-one-node")
}

pub fn read(name: &str) -> String {
    fs::read_to_string(corpus().join(name)).unwrap()
}

pub fn lines(name: &str) -> Vec<Value> {
    read(name)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The crypto files of the corpus with `milestones_text` in place of its milestone file.
pub fn files_with(milestones_text: &str) -> CryptoFiles {
    let network = Network::load(&read("crypto/network.json")).unwrap();
    let milestones = Milestones::load(milestones_text, &network).unwrap();
    CryptoFiles {
        network,
        milestones,
        milestones_text: milestones_text.to_owned(),
        genesis: serde_json::from_str(&read("crypto/genesisBlock.json")).unwrap(),
    }
}

impl Chain {
    pub fn load() -> Chain {
        let files = files_with(&read("crypto/milestones.json"));
        let blocks = lines("blocks.jsonl")
            .iter()
            .map(|line| {
                let bytes = hex::decode(line["hex"].as_str().unwrap()).unwrap();
                Block::receive(&bytes, &files.milestones).unwrap()
            })
            .collect();
        let plan: Value = serde_json::from_str(&read("revert-plan.json")).unwrap();
        let plan = plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| {
                let number = |key: &str| u32::try_from(step[key].as_u64().unwrap()).unwrap();
                (number("after"), number("blocks"))
            })
            .collect();
        Chain {
            files,
            blocks,
            plan,
            digests: lines("ts-digests.jsonl"),
        }
    }

    pub fn milestones(&self) -> &Milestones {
        &self.files.milestones
    }

    pub fn genesis(&self) -> Block {
        factory::from_json(&self.files.genesis, self.milestones()).unwrap()
    }

    pub fn block(&self, height: u32) -> &Block {
        &self.blocks[height as usize - 2]
    }

    pub fn tip(&self) -> u32 {
        self.blocks.last().unwrap().height()
    }

    /// The steps of the chain after the genesis step, with the revert plan.
    pub fn steps(&self) -> Vec<ChainStep> {
        let mut steps = Vec::new();
        for block in &self.blocks {
            let height = block.height();
            steps.push((Some(height), true));
            for &(_, count) in self.plan.iter().filter(|(after, _)| *after == height) {
                steps.extend(std::iter::repeat_n((None, false), count as usize));
                steps.extend((height - count + 1..=height).map(|h| (Some(h), false)));
            }
        }
        steps
    }

    /// The oracle's section hashes of line `index` (from 0, the genesis step).
    pub fn sections(&self, index: usize) -> [String; 5] {
        let line = &self.digests[index];
        ["b", "s", "r", "m", "w"].map(|key| line[key].as_str().unwrap().to_owned())
    }
}

/// The section hashes as the oracle's lines write them.
pub fn hex_sections(hashes: &SectionHashes) -> [String; 5] {
    [
        hashes.block,
        hashes.supply,
        hashes.round,
        hashes.missed,
        hashes.wallets,
    ]
    .map(hex::encode)
}

/// A fresh, empty directory for a test's store.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("blockchain-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn settings(dir: &Path) -> Settings {
    Settings::in_data_dir(dir)
}

/// Open the store in `dir` and restore the state from it.
pub fn start(
    dir: &Path,
    files: &CryptoFiles,
    mode: RestoreMode,
) -> Result<(ChainStore, Restored), StartError<TransactionError>> {
    let mut store = ChainStore::open(&settings(dir))?;
    let restored = restore(&mut store, files, &Handlers, mode)?;
    Ok((store, restored))
}

/// A node that replays the chain's steps, commits each one to its store in a chunk of its own,
/// and writes the digest lines as the L2 replay does.
pub struct Node<'c> {
    pub chain: &'c Chain,
    pub dir: PathBuf,
    store: Option<ChainStore>,
    pub state: State,
    pub previous: [u8; 32],
    pub lines: Vec<DigestLine>,
}

impl<'c> Node<'c> {
    /// A node on a first start in a fresh store.
    pub fn first_start(chain: &'c Chain, name: &str) -> Node<'c> {
        let dir = scratch(name);
        let (store, restored) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
        assert_eq!(restored.path, RestorePath::FirstStart);
        let mut node = Node {
            chain,
            dir,
            store: Some(store),
            state: restored.state,
            previous: [0; 32],
            lines: Vec::new(),
        };
        node.line(Step::Genesis, &[], None);
        node
    }

    pub fn line(&mut self, step: Step, missed: &[MissedBlock], reward: Option<u64>) {
        let milestones = self.chain.milestones();
        let line = digest::height_digest(
            &self.state,
            milestones,
            step,
            missed,
            reward,
            &self.previous,
        );
        self.previous = line.chain_digest;
        self.lines.push(line);
    }

    /// Run one step and commit it to the store.
    pub fn step(&mut self, step: ChainStep) {
        self.steps(&[step]);
    }

    /// Run `steps` and commit them to the store in one chunk.
    pub fn steps(&mut self, steps: &[ChainStep]) {
        let milestones = self.chain.milestones();
        let mut chunk = self.store.as_mut().unwrap().chunk().unwrap();
        let mut lines = Vec::new();
        for &(height, first) in steps {
            match height {
                None => {
                    let reverted = self.state.revert_block(milestones, &Handlers).unwrap();
                    chunk.reverted(&self.state, &reverted, milestones).unwrap();
                    lines.push((Step::Revert, Vec::new(), None));
                }
                Some(height) => {
                    let block = self.chain.block(height);
                    let generator = block.header().generator_public_key;
                    let reward = (self.state)
                        .reward_for_block_in_round(height, &generator, milestones)
                        .unwrap()
                        .reward;
                    let applied = self
                        .state
                        .apply_block(block, milestones, &Handlers)
                        .unwrap();
                    chunk
                        .applied(&self.state, block, &applied, milestones)
                        .unwrap();
                    let step = if first { Step::Apply } else { Step::Reapply };
                    lines.push((step, applied.missed, Some(reward)));
                }
            }
            // The line of each step is the state's right after it.
            let (step, missed, reward) = lines.pop().unwrap();
            let line = digest::height_digest(
                &self.state,
                milestones,
                step,
                &missed,
                reward,
                &self.previous,
            );
            self.previous = line.chain_digest;
            self.lines.push(line);
        }
        chunk.commit().unwrap();
    }

    /// The node's store.
    pub fn store(&mut self) -> &mut ChainStore {
        self.store.as_mut().unwrap()
    }

    /// The node's store and state, to change both.
    pub fn parts(&mut self) -> (&mut ChainStore, &mut State) {
        (self.store.as_mut().unwrap(), &mut self.state)
    }

    /// Close the store and start again from it with `mode`.
    pub fn restart(&mut self, mode: RestoreMode) -> RestorePath {
        self.store = None;
        let (store, restored) = start(&self.dir, &self.chain.files, mode).unwrap();
        self.store = Some(store);
        self.state = restored.state;
        restored.path
    }

    /// Close the store, and start again from it with `files` instead of the chain's. The store
    /// stays open when the start is refused.
    pub fn restart_with(
        &mut self,
        files: &CryptoFiles,
        mode: RestoreMode,
    ) -> Result<Restored, StartError<TransactionError>> {
        self.store = None;
        let mut store = ChainStore::open(&settings(&self.dir))?;
        let result = restore(&mut store, files, &Handlers, mode);
        self.store = Some(store);
        if let Ok(restored) = &result {
            self.state = restored.state.clone();
        }
        result
    }

    /// The last line as JSON.
    pub fn last_json(&self) -> Value {
        serde_json::from_str(&self.lines.last().unwrap().to_json()).unwrap()
    }
}

/// `restored` holds what `state` holds, apart from the wallets that nothing happened to (or that
/// are back to that), which the store does not keep.
pub fn assert_same_state(restored: &State, state: &State, context: &str) {
    let visible = |wallet: &&heartwood_state::Wallet| {
        **wallet != heartwood_state::Wallet::new(*wallet.address())
    };
    let wallets = |state: &State| {
        let mut wallets: Vec<_> = state.wallets().iter().filter(visible).cloned().collect();
        wallets.sort_by_key(|wallet| *wallet.address());
        wallets
    };
    assert_eq!(wallets(restored), wallets(state), "{context}: wallets");
    assert_eq!(restored.round(), state.round(), "{context}: round");
    assert_eq!(restored.totals(), state.totals(), "{context}: totals");
    assert_eq!(
        restored.last_block(),
        state.last_block(),
        "{context}: last block"
    );
    assert_eq!(
        restored.genesis_block(),
        state.genesis_block(),
        "{context}: genesis"
    );
    assert_eq!(restored.journal(), state.journal(), "{context}: journal");
}
