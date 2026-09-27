//! Restarts during an L2 replay.
//!
//! A restart plan (`restart-plan.json` in a corpus, seeded like the revert plan) names the heights
//! at which the replay restarts, the ways it restarts (the reference's rebuild and saved-state
//! paths; Heartwood's table and replay paths), and the reverts and re-applies that follow a
//! restart at a round end. The judge writes the reference's restart lines
//! (`ts-restarts.jsonl`); the Rust replay closes and opens its store at each planned height,
//! restores by each of Heartwood's paths, and compares its restart line and every later line with
//! the continuous replay's.
//!
//! A replay with restarts walks the continuous replay's steps ([`crate::levels::l2::Action`]) and
//! after the block of each planned height (and the reverts the revert plan runs after it) it
//! restarts; after a restart at a round end it reverts each of the plan's counts of blocks and
//! applies them again. [`steps`] lists these steps. A restart line must equal the continuous line
//! of its height in `b`, `s`, `r` and `w` (it has no missed slots), and every other line the
//! continuous line of the same step (a revert line the forward line of its height in `b`, `s`, `r`
//! and `w`); the running digest `cd` is left out, since a replay with restarts has more lines.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use heartwood_crypto::BlockId;
use serde_json::{Value, json};

use crate::errors::{Error, Result};
use crate::levels::l2::Action;

/// The format string of `restart-plan.json`.
pub const PLAN_FORMAT: &str = "heartwood-restart-plan/1";
/// The restart plan of a corpus.
pub const RESTART_PLAN: &str = "restart-plan.json";
/// The reference's restart lines, written by the judge.
pub const TS_RESTARTS: &str = "ts-restarts.jsonl";
/// The node's live restarts in a corpus, when it has any.
pub const LIVE_RESTARTS: &str = "reference/restarts.jsonl";

/// The reverts after a restart at a round end: one block, and one round and one block.
pub const ROUND_END_REVERTS: [u32; 2] = [1, 54];
/// The seeded heights of a plan of kind [`PlanKind::Rounds`].
const SEEDED_HEIGHTS: u32 = 20;

/// Which heights a plan restarts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    /// Every height from the genesis block to the tip.
    Every,
    /// Every round end, the first block of every round, 20 seeded heights and the heights of the
    /// node's live restarts.
    Rounds,
}

impl PlanKind {
    /// The kind's name in a plan and on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            PlanKind::Every => "every",
            PlanKind::Rounds => "rounds",
        }
    }
}

impl fmt::Display for PlanKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for PlanKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<PlanKind> {
        match s {
            "every" => Ok(PlanKind::Every),
            "rounds" => Ok(PlanKind::Rounds),
            _ => Err(Error::Usage(format!(
                "unknown restart plan kind {s} (every or rounds)"
            ))),
        }
    }
}

/// How the reference's node starts again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReferenceMode {
    /// Without a saved state: `StateBuilder` over the stored chain.
    Rebuild,
    /// From the state `StateSaver` wrote at the tip: `StateLoader`.
    Saved,
}

impl ReferenceMode {
    /// Both modes, in the judge's order.
    pub const ALL: [ReferenceMode; 2] = [ReferenceMode::Rebuild, ReferenceMode::Saved];

    /// The mode's name in the judge's lines.
    pub const fn name(self) -> &'static str {
        match self {
            ReferenceMode::Rebuild => "rebuild",
            ReferenceMode::Saved => "saved",
        }
    }

    fn parse(s: &str) -> Option<ReferenceMode> {
        ReferenceMode::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// How Heartwood's node starts again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HeartwoodMode {
    /// From the state tables.
    Table,
    /// By a replay of the stored chain from the genesis block.
    Replay,
}

impl HeartwoodMode {
    /// Both modes.
    pub const ALL: [HeartwoodMode; 2] = [HeartwoodMode::Table, HeartwoodMode::Replay];

    /// The mode's name in reports.
    pub const fn name(self) -> &'static str {
        match self {
            HeartwoodMode::Table => "table",
            HeartwoodMode::Replay => "replay",
        }
    }

    fn parse(s: &str) -> Option<HeartwoodMode> {
        HeartwoodMode::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// The restarts of a replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPlan {
    /// Which heights the plan was made with.
    pub kind: PlanKind,
    /// The seed the seeded heights were drawn with.
    pub seed: u64,
    /// The round length the round ends were taken from.
    pub round_length: u32,
    /// The last height of the chain the plan was made for.
    pub tip: u32,
    /// The heights of the node's live restarts, which a plan always includes.
    pub live: Vec<u32>,
    /// The restart heights, rising, from 1 to the tip.
    pub heights: Vec<u32>,
    /// The block counts reverted and applied again after a restart at a round end.
    pub reverts: Vec<u32>,
    /// The reference's modes.
    pub reference: Vec<ReferenceMode>,
    /// Heartwood's modes.
    pub heartwood: Vec<HeartwoodMode>,
}

/// SplitMix64, as the revert plan draws its heights.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl RestartPlan {
    /// The plan of `kind` for a chain of `tip` blocks with rounds of `round_length` blocks, from
    /// `seed`, with the node's live restarts at `live`.
    pub fn generate(
        kind: PlanKind,
        seed: u64,
        tip: u32,
        round_length: u32,
        live: &[u32],
    ) -> RestartPlan {
        let mut heights: BTreeSet<u32> = live
            .iter()
            .copied()
            .filter(|h| (1..=tip).contains(h))
            .collect();
        match kind {
            PlanKind::Every => heights.extend(1..=tip),
            PlanKind::Rounds => {
                if round_length > 0 {
                    let mut end = round_length;
                    while end <= tip {
                        heights.insert(end);
                        if end < tip {
                            heights.insert(end + 1);
                        }
                        end = end.saturating_add(round_length);
                        if end == u32::MAX {
                            break;
                        }
                    }
                }
                let mut state = seed;
                if tip >= 2 {
                    for _ in 0..SEEDED_HEIGHTS {
                        let span = u64::from(tip - 1);
                        let pick = splitmix(&mut state) % span;
                        heights.insert(2 + u32::try_from(pick).unwrap_or(0));
                    }
                }
            }
        }
        RestartPlan {
            kind,
            seed,
            round_length,
            tip,
            live: live.to_vec(),
            heights: heights.into_iter().collect(),
            reverts: ROUND_END_REVERTS.to_vec(),
            reference: ReferenceMode::ALL.to_vec(),
            heartwood: HeartwoodMode::ALL.to_vec(),
        }
    }

    /// The seed of a chain's restart plan: bytes 8 to 15 of its tip's id, big-endian (the revert
    /// plan takes bytes 0 to 7).
    pub fn seed_of(tip: &BlockId) -> u64 {
        let bytes = tip.as_bytes();
        let mut seed = [0u8; 8];
        for (to, from) in seed.iter_mut().zip(bytes.iter().skip(8)) {
            *to = *from;
        }
        u64::from_be_bytes(seed)
    }

    /// Whether the plan restarts after the block at `height`.
    pub fn restarts_at(&self, height: u32) -> bool {
        self.heights.binary_search(&height).is_ok()
    }

    /// Whether `height` ends a round.
    pub fn is_round_end(&self, height: u32) -> bool {
        self.round_length > 0 && height.is_multiple_of(self.round_length)
    }

    /// The reverts after a restart at `height`: the plan's counts when `height` ends a round,
    /// those that leave the genesis block in place.
    pub fn reverts_after(&self, height: u32) -> impl Iterator<Item = u32> + '_ {
        let round_end = self.is_round_end(height);
        self.reverts
            .iter()
            .copied()
            .filter(move |n| round_end && *n < height)
    }

    /// The plan as `restart-plan.json` holds it.
    pub fn to_json(&self) -> Value {
        json!({
            "format": PLAN_FORMAT,
            "kind": self.kind.name(),
            "seed": self.seed.to_string(),
            "roundLength": self.round_length,
            "tip": self.tip,
            "live": self.live,
            "heights": self.heights,
            "reverts": self.reverts,
            "modes": {
                "reference": self.reference.iter().map(|m| m.name()).collect::<Vec<_>>(),
                "heartwood": self.heartwood.iter().map(|m| m.name()).collect::<Vec<_>>(),
            },
        })
    }

    /// The plan in `value`, the JSON of the file `path`; [`Error::Corpus`] when it breaks the
    /// plan's format.
    pub fn from_json(value: &Value, path: &Path) -> Result<RestartPlan> {
        RestartPlan::parse(value).map_err(|message| Error::Corpus {
            path: path.to_path_buf(),
            message,
        })
    }

    fn parse(value: &Value) -> std::result::Result<RestartPlan, String> {
        if value.get("format").and_then(Value::as_str) != Some(PLAN_FORMAT) {
            return Err(format!("not a {PLAN_FORMAT} file"));
        }
        let number = |v: &Value, what: &str| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("{what} is not a height"))
        };
        let list = |key: &str| -> std::result::Result<Vec<u32>, String> {
            value
                .get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("no {key}"))?
                .iter()
                .map(|v| number(v, key))
                .collect()
        };
        let kind: PlanKind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("no kind")?
            .parse()
            .map_err(|e: Error| e.to_string())?;
        let seed = value
            .get("seed")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("seed is not a decimal u64")?;
        let tip = number(value.get("tip").unwrap_or(&Value::Null), "tip")?;
        let heights = list("heights")?;
        if heights.windows(2).any(|w| w[0] >= w[1])
            || heights.iter().any(|h| !(1..=tip).contains(h))
        {
            return Err(format!("the heights must rise from 1 to the tip {tip}"));
        }
        let reverts = list("reverts")?;
        if reverts.contains(&0) {
            return Err("a revert of no block".into());
        }
        let modes = |key: &str| -> Vec<String> {
            value
                .pointer(&format!("/modes/{key}"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let reference = modes("reference")
            .iter()
            .map(|m| ReferenceMode::parse(m).ok_or_else(|| format!("unknown reference mode {m}")))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let heartwood = modes("heartwood")
            .iter()
            .map(|m| HeartwoodMode::parse(m).ok_or_else(|| format!("unknown Heartwood mode {m}")))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(RestartPlan {
            kind,
            seed,
            round_length: number(
                value.get("roundLength").unwrap_or(&Value::Null),
                "roundLength",
            )?,
            tip,
            live: list("live")?,
            heights,
            reverts,
            reference,
            heartwood,
        })
    }
}

/// One step of a replay with restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The continuous replay's step, with the index of its line (the genesis line is 0).
    Continuous {
        /// The step.
        action: Action,
        /// The index of the continuous line of the same step.
        line: usize,
    },
    /// A restart at the tip, the block at this height.
    Restart(u32),
    /// A revert after a restart at a round end.
    RevertAfterRestart,
    /// A re-apply of the block at this height after a restart at a round end.
    ReapplyAfterRestart(u32),
}

/// The steps after the genesis step of a replay of the continuous `actions` with the restarts of
/// `plan`: the continuous steps in order, a restart after the block of each planned height and
/// the reverts that follow it, and the reverts and re-applies of a restart at a round end. A
/// restart at height 1 comes first.
pub fn steps(actions: &[Action], plan: &RestartPlan) -> Vec<Step> {
    let mut out = Vec::new();
    let restart = |out: &mut Vec<Step>, height: u32| {
        out.push(Step::Restart(height));
        for count in plan.reverts_after(height) {
            out.extend(std::iter::repeat_n(
                Step::RevertAfterRestart,
                count as usize,
            ));
            out.extend((height - count + 1..=height).map(Step::ReapplyAfterRestart));
        }
    };
    if plan.restarts_at(1) {
        restart(&mut out, 1);
    }
    for (i, action) in actions.iter().enumerate() {
        out.push(Step::Continuous {
            action: *action,
            line: i + 1,
        });
        // A block's restart follows the reverts and re-applies after it: when the next step is
        // another block's first apply, or there is none.
        let tip = match action {
            Action::Apply(h) | Action::Reapply(h) => Some(*h),
            Action::Revert => None,
        };
        let block_done = matches!(actions.get(i + 1), None | Some(Action::Apply(_)));
        if let Some(height) = tip
            && block_done
            && plan.restarts_at(height)
        {
            restart(&mut out, height);
        }
    }
    out
}

/// What the reference's restart gave, as the judge wrote it.
#[derive(Debug, Clone, PartialEq)]
pub enum RestartOutcome {
    /// The start succeeded: its digest line.
    Line(Value),
    /// The start was refused, with the reference's text.
    Refused(String),
}

/// One line of `ts-restarts.jsonl`.
#[derive(Debug, Clone, PartialEq)]
pub struct RestartRecord {
    /// The reference's mode.
    pub mode: ReferenceMode,
    /// The height of the restart.
    pub height: u32,
    /// What the restart gave.
    pub outcome: RestartOutcome,
    /// The oracle's notes (`savedStateUsername`), `Null` when none.
    pub notes: Value,
}

impl RestartRecord {
    /// Whether the oracle noted `name`.
    pub fn noted(&self, name: &str) -> bool {
        self.notes[name] == Value::Bool(true)
    }
}

/// The restart lines of `path` (`ts-restarts.jsonl`).
pub fn read_restarts(path: &Path) -> Result<Vec<RestartRecord>> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let bad = |message: String| Error::Parse {
            path: path.to_path_buf(),
            line: index + 1,
            message,
        };
        let value: Value = serde_json::from_str(line).map_err(|e| bad(e.to_string()))?;
        let mode = value
            .get("mode")
            .and_then(Value::as_str)
            .and_then(ReferenceMode::parse)
            .ok_or_else(|| bad("no reference mode".into()))?;
        let height = value
            .get("h")
            .and_then(Value::as_u64)
            .and_then(|h| u32::try_from(h).ok())
            .ok_or_else(|| bad("no height".into()))?;
        if value.get("step").and_then(Value::as_str) != Some("restart") {
            return Err(bad("not a restart line".into()));
        }
        let notes = value.get("notes").cloned().unwrap_or(Value::Null);
        let outcome = match value.pointer("/refused/reason").and_then(Value::as_str) {
            Some(reason) => RestartOutcome::Refused(reason.to_owned()),
            None => {
                let mut line = value.clone();
                if let Some(o) = line.as_object_mut() {
                    o.shift_remove("mode");
                    o.shift_remove("notes");
                }
                RestartOutcome::Line(line)
            }
        };
        out.push(RestartRecord {
            mode,
            height,
            outcome,
            notes,
        });
    }
    Ok(out)
}

/// A live restart of the node that made a corpus: the height, how the node started again, and the
/// file of the state right after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRestart {
    /// The height.
    pub height: u32,
    /// How the node started again.
    pub path: ReferenceMode,
    /// The file of the state right after it, relative to the corpus.
    pub checkpoint: String,
}

/// The live restarts of the corpus in `dir` (`reference/restarts.jsonl`); none when it has no
/// such file.
pub fn read_live(dir: &Path) -> Result<Vec<LiveRestart>> {
    let path = dir.join(LIVE_RESTARTS);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let bad = |message: &str| Error::Parse {
            path: path.clone(),
            line: index + 1,
            message: message.to_owned(),
        };
        let value: Value = serde_json::from_str(line).map_err(|e| bad(&e.to_string()))?;
        out.push(LiveRestart {
            height: value
                .get("height")
                .and_then(Value::as_u64)
                .and_then(|h| u32::try_from(h).ok())
                .ok_or_else(|| bad("no height"))?,
            path: value
                .get("path")
                .and_then(Value::as_str)
                .and_then(ReferenceMode::parse)
                .ok_or_else(|| bad("path is not saved or rebuild"))?,
            checkpoint: value
                .get("checkpoint")
                .and_then(Value::as_str)
                .filter(|c| c.starts_with("reference/") && !c.contains(".."))
                .ok_or_else(|| bad("no checkpoint file in reference/"))?
                .to_owned(),
        });
    }
    Ok(out)
}

/// The keys of a restart line that a continuous line of its height shares.
pub const SAME_STATE: [&str; 4] = ["b", "s", "r", "w"];
/// The keys every other line of a replay with restarts shares with the continuous line of its
/// step (besides `step`).
pub const SAME_STEP: [&str; 9] = ["h", "id", "rw", "b", "s", "r", "m", "w", "sd"];

/// The keys of `keys` in which `line` and `want` differ.
pub fn differing_keys(line: &Value, want: &Value, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter(|k| line.get(**k) != want.get(**k))
        .map(|k| (*k).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::RevertPlan;
    use crate::levels::l2::plan_actions;

    fn plan(kind: PlanKind, tip: u32) -> RestartPlan {
        RestartPlan::generate(kind, 7, tip, 53, &[177])
    }

    #[test]
    fn plans_are_seeded_and_cover_what_their_kind_names() {
        let every = plan(PlanKind::Every, 265);
        assert_eq!(every.heights, (1..=265).collect::<Vec<_>>());
        let rounds = plan(PlanKind::Rounds, 265);
        assert_eq!(rounds, plan(PlanKind::Rounds, 265));
        assert_ne!(
            rounds.heights,
            RestartPlan::generate(PlanKind::Rounds, 8, 265, 53, &[177]).heights
        );
        for h in [53, 54, 106, 107, 159, 160, 212, 213, 265, 177] {
            assert!(rounds.restarts_at(h), "{h}");
        }
        assert!(!rounds.restarts_at(266));
        assert!(rounds.heights.len() > 10 && rounds.heights.len() <= 10 + 20 + 1);
        assert!(rounds.heights.windows(2).all(|w| w[0] < w[1]));
        assert!(rounds.heights.iter().all(|h| (1..=265).contains(h)));
        let json = rounds.to_json();
        assert_eq!(
            RestartPlan::from_json(&json, Path::new("restart-plan.json")).unwrap(),
            rounds
        );
        let mut bad = json.clone();
        bad["heights"] = json!([5, 3]);
        assert!(RestartPlan::from_json(&bad, Path::new("p")).is_err());
        bad["heights"] = json!([300]);
        assert!(RestartPlan::from_json(&bad, Path::new("p")).is_err());
    }

    #[test]
    fn a_round_end_restart_reverts_one_block_and_a_round() {
        let p = plan(PlanKind::Every, 265);
        assert_eq!(p.reverts_after(53).collect::<Vec<_>>(), vec![1]);
        assert_eq!(p.reverts_after(106).collect::<Vec<_>>(), vec![1, 54]);
        assert_eq!(p.reverts_after(107).count(), 0);
        assert_eq!(
            RestartPlan::seed_of(&BlockId::from_array([3; 32])),
            u64::from_be_bytes([3; 8])
        );
    }

    #[test]
    fn restarts_follow_the_reverts_of_their_block() {
        let revert = RevertPlan {
            seed: 0,
            round_length: 3,
            tip: 4,
            steps: vec![crate::corpus::RevertStep {
                after_height: 3,
                blocks: 1,
            }],
        };
        let actions = plan_actions([2, 3, 4], &revert);
        let mut restarts = RestartPlan::generate(PlanKind::Every, 1, 4, 3, &[]);
        restarts.reverts = vec![2];
        use Step::*;
        let steps = steps(&actions, &restarts);
        assert_eq!(
            steps,
            vec![
                Restart(1),
                Continuous {
                    action: Action::Apply(2),
                    line: 1
                },
                Restart(2),
                Continuous {
                    action: Action::Apply(3),
                    line: 2
                },
                Continuous {
                    action: Action::Revert,
                    line: 3
                },
                Continuous {
                    action: Action::Reapply(3),
                    line: 4
                },
                Restart(3),
                RevertAfterRestart,
                RevertAfterRestart,
                ReapplyAfterRestart(2),
                ReapplyAfterRestart(3),
                Continuous {
                    action: Action::Apply(4),
                    line: 5
                },
                Restart(4),
            ]
        );
    }

    #[test]
    fn reads_restart_lines_and_live_restarts() {
        let dir = crate::vectors::testutil::TestDir::new("restart-lines");
        let path = dir.path().join(TS_RESTARTS);
        std::fs::write(
            &path,
            "{\"mode\":\"rebuild\",\"h\":3,\"step\":\"restart\",\"id\":\"x\",\"b\":\"1\"}\n\
             {\"mode\":\"saved\",\"h\":3,\"step\":\"restart\",\"refused\":{\"reason\":\"no\"},\"notes\":{\"savedStateUsername\":true}}\n",
        )
        .unwrap();
        let records = read_restarts(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].outcome,
            RestartOutcome::Line(json!({"h": 3, "step": "restart", "id": "x", "b": "1"}))
        );
        assert_eq!(records[1].outcome, RestartOutcome::Refused("no".into()));
        assert!(records[1].noted("savedStateUsername") && !records[0].noted("x"));
        std::fs::write(&path, "{\"mode\":\"other\",\"h\":3,\"step\":\"restart\"}\n").unwrap();
        assert!(read_restarts(&path).is_err());

        assert!(read_live(dir.path()).unwrap().is_empty());
        std::fs::create_dir_all(dir.path().join("reference")).unwrap();
        std::fs::write(
            dir.path().join(LIVE_RESTARTS),
            "{\"height\":177,\"path\":\"saved\",\"checkpoint\":\"reference/restarts/h000000177.json\"}\n",
        )
        .unwrap();
        assert_eq!(
            read_live(dir.path()).unwrap(),
            vec![LiveRestart {
                height: 177,
                path: ReferenceMode::Saved,
                checkpoint: "reference/restarts/h000000177.json".into()
            }]
        );
    }

    #[test]
    fn lines_are_compared_by_keys() {
        let a = json!({"h": 1, "b": "x", "m": "1", "cd": "1"});
        let b = json!({"h": 1, "b": "x", "m": "2", "cd": "2"});
        assert!(differing_keys(&a, &b, &SAME_STATE).is_empty());
        assert_eq!(differing_keys(&a, &b, &SAME_STEP), vec!["m".to_string()]);
    }
}
