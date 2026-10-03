//! The corpus a lockstep chain is written as: the blocks through `corpus from-blocks`, then the
//! chain's own plans and the forger's aux.

use std::path::Path;

use heartwood_crypto::utils::hex;
use serde_json::Value;

use super::LockstepError;
use super::producer::Made;
use crate::corpus::{self, RevertPlan, RevertStep};
use crate::corpus_from_blocks;
use crate::restart::{PlanKind, RestartPlan};
use crate::store::ScratchDir;

fn io(path: &Path, source: std::io::Error) -> LockstepError {
    LockstepError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn corpus_error(e: impl std::fmt::Display) -> LockstepError {
    LockstepError::Heartwood(format!("the corpus: {e}"))
}

fn pretty(value: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(value).unwrap_or_default()
    )
}

/// The revert plan of a chain of `tip` blocks: an ordinary corpus's plan (every round end and
/// the random reverts of `seed`), with the recipe's `reverts`, which the producer ran on both
/// sides, joined in height order; and the generated steps it changed. A generated revert that
/// would return to a round's last block reverts one block more, as a live chain's plan does
/// ([`RevertPlan::move_round_ends`]): the judge's node deletes the next round's rows there and does
/// not save them again, so the step would test the plan, not the chain.
fn revert_plan(
    seed: u64,
    tip: u32,
    round_length: u32,
    reverts: &[RevertStep],
) -> (RevertPlan, Vec<Value>) {
    let mut plan = RevertPlan::generate(seed, tip, round_length);
    let adjusted = plan.move_round_ends();
    for step in reverts {
        if !plan.steps.contains(step) {
            plan.steps.push(*step);
        }
    }
    plan.steps.sort_by_key(|s| s.after_height);
    (plan, adjusted)
}

/// The chain `made`, with the crypto files `crypto` (name and text), from `seed` and the forger's
/// `aux`, as a corpus in `out` (which must be empty or missing).
pub fn write(
    made: &Made,
    crypto: &[(&str, String)],
    seed: &str,
    aux: &[u8; 32],
    round_length: u32,
    out: &Path,
) -> Result<(), LockstepError> {
    let scratch = ScratchDir::new("lockstep-corpus").map_err(corpus_error)?;
    let dir = scratch.path().join("crypto");
    std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    for (name, text) in crypto {
        let path = dir.join(format!("{name}.json"));
        std::fs::write(&path, text).map_err(|e| io(&path, e))?;
    }
    let blocks = scratch.path().join("blocks-0001.bin");
    let lines: String = made
        .blocks
        .iter()
        .map(|b| {
            format!(
                "{}\n",
                serde_json::json!({"h": b.height, "id": b.id, "hex": b.hex})
            )
        })
        .collect();
    std::fs::write(&blocks, lines).map_err(|e| io(&blocks, e))?;
    corpus_from_blocks::from_blocks(&[blocks], &dir, seed, out).map_err(corpus_error)?;

    let tip = made.blocks.last().map_or(0, |b| b.height);
    let meta_path = out.join("corpus.json");
    let text = std::fs::read_to_string(&meta_path).map_err(|e| io(&meta_path, e))?;
    let mut meta: Value = serde_json::from_str(&text).map_err(corpus_error)?;
    let revert_seed: u64 = meta["revertSeed"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let (plan, adjusted) = revert_plan(revert_seed, tip, round_length, &made.reverts);
    meta["revertSteps"] = plan.steps.len().into();
    if !adjusted.is_empty() {
        meta["revertPlanAdjusted"] = Value::Array(adjusted);
    }
    meta["forgerAux"] = hex::encode(aux).into();
    std::fs::write(&meta_path, pretty(&meta)).map_err(|e| io(&meta_path, e))?;
    let plan_path = out.join("revert-plan.json");
    std::fs::write(&plan_path, pretty(&plan.to_json())).map_err(|e| io(&plan_path, e))?;
    let restart_seed = made
        .blocks
        .last()
        .and_then(|b| hex::decode(&b.id).ok())
        .and_then(|id| id.get(8..16).and_then(|s| <[u8; 8]>::try_from(s).ok()))
        .map_or(0, u64::from_be_bytes);
    let restarts = RestartPlan::generate(
        PlanKind::Rounds,
        restart_seed,
        tip,
        round_length,
        &made.restarts,
    );
    corpus::write_restart_plan(out, &restarts).map_err(corpus_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_revert_to_a_round_end_reverts_one_block_more_and_the_recipe_joins_the_plan() {
        let landing = |p: &RevertPlan| {
            p.steps
                .iter()
                .any(|s| (s.after_height - s.blocks) % 53 == 0)
        };
        let seed = (0..1000)
            .find(|s| landing(&RevertPlan::generate(*s, 265, 53)))
            .expect("a seed whose plan returns to a round end");
        let generated = RevertPlan::generate(seed, 265, 53);
        let recipe = [RevertStep {
            after_height: 60,
            blocks: 7,
        }];
        let (plan, adjusted) = revert_plan(seed, 265, 53, &recipe);
        assert!(!adjusted.is_empty());
        assert_eq!(plan.steps.len(), generated.steps.len() + 1);
        assert!(
            plan.steps.contains(&recipe[0]),
            "the recipe's revert is kept as it is"
        );
        let generated_only: Vec<_> = plan.steps.iter().filter(|s| **s != recipe[0]).collect();
        assert!(
            !generated_only
                .iter()
                .any(|s| (s.after_height - s.blocks) % 53 == 0)
        );
        assert!(
            plan.steps
                .windows(2)
                .all(|w| w[0].after_height <= w[1].after_height)
        );
        for a in &adjusted {
            assert_eq!(a["now"].as_u64(), a["blocks"].as_u64().map(|b| b + 1));
        }
    }
}
