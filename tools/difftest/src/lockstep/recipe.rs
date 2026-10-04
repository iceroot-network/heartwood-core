//! A lockstep chain's recipe: what the producer does at each height of a chain it forges with
//! Heartwood's forger while the reference applies every block beside it.
//!
//! A recipe is JSON: its wallets' labels and its steps, each tagged with `step`. Senders and
//! recipients are named by label: a genesis delegate by its name (`genesis_1`), a recipe wallet by
//! a label of [`Recipe::wallets`], whose keys derive from the chain's seed and the label.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A lockstep chain's recipe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Recipe {
    /// The labels of the recipe's own wallets, whose keys derive from the seed and the label.
    #[serde(default)]
    pub wallets: Vec<String>,
    /// The steps, in height order; steps at one height run in their order.
    pub steps: Vec<Step>,
}

/// One step of a recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "camelCase", deny_unknown_fields)]
pub enum Step {
    /// Transactions the reference builds once and both pools are given before the block at
    /// `height` is forged; each pool's verdict and error code must be the other's.
    #[serde(rename_all = "camelCase")]
    Transactions {
        /// The height of the block they are submitted before.
        height: u32,
        /// The transactions, in submission order.
        build: Vec<BuildSpec>,
    },
    /// A delegate's forger stopped: its slots are missed while the chain grows from `from_height`
    /// to `to_height`.
    #[serde(rename_all = "camelCase")]
    Missed {
        /// The delegate, by name.
        delegate: String,
        /// The first height whose slots it misses.
        from_height: u32,
        /// The last height whose slots it misses.
        to_height: u32,
    },
    /// At the start of `round`, the online entries a relay would record from its forgers'
    /// attestations, given to both pools' online records.
    #[serde(rename_all = "camelCase")]
    Online {
        /// The round whose start it is.
        round: u64,
        /// The delegates attested.
        delegates: Attested,
    },
    /// A revert after `after_height` of `blocks` blocks, which are then applied again, as a
    /// corpus's revert plan runs it.
    #[serde(rename_all = "camelCase")]
    Revert {
        /// The height after which the revert runs.
        after_height: u32,
        /// The blocks reverted.
        blocks: u32,
    },
    /// A fork after `after_height`: `blocks` blocks reverted and a branch of as many forged in
    /// their place with the next forging order's slots, the reverted branch's transactions
    /// submitted again.
    #[serde(rename_all = "camelCase")]
    Fork {
        /// The height after which the fork runs.
        after_height: u32,
        /// The blocks replaced.
        blocks: u32,
    },
    /// A restart point: `l2-restart` restarts both implementations at `height`.
    #[serde(rename_all = "camelCase")]
    Restart {
        /// The height of the restart.
        height: u32,
    },
}

/// The delegates an online step attests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum Attested {
    /// Every delegate of the round.
    All,
    /// Every delegate of the round but these, by name.
    AllBut(Vec<String>),
    /// These delegates only, by name.
    Only(Vec<String>),
}

/// One transaction as the reference's builders make it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BuildSpec {
    /// The kept type, by the reference's builder name (`transfer`, `vote`, `delegateRegistration`,
    /// `delegateResignation`, `burn`, `ipfs`).
    pub kind: String,
    /// The sender, by label.
    pub sender: String,
    /// The type's own fields, with labels for wallets and decimal strings for amounts: a
    /// transfer's `recipients` (label and amount), a vote's `votes` (name and percent), a
    /// registration's `username`, a resignation's `resignationType`, a burn's `amount`, an IPFS
    /// hash.
    #[serde(default)]
    pub asset: Value,
    /// The fee: the network's floor when absent, else a decimal string.
    #[serde(default)]
    pub fee: Option<String>,
    /// The memo.
    #[serde(default)]
    pub memo: Option<String>,
    /// The pools' expected verdict: accepted when absent, else the error code both must answer
    /// (`ERR_OFFLINE` for a vote for a delegate that is not attested).
    #[serde(default)]
    pub refused: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_recipe_reads_its_steps_and_refuses_an_unknown_field() {
        let text = json!({
            "wallets": ["alice"],
            "steps": [
                {"step": "online", "round": 1, "delegates": {"allBut": ["genesis_51", "genesis_52", "genesis_53"]}},
                {"step": "transactions", "height": 3, "build": [
                    {"kind": "transfer", "sender": "genesis_1",
                     "asset": {"recipients": [["alice", "1000"]]}},
                    {"kind": "vote", "sender": "alice", "asset": {"votes": [["genesis_52", "100"]]},
                     "refused": "ERR_OFFLINE"}
                ]},
                {"step": "missed", "delegate": "genesis_7", "fromHeight": 10, "toHeight": 20},
                {"step": "revert", "afterHeight": 53, "blocks": 2},
                {"step": "fork", "afterHeight": 80, "blocks": 3},
                {"step": "restart", "height": 106}
            ]
        });
        let recipe: Recipe = serde_json::from_value(text.clone()).unwrap();
        assert_eq!(recipe.steps.len(), 6);
        assert_eq!(
            serde_json::to_value(&recipe).unwrap()["steps"][2],
            text["steps"][2]
        );
        let unknown = json!({"steps": [{"step": "restart", "height": 1, "at": 2}]});
        assert!(serde_json::from_value::<Recipe>(unknown).is_err());
    }
}
