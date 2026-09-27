//! Command-line parsing for the `difftest` binary. Hand-written: the binary has few commands, and
//! the harness keeps its dependencies to serde, sha2 and thiserror.

use std::path::PathBuf;

use crate::errors::{Error, Result};
use crate::levels::Level;
use crate::levels::l1::Property;
use crate::oracle::{Mode, Oracle};

/// The usage text of the `difftest` binary.
pub const USAGE: &str = "\
difftest: Heartwood's differential-test harness

Usage:
  difftest oracle gen --out DIR [--no-backend-check] [--devnet-generator FILE] [ORACLE]
                                  write the L0 golden vectors with the reference; the
                                  genesis class needs the devnet generator, gen-network.js,
                                  a separate tool      [$DIFFTEST_DEVNET_GENERATOR]
  difftest oracle ping [ORACLE]   start oracle-server.js and check that it answers
  difftest oracle replay OP CORPUS --vectors DIR [--height N] [--milestones FILE] [ORACLE]
                                  send each file of CORPUS (raw bytes, for example a fuzz
                                  corpus) as the hex input of OP (tx.fromBytes,
                                  block.fromBytes) to the oracle and to Heartwood, and
                                  compare them as L0 does; DIR is a vector set with the
                                  genesis class, for the devnet rules; --milestones runs
                                  both sides under that milestone file; height 2 by default
  difftest vectors verify DIR     check a vector directory: manifest, meta records, record shape
  difftest vectors diff DIR DIR   compare two oracle runs record by record (meta excluded)
  difftest run l0 DIR             compare golden vectors with Heartwood's implementations
  difftest run l1 [--property P]... [--cases N] [--seed S] [--vectors DIR]
                  [--regressions DIR] [ORACLE]
                                  random and mutated cases of the properties (P1 to P9, or
                                  their names; all by default), one oracle call each; 256
                                  cases and seed 1 by default; DIR is a vector set with the
                                  genesis class [packages/crypto/tests/vectors]; the minimal
                                  case of a failed property is written to --regressions
  difftest run l2 DIR [--synthetic] [--restart] [--memory] [--to H] [ORACLE]
                                  replay the corpus DIR in Heartwood through a new store and
                                  compare every step with the oracle's digest lines, then the
                                  stored rows with the node's; with an oracle, the first
                                  differing step is dumped on both sides; --synthetic also
                                  runs the synthetic cases on top of the chain (needs one);
                                  --restart also replays it with the restarts of its restart
                                  plan; --memory replays without a store; --to H replays the
                                  chain cut at H, as the judge cut it
  difftest run l3|l4              later levels (see `difftest levels`)
  difftest corpus import ARCHIVE... --out DIR [--seed TEXT]
                                  join the devnet tooling's archives of one chain (in order)
                                  into a corpus; TEXT is the chain's public key-derivation
                                  seed, checked against the genesis block [the devnet
                                  tooling's default seed]
  difftest corpus judge DIR [--restarts every|rounds] [--restart-plan FILE] [--to H]
                            [--dump H,...] [ORACLE]
                                  run the oracle's judge on the corpus: its digest lines and
                                  anchor results are added to the corpus; --restarts writes a
                                  restart plan of that kind into the corpus first (every
                                  height, or the round ends, the first blocks of rounds and 20
                                  seeded heights), --restart-plan copies one in; with a restart
                                  plan the judge also writes the reference's restart lines;
                                  --to H judges the chain cut at H
  difftest store replay DIR --db PATH [--to H]
                                  continue replaying the corpus DIR through the store under
                                  PATH, one block per chunk, printing each committed height
  difftest store check DIR --db PATH
                                  restore the store under PATH by the table and the replay
                                  path and print each mode's tip and restart line
  difftest store crash DIR [--kills N] [--seed S] [--max-delay MS]
                                  the kill test: replay DIR through a store in a child process
                                  killed with SIGKILL N times (200) after up to MS ms (1500),
                                  and check each restore
  difftest corpus verify DIR      check a corpus against its manifest
  difftest levels                 the five levels and what each one arrives with
  difftest genesis --profile FILE --seed TEXT --epoch TIME --out DIR [--force] [--compare DIR]
                                  write the devnet chain definition of a genesis profile for a
                                  seed and an epoch (a whole second, 2026-01-01T00:00:00.000Z);
                                  --compare checks it against the output of the devnet
                                  generator for the same inputs

ORACLE, where the reference runs (the flags win over the environment):
  --image TAG        in the oracle container                     [$DIFFTEST_IMAGE]
  --solar-dir DIR    locally, against a built reference checkout  [$SOLAR_DIR]
  --docker CMD       docker command            [$DIFFTEST_DOCKER, else tools/oracle/docker.sh]
  --node BIN         Node 18.20.8 for local runs              [$DIFFTEST_NODE, else node]
  --scripts DIR      oracle scripts for local runs            [tools/oracle]

Exit status: 0 success, 1 differences or a failed check, 2 usage error or a level not built yet.";

/// Where the oracle runs, as given on the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OracleArgs {
    /// `--image TAG`: run in the oracle container.
    pub image: Option<String>,
    /// `--solar-dir DIR`: run locally against a built checkout of the reference.
    pub solar_dir: Option<PathBuf>,
    /// `--docker CMD`: the docker command.
    pub docker: Option<String>,
    /// `--node BIN`: the Node binary of local runs.
    pub node: Option<String>,
    /// `--scripts DIR`: the oracle scripts of local runs.
    pub scripts: Option<PathBuf>,
}

impl OracleArgs {
    /// Build the oracle from the flags, then the environment (`env` looks a variable up).
    pub fn resolve(&self, env: impl Fn(&str) -> Option<String>) -> Result<Oracle> {
        let mode = match (&self.image, &self.solar_dir) {
            (Some(_), Some(_)) => {
                return Err(Error::Usage("give --image or --solar-dir, not both".into()));
            }
            (Some(image), None) => Mode::Container {
                image: image.clone(),
            },
            (None, Some(dir)) => Mode::Local {
                solar_dir: dir.clone(),
            },
            (None, None) => match (env("DIFFTEST_IMAGE"), env("SOLAR_DIR")) {
                (Some(image), _) if !image.is_empty() => Mode::Container { image },
                (_, Some(dir)) if !dir.is_empty() => Mode::Local {
                    solar_dir: dir.into(),
                },
                _ => {
                    return Err(Error::Usage(
                        "no oracle: pass --image TAG or --solar-dir DIR (or set DIFFTEST_IMAGE or SOLAR_DIR)".into(),
                    ));
                }
            },
        };
        let mut oracle = Oracle::new(mode);
        if let Some(docker) = self.docker.clone().or_else(|| env("DIFFTEST_DOCKER")) {
            let words: Vec<String> = docker.split_whitespace().map(str::to_string).collect();
            if words.is_empty() {
                return Err(Error::Usage("the docker command is empty".into()));
            }
            oracle.docker = words;
        }
        if let Some(node) = self.node.clone().or_else(|| env("DIFFTEST_NODE")) {
            oracle.node = node;
        }
        if let Some(scripts) = &self.scripts {
            oracle.scripts = scripts.clone();
        }
        Ok(oracle)
    }
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `difftest oracle gen`: write the golden vectors.
    OracleGen {
        /// Where the oracle runs.
        oracle: OracleArgs,
        /// The directory the vectors are written to.
        out: PathBuf,
        /// Run the generator again under the JavaScript bcrypto backend and compare.
        backend_check: bool,
        /// The devnet generator, for the genesis class.
        devnet_generator: Option<PathBuf>,
    },
    /// `difftest oracle ping`: check that the oracle answers.
    OraclePing {
        /// Where the oracle runs.
        oracle: OracleArgs,
    },
    /// `difftest oracle replay`: a corpus through the oracle and Heartwood.
    OracleReplay(ReplayArgs),
    /// `difftest vectors verify`: check a vector directory.
    VectorsVerify {
        /// The vector directory.
        dir: PathBuf,
    },
    /// `difftest vectors diff`: compare two oracle runs record by record.
    VectorsDiff {
        /// The first run.
        a: PathBuf,
        /// The second run.
        b: PathBuf,
    },
    /// `difftest run LEVEL`: run a level.
    Run {
        /// The level.
        level: Level,
        /// The vector directory, for L0.
        dir: Option<PathBuf>,
    },
    /// `difftest levels`: describe the levels.
    Levels,
    /// `difftest help`: print the usage text.
    Help,
    /// `difftest genesis`: write a devnet chain definition.
    Genesis(GenesisArgs),
    /// `difftest run l1`: the properties.
    RunL1(L1Args),
    /// `difftest run l2`: a corpus replay.
    RunL2(L2Args),
    /// `difftest corpus import`: archives to a corpus.
    CorpusImport {
        /// The archives, in chain order.
        archives: Vec<PathBuf>,
        /// The corpus directory to write.
        out: PathBuf,
        /// The chain's public key-derivation seed.
        seed: String,
    },
    /// `difftest corpus judge`: the oracle's replay of a corpus.
    CorpusJudge(JudgeArgs),
    /// `difftest store replay`: a corpus through the store, one block per chunk.
    StoreReplay {
        /// The corpus.
        dir: PathBuf,
        /// The node's data directory.
        db: PathBuf,
        /// The last height to replay.
        to: Option<u32>,
    },
    /// `difftest store check`: the store restored by both modes.
    StoreCheck {
        /// The corpus.
        dir: PathBuf,
        /// The node's data directory.
        db: PathBuf,
    },
    /// `difftest store crash`: the kill test.
    StoreCrash {
        /// The corpus.
        dir: PathBuf,
        /// The number of kills.
        kills: u32,
        /// The seed of the delays.
        seed: u64,
        /// The longest delay before a kill, in milliseconds.
        max_delay_ms: u64,
    },
    /// `difftest corpus verify`: check a corpus's manifest.
    CorpusVerify {
        /// The corpus.
        dir: PathBuf,
    },
}

/// The arguments of `difftest run l1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L1Args {
    /// Where the oracle runs.
    pub oracle: OracleArgs,
    /// The properties, all when empty.
    pub properties: Vec<Property>,
    /// Cases per property.
    pub cases: u32,
    /// The seed of the case generator.
    pub seed: u64,
    /// A vector set with the genesis class, for the devnet configuration.
    pub vectors: Option<PathBuf>,
    /// Where minimal failing cases are written.
    pub regressions: Option<PathBuf>,
}

/// The arguments of `difftest run l2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Args {
    /// Where the oracle runs, when one is given on the command line.
    pub oracle: Option<OracleArgs>,
    /// The corpus.
    pub dir: PathBuf,
    /// Also run the synthetic cases.
    pub synthetic: bool,
    /// Also run the restarts of the corpus's restart plan.
    pub restart: bool,
    /// Replay without a store.
    pub memory: bool,
    /// Replay the chain cut at this height.
    pub to: Option<u32>,
}

/// The arguments of `difftest corpus judge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeArgs {
    /// Where the oracle runs.
    pub oracle: OracleArgs,
    /// The corpus.
    pub dir: PathBuf,
    /// Heights whose records the judge writes out.
    pub dump: Vec<u32>,
    /// The kind of restart plan to write into the corpus first.
    pub restarts: Option<crate::restart::PlanKind>,
    /// A restart plan to copy into the corpus first.
    pub restart_plan: Option<PathBuf>,
    /// Judge the chain cut at this height.
    pub to: Option<u32>,
}

/// The arguments of `difftest oracle replay`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayArgs {
    /// Where the oracle runs.
    pub oracle: OracleArgs,
    /// The operation.
    pub op: String,
    /// The corpus directory.
    pub corpus: PathBuf,
    /// A vector set with the genesis class, for the devnet configuration.
    pub vectors: PathBuf,
    /// The rule height.
    pub height: u64,
    /// A milestone file for both sides.
    pub milestones: Option<PathBuf>,
}

/// The arguments of `difftest genesis`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenesisArgs {
    /// The genesis profile file.
    pub profile: PathBuf,
    /// The seed text.
    pub seed: String,
    /// The epoch.
    pub epoch: String,
    /// The output directory.
    pub out: PathBuf,
    /// Overwrite the files of a directory that is not empty.
    pub force: bool,
    /// A directory written by the devnet generator, to compare with.
    pub compare: Option<PathBuf>,
}

fn usage(message: impl Into<String>) -> Error {
    Error::Usage(message.into())
}

/// Parse the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["help" | "-h" | "--help", ..] => Ok(Command::Help),
        ["levels"] => Ok(Command::Levels),
        ["vectors", "verify", dir] => Ok(Command::VectorsVerify { dir: dir.into() }),
        ["vectors", "diff", a, b] => Ok(Command::VectorsDiff {
            a: a.into(),
            b: b.into(),
        }),
        ["run", level, rest @ ..] => {
            let level: Level = level.parse()?;
            match (level, rest) {
                (Level::L0, [dir]) => Ok(Command::Run {
                    level,
                    dir: Some(dir.into()),
                }),
                (Level::L0, _) => Err(usage("usage: difftest run l0 DIR")),
                (Level::L1, rest) => parse_l1(rest),
                (Level::L2, rest) => parse_l2(rest),
                (_, []) => Ok(Command::Run { level, dir: None }),
                (_, _) => Err(usage(format!(
                    "difftest run {level} takes no arguments yet"
                ))),
            }
        }
        ["corpus", "import", rest @ ..] => {
            let (mut archives, mut out, mut seed) =
                (Vec::new(), None, crate::synthetic::DEVNET_SEED.to_string());
            let mut it = rest.iter();
            while let Some(&arg) = it.next() {
                match arg {
                    "--out" => out = Some(PathBuf::from(value(arg, it.next())?)),
                    "--seed" => seed = value(arg, it.next())?.to_string(),
                    flag if flag.starts_with("--") => {
                        return Err(usage(format!("unknown option {flag}")));
                    }
                    archive => archives.push(PathBuf::from(archive)),
                }
            }
            if archives.is_empty() {
                return Err(usage("corpus import needs at least one archive"));
            }
            Ok(Command::CorpusImport {
                archives,
                out: out.ok_or_else(|| usage("corpus import needs --out DIR"))?,
                seed,
            })
        }
        ["corpus", "verify", dir] => Ok(Command::CorpusVerify { dir: dir.into() }),
        ["corpus", "judge", dir, rest @ ..] => {
            let mut judge = JudgeArgs {
                oracle: OracleArgs::default(),
                dir: dir.into(),
                dump: Vec::new(),
                restarts: None,
                restart_plan: None,
                to: None,
            };
            let mut it = rest.iter();
            while let Some(&flag) = it.next() {
                match flag {
                    "--dump" => judge.dump = heights(value(flag, it.next())?)?,
                    "--restarts" => judge.restarts = Some(value(flag, it.next())?.parse()?),
                    "--restart-plan" => {
                        judge.restart_plan = Some(PathBuf::from(value(flag, it.next())?));
                    }
                    "--to" => judge.to = Some(height(flag, it.next())?),
                    _ => oracle_flag(&mut judge.oracle, flag, it.next())?,
                }
            }
            if judge.restarts.is_some() && judge.restart_plan.is_some() {
                return Err(usage("give --restarts or --restart-plan, not both"));
            }
            Ok(Command::CorpusJudge(judge))
        }
        ["store", "replay", dir, rest @ ..] => {
            let (mut db, mut to) = (None, None);
            let mut it = rest.iter();
            while let Some(&flag) = it.next() {
                match flag {
                    "--db" => db = Some(PathBuf::from(value(flag, it.next())?)),
                    "--to" => to = Some(height(flag, it.next())?),
                    _ => return Err(usage(format!("unknown option {flag}"))),
                }
            }
            Ok(Command::StoreReplay {
                dir: dir.into(),
                db: db.ok_or_else(|| usage("store replay needs --db PATH"))?,
                to,
            })
        }
        ["store", "check", dir, "--db", db] => Ok(Command::StoreCheck {
            dir: dir.into(),
            db: db.into(),
        }),
        ["store", "crash", dir, rest @ ..] => {
            let (mut kills, mut seed, mut max_delay_ms) = (200, 1, 1_500);
            let mut it = rest.iter();
            while let Some(&flag) = it.next() {
                let number = |v: &str| {
                    v.parse::<u64>()
                        .map_err(|_| usage(format!("{flag} needs an unsigned integer")))
                };
                match flag {
                    "--kills" => {
                        kills = u32::try_from(number(value(flag, it.next())?)?)
                            .ok()
                            .filter(|k| *k >= 1)
                            .ok_or_else(|| usage("--kills needs a positive integer"))?;
                    }
                    "--seed" => seed = number(value(flag, it.next())?)?,
                    "--max-delay" => {
                        max_delay_ms = number(value(flag, it.next())?)?.max(1);
                    }
                    _ => return Err(usage(format!("unknown option {flag}"))),
                }
            }
            Ok(Command::StoreCrash {
                dir: dir.into(),
                kills,
                seed,
                max_delay_ms,
            })
        }
        ["oracle", "gen", rest @ ..] => {
            let mut oracle = OracleArgs::default();
            let mut out = None;
            let mut backend_check = true;
            let mut devnet_generator = None;
            let mut it = rest.iter();
            while let Some(&flag) = it.next() {
                if flag == "--no-backend-check" {
                    backend_check = false;
                } else if flag == "--out" {
                    out = Some(PathBuf::from(value(flag, it.next())?));
                } else if flag == "--devnet-generator" {
                    devnet_generator = Some(PathBuf::from(value(flag, it.next())?));
                } else {
                    oracle_flag(&mut oracle, flag, it.next())?;
                }
            }
            let out = out.ok_or_else(|| usage("oracle gen needs --out DIR"))?;
            Ok(Command::OracleGen {
                oracle,
                out,
                backend_check,
                devnet_generator,
            })
        }
        ["genesis", rest @ ..] => parse_genesis(rest),
        ["oracle", "replay", op, corpus, rest @ ..] => {
            let mut oracle = OracleArgs::default();
            let (mut vectors, mut milestones, mut height) = (None, None, 2);
            let mut it = rest.iter();
            while let Some(&flag) = it.next() {
                match flag {
                    "--vectors" => vectors = Some(PathBuf::from(value(flag, it.next())?)),
                    "--milestones" => milestones = Some(PathBuf::from(value(flag, it.next())?)),
                    "--height" => {
                        height = value(flag, it.next())?
                            .parse()
                            .ok()
                            .filter(|&h| h >= 1)
                            .ok_or_else(|| usage("--height needs a positive integer"))?;
                    }
                    _ => oracle_flag(&mut oracle, flag, it.next())?,
                }
            }
            Ok(Command::OracleReplay(ReplayArgs {
                oracle,
                op: (*op).to_owned(),
                corpus: corpus.into(),
                vectors: vectors.ok_or_else(|| usage("oracle replay needs --vectors DIR"))?,
                height,
                milestones,
            }))
        }
        ["oracle", "ping", rest @ ..] => {
            let mut oracle = OracleArgs::default();
            let mut it = rest.iter();
            while let Some(&flag) = it.next() {
                oracle_flag(&mut oracle, flag, it.next())?;
            }
            Ok(Command::OraclePing { oracle })
        }
        _ => Err(usage(format!("unknown command: {}", words.join(" ")))),
    }
}

fn height(flag: &str, next: Option<&&str>) -> Result<u32> {
    value(flag, next)?
        .parse::<u32>()
        .ok()
        .filter(|h| *h >= 1)
        .ok_or_else(|| usage(format!("{flag} needs a height")))
}

fn heights(list: &str) -> Result<Vec<u32>> {
    list.split(',')
        .filter(|h| !h.is_empty())
        .map(|h| {
            h.parse::<u32>()
                .ok()
                .filter(|h| *h >= 1)
                .ok_or_else(|| usage(format!("{h} is not a height")))
        })
        .collect()
}

fn parse_l1(args: &[&str]) -> Result<Command> {
    let mut l1 = L1Args {
        oracle: OracleArgs::default(),
        properties: Vec::new(),
        cases: 256,
        seed: 1,
        vectors: None,
        regressions: None,
    };
    let mut it = args.iter();
    while let Some(&flag) = it.next() {
        match flag {
            "--property" => {
                for p in value(flag, it.next())?.split(',') {
                    l1.properties.push(p.parse()?);
                }
            }
            "--cases" => {
                l1.cases = value(flag, it.next())?
                    .parse()
                    .ok()
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| usage("--cases needs a positive integer"))?;
            }
            "--seed" => {
                l1.seed = value(flag, it.next())?
                    .parse()
                    .map_err(|_| usage("--seed needs an unsigned integer"))?;
            }
            "--vectors" => l1.vectors = Some(PathBuf::from(value(flag, it.next())?)),
            "--regressions" => l1.regressions = Some(PathBuf::from(value(flag, it.next())?)),
            _ => oracle_flag(&mut l1.oracle, flag, it.next())?,
        }
    }
    Ok(Command::RunL1(l1))
}

fn parse_l2(args: &[&str]) -> Result<Command> {
    let Some((dir, rest)) = args.split_first() else {
        return Err(usage(
            "usage: difftest run l2 DIR [--synthetic] [--restart] [--memory] [--to H] [ORACLE]",
        ));
    };
    let mut oracle = OracleArgs::default();
    let (mut synthetic, mut restart, mut memory, mut to) = (false, false, false, None);
    let mut it = rest.iter();
    while let Some(&flag) = it.next() {
        match flag {
            "--synthetic" => synthetic = true,
            "--restart" => restart = true,
            "--memory" => memory = true,
            "--to" => to = Some(height(flag, it.next())?),
            _ => oracle_flag(&mut oracle, flag, it.next())?,
        }
    }
    if memory && restart {
        return Err(usage(
            "the restarts need the store: --restart without --memory",
        ));
    }
    let given = oracle != OracleArgs::default();
    Ok(Command::RunL2(L2Args {
        oracle: (given || synthetic).then_some(oracle),
        dir: (*dir).into(),
        synthetic,
        restart,
        memory,
        to,
    }))
}

fn parse_genesis(args: &[&str]) -> Result<Command> {
    let (mut profile, mut seed, mut epoch, mut out) = (None, None, None, None);
    let mut force = false;
    let mut compare = None;
    let mut it = args.iter();
    while let Some(&flag) = it.next() {
        match flag {
            "--force" => force = true,
            "--profile" => profile = Some(PathBuf::from(value(flag, it.next())?)),
            "--seed" => seed = Some(value(flag, it.next())?.to_string()),
            "--epoch" => epoch = Some(value(flag, it.next())?.to_string()),
            "--out" => out = Some(PathBuf::from(value(flag, it.next())?)),
            "--compare" => compare = Some(PathBuf::from(value(flag, it.next())?)),
            _ => return Err(usage(format!("unknown option {flag}"))),
        }
    }
    let need = |what: &str| usage(format!("genesis needs {what}"));
    Ok(Command::Genesis(GenesisArgs {
        profile: profile.ok_or_else(|| need("--profile FILE"))?,
        seed: seed.ok_or_else(|| need("--seed TEXT"))?,
        epoch: epoch.ok_or_else(|| need("--epoch TIME"))?,
        out: out.ok_or_else(|| need("--out DIR"))?,
        force,
        compare,
    }))
}

fn value<'a>(flag: &str, next: Option<&&'a str>) -> Result<&'a str> {
    next.copied()
        .ok_or_else(|| usage(format!("{flag} needs a value")))
}

fn oracle_flag(oracle: &mut OracleArgs, flag: &str, next: Option<&&str>) -> Result<()> {
    let v = value(flag, next)?;
    match flag {
        "--image" => oracle.image = Some(v.to_string()),
        "--solar-dir" => oracle.solar_dir = Some(v.into()),
        "--docker" => oracle.docker = Some(v.to_string()),
        "--node" => oracle.node = Some(v.to_string()),
        "--scripts" => oracle.scripts = Some(v.into()),
        _ => return Err(usage(format!("unknown option {flag}"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> Result<Command> {
        let args: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        parse(&args)
    }

    #[test]
    fn parses_a_replay() {
        assert_eq!(
            p("oracle replay tx.fromBytes corpus --vectors v --milestones m.json --height 3 --image i")
                .unwrap(),
            Command::OracleReplay(ReplayArgs {
                oracle: OracleArgs {
                    image: Some("i".into()),
                    ..OracleArgs::default()
                },
                op: "tx.fromBytes".into(),
                corpus: "corpus".into(),
                vectors: "v".into(),
                height: 3,
                milestones: Some("m.json".into()),
            })
        );
        assert!(p("oracle replay tx.fromBytes corpus").is_err());
        assert!(p("oracle replay tx.fromBytes corpus --vectors v --height 0").is_err());
    }

    #[test]
    fn parses_the_commands() {
        assert_eq!(p("").unwrap(), Command::Help);
        assert_eq!(p("levels").unwrap(), Command::Levels);
        assert_eq!(
            p("run l0 target/vectors").unwrap(),
            Command::Run {
                level: Level::L0,
                dir: Some("target/vectors".into())
            }
        );
        assert_eq!(
            p("run L3").unwrap(),
            Command::Run {
                level: Level::L3,
                dir: None
            }
        );
        assert_eq!(
            p("run l1 --property P7 --property digest --cases 9 --seed 20260926 --solar-dir /s")
                .unwrap(),
            Command::RunL1(L1Args {
                oracle: OracleArgs {
                    solar_dir: Some("/s".into()),
                    ..OracleArgs::default()
                },
                properties: vec![Property::Handlers, Property::Digest],
                cases: 9,
                seed: 20_260_926,
                vectors: None,
                regressions: None,
            })
        );
        assert_eq!(
            p("run l2 corpus/c1").unwrap(),
            Command::RunL2(L2Args {
                oracle: None,
                dir: "corpus/c1".into(),
                synthetic: false,
                restart: false,
                memory: false,
                to: None,
            })
        );
        assert_eq!(
            p("run l2 c --synthetic --image i --restart --to 265").unwrap(),
            Command::RunL2(L2Args {
                oracle: Some(OracleArgs {
                    image: Some("i".into()),
                    ..OracleArgs::default()
                }),
                dir: "c".into(),
                synthetic: true,
                restart: true,
                memory: false,
                to: Some(265),
            })
        );
        assert_eq!(
            p("corpus import a b --out c").unwrap(),
            Command::CorpusImport {
                archives: vec!["a".into(), "b".into()],
                out: "c".into(),
                seed: crate::synthetic::DEVNET_SEED.into()
            }
        );
        assert_eq!(
            p("corpus judge c --dump 5,7 --image i --restarts rounds --to 265").unwrap(),
            Command::CorpusJudge(JudgeArgs {
                oracle: OracleArgs {
                    image: Some("i".into()),
                    ..OracleArgs::default()
                },
                dir: "c".into(),
                dump: vec![5, 7],
                restarts: Some(crate::restart::PlanKind::Rounds),
                restart_plan: None,
                to: Some(265),
            })
        );
        assert_eq!(
            p("store replay c --db d --to 9").unwrap(),
            Command::StoreReplay {
                dir: "c".into(),
                db: "d".into(),
                to: Some(9)
            }
        );
        assert_eq!(
            p("store check c --db d").unwrap(),
            Command::StoreCheck {
                dir: "c".into(),
                db: "d".into()
            }
        );
        assert_eq!(
            p("store crash c --kills 3 --seed 9 --max-delay 50").unwrap(),
            Command::StoreCrash {
                dir: "c".into(),
                kills: 3,
                seed: 9,
                max_delay_ms: 50
            }
        );
        assert_eq!(
            p("corpus verify c").unwrap(),
            Command::CorpusVerify { dir: "c".into() }
        );
        assert_eq!(
            p("oracle gen --image heartwood-oracle:x --out v --no-backend-check").unwrap(),
            Command::OracleGen {
                oracle: OracleArgs {
                    image: Some("heartwood-oracle:x".into()),
                    ..OracleArgs::default()
                },
                out: "v".into(),
                backend_check: false,
                devnet_generator: None,
            }
        );
        assert_eq!(
            p("oracle gen --out v --devnet-generator tools/gen-network.js --solar-dir /s").unwrap(),
            Command::OracleGen {
                oracle: OracleArgs {
                    solar_dir: Some("/s".into()),
                    ..OracleArgs::default()
                },
                out: "v".into(),
                backend_check: true,
                devnet_generator: Some("tools/gen-network.js".into()),
            }
        );
        assert_eq!(
            p("genesis --profile p.json --seed s --epoch 2026-01-01T00:00:00.000Z --out o")
                .unwrap(),
            Command::Genesis(GenesisArgs {
                profile: "p.json".into(),
                seed: "s".into(),
                epoch: "2026-01-01T00:00:00.000Z".into(),
                out: "o".into(),
                force: false,
                compare: None,
            })
        );
        assert_eq!(
            p("genesis --out o --force --compare g --epoch e --seed s --profile p").unwrap(),
            Command::Genesis(GenesisArgs {
                profile: "p".into(),
                seed: "s".into(),
                epoch: "e".into(),
                out: "o".into(),
                force: true,
                compare: Some("g".into()),
            })
        );
        assert_eq!(
            p("vectors diff a b").unwrap(),
            Command::VectorsDiff {
                a: "a".into(),
                b: "b".into()
            }
        );
    }

    #[test]
    fn refuses_bad_command_lines() {
        for line in [
            "run l0",
            "run l2",
            "run l1 --cases 0",
            "run l1 --property P10",
            "corpus import --out o",
            "corpus import a",
            "corpus judge c --dump x",
            "corpus judge c --restarts some",
            "corpus judge c --restarts every --restart-plan p",
            "corpus judge c --to 0",
            "run l2 c --memory --restart",
            "store replay c",
            "store crash c --kills 0",
            "store check c",
            "run l9",
            "oracle gen --image x",
            "oracle gen --out v --devnet-generator",
            "oracle ping --image",
            "oracle ping --colour red",
            "vectors verify",
            "genesis",
            "genesis --profile p --seed s --epoch e",
            "genesis --profile p --seed s --epoch e --out o --colour red",
            "genesis --profile p --seed s --epoch e --out",
            "frobnicate",
        ] {
            assert!(matches!(p(line), Err(Error::Usage(_))), "{line}");
        }
    }

    #[test]
    fn oracle_resolution_prefers_flags_then_image_then_solar_dir() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let flags = OracleArgs {
            solar_dir: Some("/built".into()),
            ..OracleArgs::default()
        };
        let o = flags.resolve(env(&[("DIFFTEST_IMAGE", "img")])).unwrap();
        assert_eq!(
            o.mode,
            Mode::Local {
                solar_dir: "/built".into()
            }
        );

        let none = OracleArgs::default();
        let o = none
            .resolve(env(&[
                ("DIFFTEST_IMAGE", "img"),
                ("SOLAR_DIR", "/s"),
                ("DIFFTEST_DOCKER", "sudo -n docker"),
            ]))
            .unwrap();
        assert_eq!(
            o.mode,
            Mode::Container {
                image: "img".into()
            }
        );
        assert_eq!(o.docker, vec!["sudo", "-n", "docker"]);

        let o = none.resolve(env(&[("SOLAR_DIR", "/s")])).unwrap();
        assert_eq!(
            o.mode,
            Mode::Local {
                solar_dir: "/s".into()
            }
        );

        assert!(none.resolve(env(&[])).is_err());
        let both = OracleArgs {
            image: Some("i".into()),
            solar_dir: Some("/s".into()),
            ..OracleArgs::default()
        };
        assert!(both.resolve(env(&[])).is_err());
    }
}
