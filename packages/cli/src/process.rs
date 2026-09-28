//! The role commands: `start`, `stop`, `restart`, `status` and `log` of one instance.
//!
//! - `start` refuses a running role; it spawns `<program> supervise` (the node's own program)
//!   with an explicit argv, the caller's environment plus `env` and the instance's five
//!   `CORE_PATH_*`, and `/dev/null` as its input. As a daemon the supervisor leaves the caller's
//!   session and `start` returns once it holds its lock; otherwise `start` waits for it.
//! - `stop` refuses a role that does not run; it sends SIGTERM to the supervisor and returns when
//!   the supervisor, and so the node, has exited. A node without a supervisor gets SIGINT, then
//!   SIGKILL after 30 s.
//! - `restart` refuses a role that does not run; it stops it, then starts the command line its
//!   supervisor recorded with the caller's environment, as pm2's `--update-env` does.
//! - `status` gives the process table row: ID, Name, Version, Status, Uptime, Restarts, CPU, RAM.
//!   The state file is trusted only while the supervisor's lock is held; a node whose lock is held
//!   without one is `online (unsupervised)`.
//! - `log` prints the last lines of the output capture (15 by default), or of the error capture,
//!   and follows it when asked.
//!
//! Every pid is signalled only while its lock is held.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use heartwood_kernel::{Clock, SystemClock};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, getpgid};

use crate::errors::ProcessError;
use crate::process_table;
use crate::seams::{Env, Instance, ProcessOps, StatusRow, SupervisedStatus};
use crate::supervisor::signals::{KILL_AFTER, is_locked, send, signal_if_locked};
use crate::supervisor::state::SupervisorState;
use crate::supervisor::{SuperviseArgs, VERSION, at, run_dir};

/// The lines `log` prints by default.
pub const LOG_LINES: usize = 15;

/// How long `start` waits for a daemon supervisor to take its lock.
const START_WAIT: Duration = Duration::from_secs(10);
/// How long `stop` and `kill` wait, beyond the 30 s of a graceful stop, for an exit.
const EXIT_WAIT: Duration = Duration::from_secs(5);
/// How often the commands look at a lock, a process or a file.
const POLL: Duration = Duration::from_millis(50);
/// How often `log` looks for new lines when following.
const FOLLOW: Duration = Duration::from_millis(250);

/// Start `instance` under a supervisor, running `argv` (the node's command line, the program
/// first) with `env`; as a daemon, or in the foreground until it stops.
pub fn start(
    instance: &Instance,
    argv: &[OsString],
    env: &Env,
    daemon: bool,
) -> Result<(), ProcessError> {
    let name = instance.name();
    run_dir(instance)?;
    if is_locked(&instance.supervisor_lock_path())? || is_locked(&instance.lock_path())? {
        return Err(ProcessError::AlreadyRunning { name });
    }
    let program = argv
        .first()
        .map_or_else(|| name.clone(), |p| p.to_string_lossy().into());
    let failed = |message: String| ProcessError::Spawn {
        program: program.clone(),
        message,
    };
    let mut command = Command::new(argv.first().ok_or_else(|| failed("no argv".into()))?);
    command
        .args(SuperviseArgs::command_line(instance, argv, daemon))
        .envs(env.iter().cloned().chain(path_env(instance)))
        .stdin(Stdio::null());
    if daemon {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut supervisor = command.spawn().map_err(|e| failed(e.to_string()))?;
    let lock = instance.supervisor_lock_path();
    // The lock is this start's once the state file names its supervisor: a start beside this one
    // may have taken it first.
    let ours = |id: u32| -> Result<bool, ProcessError> {
        let state = SupervisorState::read(&instance.state_path()).ok().flatten();
        Ok(is_locked(&lock)? && state.is_some_and(|state| state.supervisor_pid == id))
    };
    let deadline = Instant::now() + START_WAIT;
    loop {
        match supervisor.try_wait().map_err(|e| failed(e.to_string()))? {
            Some(exit) if exit.success() && !daemon => return Ok(()),
            Some(_) if daemon && is_locked(&lock)? => {
                return Err(ProcessError::AlreadyRunning { name });
            }
            Some(exit) => return Err(failed(format!("the supervisor exited: {exit}"))),
            None if daemon && ours(supervisor.id())? => return Ok(()),
            None if daemon && Instant::now() >= deadline => {
                let seconds = START_WAIT.as_secs();
                return Err(ProcessError::Timeout { name, seconds });
            }
            None => thread::sleep(POLL),
        }
    }
}

/// Stop `instance` and wait until its node has exited.
pub fn stop(instance: &Instance) -> Result<(), ProcessError> {
    let name = instance.name();
    let row = status(instance)?;
    let (lock, pid, signal) = match row.status {
        SupervisedStatus::OnlineUnsupervised => (instance.lock_path(), row.pid, Signal::SIGINT),
        status if status.is_running() => {
            let pid = read_state(instance)?.supervisor_pid;
            (instance.supervisor_lock_path(), Some(pid), Signal::SIGTERM)
        }
        _ => return Err(ProcessError::NotRunning { name }),
    };
    let reason = "no pid file".into();
    let pid = pid.ok_or_else(|| ProcessError::State {
        path: instance.pid_path(),
        reason,
    })?;
    signal_if_locked(pid, signal, &lock)?;
    if !wait_unlocked(&lock, KILL_AFTER)? {
        // A supervisor kills its node itself; a node without one is killed here.
        if signal == Signal::SIGINT {
            signal_if_locked(pid, Signal::SIGKILL, &lock)?;
        }
        if !wait_unlocked(&lock, EXIT_WAIT)? {
            let seconds = (KILL_AFTER + EXIT_WAIT).as_secs();
            return Err(ProcessError::Timeout { name, seconds });
        }
    }
    Ok(())
}

/// Kill `instance`'s supervisor and node with SIGKILL, as a crash does, wait until they have
/// exited, and record the stop in the state file. A daemon supervisor leads its own process group,
/// which holds its node, so both are killed at once and the node is never restarted; otherwise the
/// node is killed through its own lock.
pub fn kill(instance: &Instance) -> Result<(), ProcessError> {
    let name = instance.name();
    let (supervisor_lock, node_lock) = (instance.supervisor_lock_path(), instance.lock_path());
    let state = SupervisorState::read(&instance.state_path())?;
    let supervised = is_locked(&supervisor_lock)?;
    let seconds = EXIT_WAIT.as_secs();
    let timeout = || ProcessError::Timeout {
        name: name.clone(),
        seconds,
    };
    let mut killed = false;
    if let Some(state) = state.as_ref().filter(|_| supervised) {
        let supervisor = Pid::from_raw(i32::try_from(state.supervisor_pid).unwrap_or(0));
        let sent = if getpgid(Some(supervisor)) == Ok(supervisor) {
            killpg(supervisor, Signal::SIGKILL).is_ok()
        } else {
            send(state.supervisor_pid, Signal::SIGKILL)?
        };
        if sent && !wait_unlocked(&supervisor_lock, EXIT_WAIT)? {
            return Err(timeout());
        }
        killed = sent;
    }
    let node = read_pid(&instance.pid_path()).or(state.as_ref().and_then(|s| s.pid));
    if let Some(pid) = node
        && signal_if_locked(pid, Signal::SIGKILL, &node_lock)?
    {
        if !wait_unlocked(&node_lock, EXIT_WAIT)? {
            return Err(timeout());
        }
        killed = true;
    }
    if !killed {
        return Err(ProcessError::NotRunning { name });
    }
    let Some(mut state) = state else {
        return Ok(());
    };
    state.status = SupervisedStatus::Stopped;
    state.pid = None;
    state.last_exit = Some("signal: 9 (SIGKILL)".into());
    state.write(&instance.state_path())
}

/// Stop `instance`, then start it again with the argv its supervisor ran and `env`.
pub fn restart(instance: &Instance, env: &Env) -> Result<(), ProcessError> {
    if !status(instance)?.status.is_running() {
        let name = instance.name();
        return Err(ProcessError::NotRunning { name });
    }
    let argv = read_state(instance)?.argv;
    let argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    stop(instance)?;
    start(instance, &argv, env, true)
}

/// The process table row of `instance`.
pub fn status(instance: &Instance) -> Result<StatusRow, ProcessError> {
    let state = SupervisorState::read(&instance.state_path())?;
    let supervised = is_locked(&instance.supervisor_lock_path())?;
    let (status, pid) = match &state {
        _ if !supervised && is_locked(&instance.lock_path())? => (
            SupervisedStatus::OnlineUnsupervised,
            read_pid(&instance.pid_path()),
        ),
        Some(state) if supervised => (state.status, state.pid),
        None if supervised => (SupervisedStatus::Launching, None),
        Some(state) if state.status == SupervisedStatus::Errored => (state.status, None),
        _ => (SupervisedStatus::Stopped, None),
    };
    let running = pid.filter(|_| status.is_running());
    let sample = running.and_then(|pid| process_table::sample(pid).ok());
    let started = state.as_ref().and_then(|s| s.started_at_ms);
    // A start time in the future, or one too far in the past to subtract, gives no uptime.
    let uptime = started
        .filter(|_| supervised && running.is_some())
        .and_then(|at| SystemClock.now_ms().checked_sub(at))
        .and_then(|ms| u64::try_from(ms).ok())
        .map(Duration::from_millis);
    Ok(StatusRow {
        pid,
        name: instance.name(),
        version: state.as_ref().map_or(VERSION, |s| &s.version).to_owned(),
        status,
        uptime,
        restarts: state.as_ref().map_or(0, |s| s.restarts),
        cpu_percent: sample.map(|s| s.cpu_percent),
        rss_bytes: sample.map(|s| s.rss_bytes),
    })
}

/// Print the last `lines` lines of `instance`'s log, or of its error capture, and follow it.
pub fn log(
    instance: &Instance,
    lines: usize,
    error: bool,
    follow: bool,
) -> Result<(), ProcessError> {
    let path = match error {
        true => instance.error_log_path(),
        false => instance.out_log_path(),
    };
    if !path.exists() {
        let name = instance.name();
        return Err(ProcessError::NotRunning { name });
    }
    let (text, mut offset) = tail(&path, lines)?;
    let mut out = std::io::stdout().lock();
    let header = format!(
        "Tailing last {lines} lines for [{}] process (change the value with --lines option)",
        instance.name()
    );
    let text = String::from_utf8_lossy(&text);
    at(&path, writeln!(out, "{header}\n{}", text.trim()))?;
    if !follow {
        return Ok(());
    }
    loop {
        thread::sleep(FOLLOW);
        let len = at(&path, fs::metadata(&path))?.len();
        // A rotated capture starts again from its beginning.
        offset = if len < offset { 0 } else { offset };
        let mut file = at(&path, File::open(&path))?;
        let new = read_range(&mut file, &path, offset, len - offset)?;
        at(&path, out.write_all(&new))?;
        offset = len;
    }
}

/// The last `lines` lines of the file at `path` (found in its last `64 KiB × (lines + 1)`), and
/// the file's length.
pub fn tail(path: &Path, lines: usize) -> Result<(Vec<u8>, u64), ProcessError> {
    let mut file = at(path, File::open(path))?;
    let len = at(path, file.metadata())?.len();
    let window = u64::try_from(lines).unwrap_or(u64::MAX).saturating_add(1);
    let window = window.saturating_mul(64 * 1024).min(len);
    let text = read_range(&mut file, path, len - window, window)?;
    let body = text.strip_suffix(b"\n").unwrap_or(&text);
    let newlines = body.iter().enumerate().rev().filter(|(_, b)| **b == b'\n');
    let from = match lines.checked_sub(1) {
        None => text.len(),
        Some(n) => newlines.map(|(i, _)| i + 1).nth(n).unwrap_or(0),
    };
    Ok((text.get(from..).unwrap_or_default().to_vec(), len))
}

/// The process operations over the role commands, running `binary` as the supervisor and the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supervisors {
    /// The `heartwood` binary.
    pub binary: PathBuf,
}

impl Supervisors {
    /// The node's command line: `<binary> <role> run --token <token> --network <network>`.
    pub fn node_argv(&self, instance: &Instance) -> Vec<OsString> {
        let role = instance.role.name();
        let args = [
            role,
            "run",
            "--token",
            &instance.token,
            "--network",
            &instance.network,
        ];
        let mut argv = vec![self.binary.clone().into_os_string()];
        argv.extend(args.map(OsString::from));
        argv
    }
}

impl ProcessOps for Supervisors {
    fn start(&self, instance: &Instance, env: &Env) -> Result<(), ProcessError> {
        start(instance, &self.node_argv(instance), env, true)
    }

    fn stop(&self, instance: &Instance) -> Result<(), ProcessError> {
        stop(instance)
    }

    fn kill(&self, instance: &Instance) -> Result<(), ProcessError> {
        kill(instance)
    }

    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError> {
        status(instance)
    }
}

/// The five `CORE_PATH_*` variables of `instance`'s directories.
pub fn path_env(instance: &Instance) -> Env {
    let paths = &instance.paths;
    let dirs = [
        ("CONFIG", &paths.config),
        ("DATA", &paths.data),
        ("CACHE", &paths.cache),
        ("LOG", &paths.log),
        ("TEMP", &paths.temp),
    ];
    let var = |(kind, dir): (&str, &PathBuf)| (format!("CORE_PATH_{kind}").into(), dir.into());
    dirs.into_iter().map(var).collect()
}

/// `len` bytes of `file` (at `path`) from `start`.
fn read_range(file: &mut File, path: &Path, start: u64, len: u64) -> Result<Vec<u8>, ProcessError> {
    let mut bytes = Vec::new();
    at(path, file.seek(SeekFrom::Start(start)))?;
    at(path, file.take(len).read_to_end(&mut bytes))?;
    Ok(bytes)
}

/// The supervisor's state; a missing file is an error here.
fn read_state(instance: &Instance) -> Result<SupervisorState, ProcessError> {
    let path = instance.state_path();
    let reason = "no state file".into();
    SupervisorState::read(&path)?.ok_or(ProcessError::State { path, reason })
}

/// The pid in the pid file at `path`, if it holds one.
fn read_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Wait until the lock at `path` is released, for at most `timeout`; gives whether it was.
fn wait_unlocked(path: &Path, timeout: Duration) -> Result<bool, ProcessError> {
    let deadline = Instant::now() + timeout;
    while is_locked(path)? {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(POLL);
    }
    Ok(true)
}
