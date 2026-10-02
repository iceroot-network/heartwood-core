//! The scenario format: one JSON file per scenario, for the lockstep producer (L2), a devnet of
//! either implementation (L3) and the mixed network (L4). Every object refuses a key it does not
//! know, so a misspelt key fails the file instead of being ignored.
//!
//! ```json
//! {
//!   "id": "double-spend-by-nonce", "title": "Double spend by nonce", "levels": ["L2", "L3"],
//!   "network": "devnet-5", "genesisSeed": "public-seed", "settleBlocks": 3,
//!   "actors": {"alice": {"seed": 1}, "bob": {"seed": 2}},
//!   "steps": [
//!     {"at": {"height": 5}, "do": "submit", "node": 1, "via": "api",
//!      "txs": [{"ref": "fund", "type": "transfer", "from": "genesis",
//!               "to": [["alice", "100000000000"]]}]},
//!     {"at": {"afterBlocks": 2}, "do": "submit", "node": 1, "via": "api",
//!      "txs": [{"ref": "t1", "type": "transfer", "from": "alice", "nonce": 1,
//!               "to": [["bob", "60000000000"]]},
//!              {"ref": "t2", "type": "transfer", "from": "alice", "nonce": 1,
//!               "to": [["bob", "60000000000"]]}],
//!      "check": {"accept": ["t1"], "invalid": ["t2"]}},
//!     {"at": {"afterBlocks": 2}, "check": {"forged": ["t1"], "absent": ["t2"], "withinBlocks": 2}},
//!     {"at": {"round": 2, "position": "start"}, "do": "stop", "node": 2, "role": "forger"},
//!     {"at": {"roundEnd": true}, "check": {"digest": true}}
//!   ]
//! }
//! ```
//!
//! A step fires when the observed tip first satisfies its trigger (`at`, exactly one of its
//! forms); in live levels a trigger is a lower bound, and a check that depends on timing gives
//! `withinBlocks`. A step does an action (`do`), checks (`check`), or both: a `submit` with a
//! `check` of `accept` and `invalid` checks the pools' answers to its transactions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scenario {
    /// The scenario's id, a content name such as `double-spend-by-nonce`.
    pub id: String,
    /// What it shows, in one line.
    pub title: String,
    /// Where it comes from, when it is adapted from an earlier test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The levels it runs at.
    pub levels: Vec<Level>,
    /// The network profile, such as `devnet-5` (five nodes, 53 delegates split 11/11/11/10/10).
    pub network: String,
    /// The public seed of the network's genesis and of the actors' keys.
    pub genesis_seed: String,
    /// Blocks to wait after the start before the first step (the start-up window).
    #[serde(default)]
    pub settle_blocks: u32,
    /// Variables every node's environment file gains (such as a lowered
    /// `CORE_P2P_MIN_NETWORK_REACH`), the same on both implementations.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The scenario's wallets by name, each derived from the genesis seed and its own seed;
    /// `genesis` names the genesis wallet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actors: BTreeMap<String, Actor>,
    /// The steps, in order.
    pub steps: Vec<Step>,
}

/// A level a scenario runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    /// Through the lockstep producer, without a network.
    L2,
    /// On a devnet of one implementation.
    L3,
    /// On the mixed network of both.
    L4,
}

/// A wallet of the scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Actor {
    /// The wallet's seed, beside the genesis seed.
    pub seed: u64,
}

/// One step: when it fires, what it does and what it checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Step {
    /// A name for the reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// When it fires.
    pub at: Trigger,
    /// The action, if any.
    #[serde(rename = "do", default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    /// How a `submit` reaches its node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Via>,
    /// The node the action acts on or submits to, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
    /// The part of the node a `stop`, `start` or `restart` acts on: the whole node by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Part>,
    /// How a `restart` restarts, per implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RestartModes>,
    /// The node groups of a `partition`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Vec<u32>>,
    /// The blocks of a `revert` or a `wait`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<u32>,
    /// The branch a `fork` switches to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The transactions of a `submit`, each compiled once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub txs: Vec<TxSpec>,
    /// What the step checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
}

/// When a step fires: exactly one of the forms (`round` with `position`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Trigger {
    /// At the first tip at or above this height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// This many blocks after the previous step fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_blocks: Option<u32>,
    /// At this slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u64>,
    /// This many slots after the previous step fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots_after: Option<u32>,
    /// At the start or the end of this round (with `position`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u64>,
    /// Which end of `round`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<RoundPosition>,
    /// At the last block of the current round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_end: Option<bool>,
}

/// Which end of a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoundPosition {
    /// Its first block.
    Start,
    /// Its last block.
    End,
}

/// An action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Submit the step's transactions to its node.
    Submit,
    /// Stop a node or a part of it.
    Stop,
    /// Start a node or a part of it.
    Start,
    /// Restart a node or a part of it, as `mode` says.
    Restart,
    /// Split the nodes into `groups`.
    Partition,
    /// Join them again.
    Heal,
    /// Take back `blocks` blocks (L2 only).
    Revert,
    /// Switch to the branch `branch` (L2 only).
    Fork,
    /// Wait `blocks` blocks.
    Wait,
}

/// How a transaction reaches its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    /// The public API's `POST /api/transactions`.
    Api,
    /// The peer-to-peer `postTransactions`.
    P2p,
}

/// The part of a node an action acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Part {
    /// The relay.
    Relay,
    /// The forger.
    Forger,
    /// Both.
    Node,
}

/// How a restart restarts, per implementation: the reference from its saved state or by its
/// StateBuilder, Heartwood by its table path or its replay path (the defaults are the saved state
/// and the table path, a graceful stop and start).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestartModes {
    /// The reference's.
    #[serde(default)]
    pub reference: ReferenceRestart,
    /// Heartwood's.
    #[serde(default)]
    pub heartwood: HeartwoodRestart,
}

/// The reference's restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceRestart {
    /// From its saved state.
    #[default]
    Saved,
    /// By its StateBuilder.
    Rebuild,
}

/// Heartwood's restart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HeartwoodRestart {
    /// From its stored tables.
    #[default]
    Table,
    /// By replaying its stored blocks.
    Replay,
}

/// A transaction to compile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TxSpec {
    /// Its name in the scenario's checks.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Its type: `transfer`, `vote`, `delegateRegistration`, `delegateResignation`,
    /// `secondSignature`, `burn` or `ipfs`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The sending actor.
    pub from: String,
    /// Its nonce: `"next"` (the sender's at compile time plus one) or a number.
    #[serde(default)]
    pub nonce: Nonce,
    /// A transfer's recipients (actors) and amounts (satoshi, as text).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<(String, String)>,
    /// Its memo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Its fee (satoshi, as text); the dynamic fee floor at the planned height when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee: Option<String>,
    /// The type's asset beyond the recipients: votes, a username, a resignation type, a hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<Value>,
    /// The actor whose key second-signs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_signature: Option<String>,
}

/// A transaction's nonce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NonceText", into = "NonceText")]
pub enum Nonce {
    /// The sender's nonce at compile time plus one.
    #[default]
    Next,
    /// This nonce.
    At(u64),
}

/// A nonce as the file writes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum NonceText {
    Number(u64),
    Text(String),
}

impl TryFrom<NonceText> for Nonce {
    type Error = String;

    fn try_from(text: NonceText) -> Result<Nonce, String> {
        match text {
            NonceText::Number(nonce) => Ok(Nonce::At(nonce)),
            NonceText::Text(text) if text == "next" => Ok(Nonce::Next),
            NonceText::Text(text) => Err(format!("{text:?} is not a nonce: \"next\" or a number")),
        }
    }
}

impl From<Nonce> for NonceText {
    fn from(nonce: Nonce) -> NonceText {
        match nonce {
            Nonce::Next => NonceText::Text("next".to_owned()),
            Nonce::At(nonce) => NonceText::Number(nonce),
        }
    }
}

/// What a step checks. At least one part is given.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Check {
    /// Transactions every pool admits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accept: Vec<String>,
    /// Transactions every pool refuses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid: Vec<String>,
    /// The refusal code of a refused transaction, such as `ERR_OFFLINE`, by its name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub codes: BTreeMap<String, String>,
    /// Transactions in the chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forged: Vec<String>,
    /// Transactions in no block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
    /// The implementations' per-height digests agree up to here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub digest: bool,
    /// A named projection of the API's answers, equal on every node given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<ApiCheck>,
    /// Missed slots recorded for delegates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missed: Option<MissedCheck>,
    /// Invariants that hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invariant: Vec<Invariant>,
    /// In live levels, the blocks the check may take to hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_blocks: Option<u32>,
}

/// An API projection's check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiCheck {
    /// The projection's name, such as `wallets` (the scenario wallets' balances and nonces).
    pub projection: String,
    /// The nodes compared; every node when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<u32>,
}

/// A missed slots' check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissedCheck {
    /// The node whose forger's delegates missed.
    pub node: u32,
    /// The fewest missed slots recorded.
    pub at_least: u32,
}

/// A named invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Invariant {
    /// The supply equals the genesis amount plus the rewards less the burns.
    Supply,
    /// Every node that is meant to be up is on one tip.
    SingleTip,
    /// Every block was forged by a delegate of its round (round membership; that the forger is
    /// the one its slot names is judged with the chain against the reference).
    Schedule,
    /// A revert and its reapply give the same states.
    RevertSymmetry,
    /// A restart gives the state of the continuous run.
    RestartEqual,
    /// While a partition holds, each of its groups forges a branch of its own: at one height
    /// above the one the partition was made at (the lowest of the groups' highest tips), each
    /// group's highest node holds a block of its own, no two groups the same, forged by a
    /// delegate of a node of that group (as the backend names each node's delegates).
    BothSidesForge,
    /// After a heal, the branches are joined into one chain: every node that is meant to be up is
    /// on one tip, at or above the branches' height, and the chain's block at that height (a
    /// block the API answers with) is not every block [`Invariant::BothSidesForge`] saw there.
    ForkRecovered,
}

/// Every transaction of a scenario, built once through the oracle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Compiled {
    /// The scenario.
    pub scenario: Scenario,
    /// Its transactions, in step order.
    pub transactions: Vec<CompiledTx>,
}

/// One compiled transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompiledTx {
    /// Its name in the scenario.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The index of its step.
    pub step: usize,
    /// The height it was planned for, which its fee floor and strict decode used.
    pub height: u32,
    /// Its id, in hex.
    pub id: String,
    /// Its bytes, in hex.
    pub bytes: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scenario_reads_back_as_written_and_an_unknown_key_is_refused() {
        let text = r#"{
            "id": "double-spend", "title": "t", "levels": ["L2", "L3"], "network": "devnet-5",
            "genesisSeed": "s", "settleBlocks": 3, "actors": {"alice": {"seed": 1}},
            "steps": [
                {"at": {"height": 5}, "do": "submit", "node": 1, "via": "api",
                 "txs": [{"ref": "t1", "type": "transfer", "from": "genesis", "nonce": 3,
                          "to": [["alice", "100"]]}],
                 "check": {"accept": ["t1"], "codes": {"t2": "ERR_OFFLINE"}}},
                {"at": {"round": 2, "position": "start"}, "do": "restart", "node": 2,
                 "role": "relay", "mode": {"heartwood": "replay"}},
                {"at": {"roundEnd": true}, "check": {"digest": true, "invariant": ["singleTip"]}}
            ]
        }"#;
        let scenario: Scenario = serde_json::from_str(text).unwrap();
        assert_eq!(scenario.steps[0].txs[0].nonce, Nonce::At(3));
        let mode = scenario.steps[1].mode.unwrap();
        assert_eq!(mode.reference, ReferenceRestart::Saved);
        assert_eq!(mode.heartwood, HeartwoodRestart::Replay);
        let again: Scenario =
            serde_json::from_str(&serde_json::to_string(&scenario).unwrap()).unwrap();
        assert_eq!(again, scenario);
        for bad in [
            text.replace("\"settleBlocks\"", "\"settle\""),
            text.replace("\"nonce\": 3", "\"nonce\": \"last\""),
            text.replace("\"roundEnd\": true", "\"roundEnd\": true, \"late\": 1"),
            text.replace("\"api\"", "\"http\""),
        ] {
            assert!(serde_json::from_str::<Scenario>(&bad).is_err(), "{bad}");
        }
    }
}
