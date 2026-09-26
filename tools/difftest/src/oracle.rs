//! Running the reference implementation as the oracle: in the oracle container or locally.
//!
//! The oracle scripts live in `tools/oracle/` (gen-vectors.js for L0, oracle-server.js for L1).
//! In the container they are at `/opt/oracle` and the reference at `/opt/solar`; the container
//! always runs with `--network none`, because the reference runs on Node 18, which is past its end
//! of life. Locally they run with Node 18.20.8 against a built reference checkout (`--solar-dir`).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

use crate::errors::{Error, Result};
use crate::vectors::ErrorRecord;

/// Script paths inside the oracle image.
const IMAGE_SCRIPTS: &str = "/opt/oracle";
/// Where a container run sees the directory of the devnet generator (mounted read-only).
const IMAGE_DEVNET_GENERATOR: &str = "/opt/devnet-generator";
/// Where a container run of the chain judge sees the corpus (mounted read-only).
const IMAGE_CORPUS: &str = "/corpus";

/// Options of a golden-vector run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenOptions {
    /// Run gen-vectors.js again under the JavaScript bcrypto backend and require equal records.
    pub backend_check: bool,
    /// The devnet generator (`gen-network.js`), which the genesis class runs: a separate tool of the
    /// maintainers, not published. Its directory, with the generator's `lib/`, is mounted
    /// read-only in a container run. Without it the genesis class is not written.
    pub devnet_generator: Option<PathBuf>,
}

/// Where the reference runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// In the oracle image (`docker run --network none`).
    Container {
        /// The image tag or id.
        image: String,
    },
    /// On this machine, against a built reference checkout.
    Local {
        /// The checkout.
        solar_dir: PathBuf,
    },
}

/// How to start oracle scripts.
#[derive(Debug, Clone)]
pub struct Oracle {
    /// Where the reference runs.
    pub mode: Mode,
    /// The docker command (program and leading arguments), for container mode.
    pub docker: Vec<String>,
    /// The Node binary, for local mode.
    pub node: String,
    /// The oracle scripts directory, for local mode.
    pub scripts: PathBuf,
}

impl Oracle {
    /// The scripts directory of this repository (`tools/oracle`).
    pub fn default_scripts() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../oracle")
    }

    /// An oracle in `mode` with the default docker command, `node` from `PATH` and this
    /// repository's scripts.
    pub fn new(mode: Mode) -> Oracle {
        let scripts = Oracle::default_scripts();
        let wrapper = scripts.join("docker.sh");
        let docker = if wrapper.exists() {
            wrapper.to_string_lossy().into_owned()
        } else {
            "docker".to_string()
        };
        Oracle {
            mode,
            docker: vec![docker],
            node: "node".to_string(),
            scripts,
        }
    }

    fn docker_command(&self) -> Command {
        let (program, args) = match self.docker.split_first() {
            Some((program, args)) => (program.as_str(), args),
            None => ("docker", &[][..]),
        };
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    }

    /// The image id (`docker image inspect`), recorded in every vector file.
    pub fn image_id(&self) -> Result<Option<String>> {
        let Mode::Container { image } = &self.mode else {
            return Ok(None);
        };
        let out = self
            .docker_command()
            .args(["image", "inspect", "--format", "{{.Id}}", image])
            .output()
            .map_err(|e| Error::Oracle(format!("cannot run {:?}: {e}", self.docker)))?;
        if !out.status.success() {
            return Err(Error::Oracle(format!(
                "image {image} not found: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    /// The command that runs `script` with `args`. In container mode `out` is mounted at `/out`
    /// and the process runs with the user and group ids of `out`, so the files it writes belong
    /// to the caller; each of `read_only` (host directory, container path) is mounted read-only.
    fn script_command(
        &self,
        script: &str,
        args: &[String],
        out: Option<&Path>,
        image_id: Option<&str>,
        interactive: bool,
        read_only: &[(&Path, &str)],
    ) -> Result<Command> {
        match &self.mode {
            Mode::Container { image } => {
                let mut cmd = self.docker_command();
                cmd.args(["run", "--rm", "--network", "none"]);
                if interactive {
                    cmd.arg("-i");
                }
                if let Some(out) = out {
                    let meta = std::fs::metadata(out).map_err(|e| Error::io(out, e))?;
                    let abs = out.canonicalize().map_err(|e| Error::io(out, e))?;
                    cmd.args(["--user", &format!("{}:{}", meta.uid(), meta.gid())]);
                    cmd.args(["-v", &format!("{}:/out", abs.display())]);
                }
                for (host, target) in read_only {
                    let abs = host.canonicalize().map_err(|e| Error::io(host, e))?;
                    cmd.args(["-v", &format!("{}:{target}:ro", abs.display())]);
                }
                if let Some(id) = image_id {
                    cmd.args(["-e", &format!("ORACLE_IMAGE_ID={id}")]);
                }
                cmd.arg(image)
                    .arg("node")
                    .arg(format!("{IMAGE_SCRIPTS}/{script}"));
                cmd.args(args);
                Ok(cmd)
            }
            Mode::Local { solar_dir } => {
                let mut cmd = Command::new(&self.node);
                cmd.arg(self.scripts.join(script));
                cmd.arg("--solar-dir").arg(solar_dir);
                cmd.args(args);
                cmd.env_remove("ORACLE_IMAGE_ID");
                Ok(cmd)
            }
        }
    }

    /// The command that writes the L0 vectors into `out` (created if needed).
    pub fn gen_vectors_command(&self, out: &Path, options: &GenOptions) -> Result<Command> {
        std::fs::create_dir_all(out).map_err(|e| Error::io(out, e))?;
        let image_id = self.image_id()?;
        let container = matches!(self.mode, Mode::Container { .. });
        let target = if container {
            "/out".to_string()
        } else {
            out.to_string_lossy().into_owned()
        };
        let mut args = vec!["--out".to_string(), target];
        if !options.backend_check {
            args.push("--no-backend-check".to_string());
        }
        let generator = match &options.devnet_generator {
            Some(path) => Some(path.canonicalize().map_err(|e| Error::io(path, e))?),
            None => None,
        };
        let mut read_only = Vec::new();
        if let Some(generator) = &generator {
            let (Some(dir), Some(file)) = (generator.parent(), generator.file_name()) else {
                return Err(Error::Oracle(format!(
                    "{} is not a file in a directory",
                    generator.display()
                )));
            };
            let path = if container {
                read_only.push((dir, IMAGE_DEVNET_GENERATOR));
                format!("{IMAGE_DEVNET_GENERATOR}/{}", file.to_string_lossy())
            } else {
                generator.to_string_lossy().into_owned()
            };
            args.extend(["--devnet-generator".to_string(), path]);
        }
        self.script_command(
            "gen-vectors.js",
            &args,
            Some(out),
            image_id.as_deref(),
            false,
            &read_only,
        )
    }

    /// Run gen-vectors.js; its progress lines go to this process's output.
    pub fn gen_vectors(&self, out: &Path, options: &GenOptions) -> Result<()> {
        let mut cmd = self.gen_vectors_command(out, options)?;
        let status = cmd
            .status()
            .map_err(|e| Error::Oracle(format!("cannot start gen-vectors.js: {e}")))?;
        if !status.success() {
            return Err(Error::Oracle(format!("gen-vectors.js failed ({status})")));
        }
        Ok(())
    }

    /// Run the chain judge, `replay-chain.js`, on the corpus in `corpus`: it writes
    /// `ts-digests.jsonl` and `anchors.json` into `out` (created if needed). `extra` are further
    /// arguments (`--dump H,...`). In a container run the corpus is mounted read-only. Returns
    /// whether the judge's anchors hold; an error when the judge could not run.
    pub fn judge(&self, corpus: &Path, out: &Path, extra: &[String]) -> Result<bool> {
        std::fs::create_dir_all(out).map_err(|e| Error::io(out, e))?;
        let container = matches!(self.mode, Mode::Container { .. });
        let (corpus_arg, out_dir) = if container {
            (IMAGE_CORPUS.to_string(), "/out".to_string())
        } else {
            (
                corpus.to_string_lossy().into_owned(),
                out.to_string_lossy().into_owned(),
            )
        };
        let mut args = vec![
            "--corpus".to_string(),
            corpus_arg,
            "--out".to_string(),
            format!("{out_dir}/ts-digests.jsonl"),
            "--anchors".to_string(),
            format!("{out_dir}/anchors.json"),
        ];
        args.extend(extra.iter().cloned());
        let image_id = self.image_id()?;
        let read_only: Vec<(&Path, &str)> = if container {
            vec![(corpus, IMAGE_CORPUS)]
        } else {
            Vec::new()
        };
        let mut cmd = self.script_command(
            "replay-chain.js",
            &args,
            Some(out),
            image_id.as_deref(),
            false,
            &read_only,
        )?;
        let status = cmd
            .status()
            .map_err(|e| Error::Oracle(format!("cannot start replay-chain.js: {e}")))?;
        match status.code() {
            Some(0) => Ok(true),
            Some(1) if out.join("anchors.json").exists() => Ok(false),
            _ => Err(Error::Oracle(format!("replay-chain.js failed ({status})"))),
        }
    }

    /// Start oracle-server.js (L1).
    pub fn server(&self) -> Result<OracleServer> {
        let mut cmd = self.script_command("oracle-server.js", &[], None, None, true, &[])?;
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::Oracle(format!("cannot start oracle-server.js: {e}")))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new);
        match (stdin, stdout) {
            (Some(stdin), Some(stdout)) => Ok(OracleServer {
                child,
                stdin: Some(stdin),
                stdout,
                next_id: 1,
            }),
            _ => Err(Error::Oracle("oracle-server.js has no stdio pipes".into())),
        }
    }
}

/// A running oracle-server.js: one JSON request per line, one response per line.
pub struct OracleServer {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl OracleServer {
    fn exchange(&mut self, mut request: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        request["id"] = json!(id);
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| Error::Oracle("oracle-server.js is closed".into()))?;
        writeln!(stdin, "{request}")
            .and_then(|_| stdin.flush())
            .map_err(|e| Error::Oracle(format!("oracle-server.js stdin: {e}")))?;
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| Error::Oracle(format!("oracle-server.js stdout: {e}")))?;
        if n == 0 {
            return Err(Error::Oracle("oracle-server.js exited".into()));
        }
        let response: Value = serde_json::from_str(&line)
            .map_err(|e| Error::Oracle(format!("oracle-server.js sent no JSON ({e}): {line}")))?;
        if response.get("id") != Some(&json!(id)) {
            return Err(Error::Oracle(format!(
                "response for another request: {line}"
            )));
        }
        Ok(response)
    }

    /// Run one operation. `Ok(Err(..))` is the reference rejecting the input.
    pub fn call(
        &mut self,
        op: &str,
        network: &str,
        height: u64,
        args: &Value,
    ) -> Result<std::result::Result<Value, ErrorRecord>> {
        let response =
            self.exchange(json!({"op": op, "network": network, "height": height, "args": args}))?;
        if response.get("ok") == Some(&Value::Bool(true)) {
            return Ok(Ok(response.get("result").cloned().unwrap_or(Value::Null)));
        }
        let error: ErrorRecord = serde_json::from_value(
            response.get("error").cloned().unwrap_or(Value::Null),
        )
        .map_err(|e| Error::Oracle(format!("malformed error response ({e}): {response}")))?;
        if error.class == "RequestError" {
            return Err(Error::Oracle(format!("request refused: {}", error.message)));
        }
        Ok(Err(error))
    }

    /// Check that the oracle answers; returns what it reports about itself.
    pub fn ping(&mut self) -> Result<Value> {
        let response = self.exchange(json!({"op": "ping"}))?;
        match response.get("result") {
            Some(result) if response.get("ok") == Some(&Value::Bool(true)) => Ok(result.clone()),
            _ => Err(Error::Oracle(format!("ping failed: {response}"))),
        }
    }
}

impl Drop for OracleServer {
    fn drop(&mut self) {
        // Closing stdin ends the server's input; it exits, and the container is removed (--rm).
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::testutil::TestDir;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn container_runs_have_no_network_and_write_as_the_caller() {
        let dir = TestDir::new("oracle-cmd");
        let mut oracle = Oracle::new(Mode::Container {
            image: "heartwood-oracle:test".into(),
        });
        oracle.docker = vec!["docker".into()];
        let cmd = oracle
            .script_command(
                "gen-vectors.js",
                &["--out".into(), "/out".into()],
                Some(dir.path()),
                Some("sha256:ab"),
                false,
                &[],
            )
            .unwrap();
        let a = args(&cmd);
        assert_eq!(&a[..4], ["run", "--rm", "--network", "none"]);
        let meta = std::fs::metadata(dir.path()).unwrap();
        assert!(a.contains(&format!("{}:{}", meta.uid(), meta.gid())));
        assert!(a.contains(&"ORACLE_IMAGE_ID=sha256:ab".to_string()));
        assert_eq!(
            &a[a.len() - 5..],
            [
                "heartwood-oracle:test",
                "node",
                "/opt/oracle/gen-vectors.js",
                "--out",
                "/out"
            ]
        );
    }

    #[test]
    fn the_devnet_generator_is_mounted_read_only_in_the_container() {
        let out = TestDir::new("oracle-gen-out");
        let tools = TestDir::new("oracle-gen-tools");
        let generator = tools.path().join("gen-network.js");
        std::fs::write(&generator, "").unwrap();
        let options = GenOptions {
            backend_check: true,
            devnet_generator: Some(generator.clone()),
        };

        let mut container = Oracle::new(Mode::Container {
            image: "heartwood-oracle:test".into(),
        });
        container.docker = vec!["true".into()];
        let a = args(&container.gen_vectors_command(out.path(), &options).unwrap());
        let mount = format!(
            "{}:{IMAGE_DEVNET_GENERATOR}:ro",
            tools.path().canonicalize().unwrap().display()
        );
        assert!(a.contains(&mount), "{a:?}");
        assert_eq!(
            &a[a.len() - 2..],
            [
                "--devnet-generator",
                &format!("{IMAGE_DEVNET_GENERATOR}/gen-network.js")
            ]
        );

        let local = Oracle::new(Mode::Local {
            solar_dir: "/some/solar".into(),
        });
        let a = args(&local.gen_vectors_command(out.path(), &options).unwrap());
        let file = generator.canonicalize().unwrap();
        assert_eq!(
            &a[a.len() - 2..],
            ["--devnet-generator", &file.to_string_lossy()]
        );
        assert!(!a.iter().any(|x| x.contains(":ro")), "{a:?}");
    }

    #[test]
    fn local_runs_use_the_given_reference() {
        let oracle = Oracle::new(Mode::Local {
            solar_dir: "/some/solar".into(),
        });
        let cmd = oracle
            .script_command("oracle-server.js", &[], None, None, true, &[])
            .unwrap();
        assert_eq!(cmd.get_program(), "node");
        let a = args(&cmd);
        assert!(a[0].ends_with("oracle-server.js"));
        assert_eq!(&a[1..], ["--solar-dir", "/some/solar"]);
    }

    /// Pings the oracle when a test oracle is configured: DIFFTEST_TEST_SOLAR_DIR (local) or
    /// DIFFTEST_TEST_IMAGE (container). When neither is set, the test pings nothing.
    /// `make test-oracle` sets both.
    #[test]
    fn ping_configured_oracles() {
        let mut modes = Vec::new();
        if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
            modes.push(Mode::Local {
                solar_dir: dir.into(),
            });
        }
        if let Ok(image) = std::env::var("DIFFTEST_TEST_IMAGE") {
            modes.push(Mode::Container { image });
        }
        if modes.is_empty() {
            eprintln!("ping_configured_oracles: no test oracle configured, nothing to ping");
        }
        for mode in modes {
            let mut server = Oracle::new(mode.clone()).server().unwrap();
            let pong = server.ping().unwrap();
            assert_eq!(pong["pong"], json!(true), "{mode:?}");
            assert_eq!(pong["network"]["pubKeyHash"], json!(90), "{mode:?}");
            let keys = server
                .call(
                    "keys.fromPassphrase",
                    "devnet",
                    2,
                    &json!({"passphrase": "delegate one"}),
                )
                .unwrap()
                .unwrap();
            assert_eq!(
                keys["publicKey"],
                json!("02dcb90a589d24e2d7e836ccc299f11d3f015ef86e208418d8b3d6de614a02cccd"),
                "{mode:?}"
            );
            let rejected = server
                .call("tx.fromBytes", "devnet", 2, &json!({"hex": "ff03"}))
                .unwrap()
                .unwrap_err();
            assert_eq!(rejected.class, "InvalidTransactionBytesError", "{mode:?}");
        }
    }
}
