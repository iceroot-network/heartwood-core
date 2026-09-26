//! Command-line parsing for the `difftest` binary. Hand-written: the binary has few commands, and
//! the harness keeps its dependencies to serde, sha2 and thiserror.

use std::path::PathBuf;

use crate::errors::{Error, Result};
use crate::levels::Level;
use crate::oracle::{Mode, Oracle};

pub const USAGE: &str = "\
difftest: Heartwood's differential-test harness

Usage:
  difftest oracle gen --out DIR [--no-backend-check] [--devnet-generator FILE] [ORACLE]
                                  write the L0 golden vectors with the Solar reference; the
                                  genesis class needs the reference's devnet generator,
                                  gen-network.js       [$DIFFTEST_DEVNET_GENERATOR]
  difftest oracle ping [ORACLE]   start oracle-server.js and check that it answers
  difftest vectors verify DIR     check a vector directory: manifest, meta records, record shape
  difftest vectors diff DIR DIR   compare two oracle runs record by record (meta excluded)
  difftest run l0 DIR             compare golden vectors with Heartwood's implementations
  difftest run l1|l2|l3|l4        later levels (see `difftest levels`)
  difftest levels                 the five levels and what each one arrives with

ORACLE, where the Solar TypeScript reference runs (the flags win over the environment):
  --image TAG        in the oracle container                     [$DIFFTEST_IMAGE]
  --solar-dir DIR    locally, against a built reference checkout  [$SOLAR_DIR]
  --docker CMD       docker command            [$DIFFTEST_DOCKER, else tools/oracle/docker.sh]
  --node BIN         Node 18.20.8 for local runs              [$DIFFTEST_NODE, else node]
  --scripts DIR      oracle scripts for local runs            [tools/oracle]

Exit status: 0 success, 1 differences or a failed check, 2 usage error or a level not built yet.";

/// Where the oracle runs, as given on the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OracleArgs {
    pub image: Option<String>,
    pub solar_dir: Option<PathBuf>,
    pub docker: Option<String>,
    pub node: Option<String>,
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
    OracleGen {
        oracle: OracleArgs,
        out: PathBuf,
        backend_check: bool,
        devnet_generator: Option<PathBuf>,
    },
    OraclePing {
        oracle: OracleArgs,
    },
    VectorsVerify {
        dir: PathBuf,
    },
    VectorsDiff {
        a: PathBuf,
        b: PathBuf,
    },
    Run {
        level: Level,
        dir: Option<PathBuf>,
    },
    Levels,
    Help,
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
                (_, []) => Ok(Command::Run { level, dir: None }),
                (_, _) => Err(usage(format!(
                    "difftest run {level} takes no arguments yet"
                ))),
            }
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
            p("run L2").unwrap(),
            Command::Run {
                level: Level::L2,
                dir: None
            }
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
            "run l2 x",
            "run l9",
            "oracle gen --image x",
            "oracle gen --out v --devnet-generator",
            "oracle ping --image",
            "oracle ping --colour red",
            "vectors verify",
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
