//! Run `s`'s status attestations: the reference observer admits both scripted votes for delegates
//! of Heartwood relays, every forged block's slot had a quorum in the relays' network-state
//! records, and txgen's results equal the baseline's.

use super::forging::{self, Slots};
use super::mixed;
use super::run;
use super::steps;
use super::{Cases, MixedRun};
use crate::corpus::Corpus;
use crate::errors::{Error, Result};

const SECTION: &str = "attest";

/// The attestations' checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    mixed::guard(
        cases,
        SECTION,
        "the reference observer's attestations",
        |cases| {
            let m = run.manifest;
            if m.run != "s" {
                cases.record(
                    SECTION,
                    "only run s attests through Heartwood forgers' relays",
                );
                return Ok(());
            }
            let recorded = run::steps(run.run_dir)?;
            // S-18 (a) and (b): the votes for a node-1 and a node-2 delegate through node 4.
            for name in ["vote-s18a", "vote-s18b"] {
                let label = format!("{name}: the reference observer admits the vote");
                match mixed::step(&recorded, name) {
                    Some(s)
                        if s.ok
                            && s.record
                                .get("admitted")
                                .or_else(|| s.record.pointer("/detail/admitted"))
                                .and_then(serde_json::Value::as_bool)
                                != Some(false) =>
                    {
                        cases.pass(SECTION, label);
                    }
                    Some(_) => cases.fail(SECTION, label, "the vote was not admitted"),
                    None => cases.fail(SECTION, label, format!("no {name} step")),
                }
            }
            quorum(run, cases)?;
            txgen(run, cases)
        },
    )
}

/// Every recorded block's slot had an answer counting a quorum.
fn quorum(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let label = "every forged block's slot had a quorum in the relays' network-state records";
    if mixed::heartwood(run.manifest).is_empty() {
        cases.record(
            SECTION,
            "no Heartwood relay in this run (a dry run of the recipe): no network-state record to read",
        );
        return Ok(());
    }
    let chains = mixed::chains(run)?;
    let corpus = Corpus::load(&chains.last().dir)?;
    let params = corpus.milestones.at(1);
    let epoch_ms = heartwood_crypto::crypto::slots::epoch_unix_ms(params.epoch())
        .ok_or_else(|| Error::Usage("the chain's epoch is not a time".into()))?;
    let slots = Slots {
        epoch_ms,
        slot_ms: i64::from(run.manifest.block_time) * 1_000,
    };
    let validators = forging::validators(run.run_dir)?;
    let (blocks, block_problems) =
        forging::recorded_blocks(run.run_dir, run.manifest, &corpus.milestones)?;
    let (answers, _) = forging::answers(run.run_dir, run.manifest)?;
    // Only a Heartwood relay writes block files and network-state records.
    let reference: Vec<String> = mixed::references(run.manifest)
        .iter()
        .map(|n| format!("{}: no block file", n.name))
        .collect();
    let mut problems: Vec<String> = block_problems
        .into_iter()
        .filter(|p| !reference.contains(p))
        .collect();
    if blocks.is_empty() {
        problems.push("no block in the relays' block files".to_owned());
    }
    if answers.is_empty() {
        problems.push("no answer in any relay's network-state record".to_owned());
    }
    if problems.is_empty() {
        problems.extend(forging::quorum_before_blocks(
            &blocks,
            &answers,
            &validators,
            slots,
        ));
    }
    if problems.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "{label}: {} blocks, {} answers",
                blocks.len(),
                answers.len()
            ),
        );
    } else {
        for p in problems.iter().take(6) {
            cases.fail(SECTION, label, p.clone());
        }
        if problems.len() > 6 {
            cases.fail(
                SECTION,
                label,
                format!("... and {} more", problems.len() - 6),
            );
        }
    }
    Ok(())
}

/// Txgen's answers equal the baseline's, submission for submission.
fn txgen(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let label = "txgen's results equal the baseline's";
    let Some(base) = mixed::baseline(run)? else {
        return Ok(());
    };
    let once = |dir: &std::path::Path| dir.join("txgen").join("once");
    let ours = steps::submissions(&once(run.run_dir))?;
    let theirs = steps::submissions(&once(&base.dir))?;
    let compared = steps::compare_submissions(&ours, &theirs);
    if ours.is_empty() {
        cases.fail(SECTION, label, "no submission recorded");
    } else if compared.differences.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "{label}: {} submissions ({} answers differ only in their numbers, {} only in a refusal's reason text)",
                ours.len(),
                compared.timing.len(),
                compared.documented.len()
            ),
        );
    } else {
        for d in compared.differences.iter().take(6) {
            cases.fail(SECTION, label, d.clone());
        }
    }
    Ok(())
}
