//! The role commands: `start`, `stop`, `restart`, `status` and `log` of one instance.
//!
//! - `start` refuses a running role; it spawns `<program> supervise` (the node's own program)
//!   with an explicit argv, the caller's environment plus `env` and the instance's five
//!   `CORE_PATH_*`, and `/dev/null` as its input. As a daemon the supervisor leaves the caller's
//!   session and `start` returns once it holds its lock and has written its state file (a
//!   supervisor that cannot write it gives a timeout after 10 s); otherwise `start` waits for it.
//! - `stop` refuses a role that does not run; it sends SIGTERM to the supervisor and returns when
//!   the supervisor, and so the node, has exited. A node without a supervisor gets SIGINT, then
//!   SIGKILL after 30 s.
//! - `restart` refuses a role that does not run; it stops it, then starts the command line its
//!   supervisor recorded with the caller's environment, as pm2's `--update-env` does.
//! - `status` gives the process table row: ID, Name, Version, Status, Uptime, Restarts, CPU, RAM.
//!   The state file is trusted only while the supervisor's lock is held; a node whose lock is held
//!   without one is `online (unsupervised)`, or `online (systemd)` when its start recorded a
//!   service manager ([`crate::supervisor::manager`]); `stop` and `restart` refuse such a node
//!   and name the manager's command.
//! - `log` prints the last lines of the output capture (15 by default), or of the error capture,
//!   and follows it when asked; for a node a service manager runs, whose output is in the
//!   manager's journal, it names the journal's command instead.
//!
//! Every pid is signalled only while its lock is held, but for one process group: `kill` ends the
//! group of the node that a supervisor in the foreground, killed a moment before, named last,
//! which may not hold its lock yet ([`kill`]). `kill` refuses a node a service manager runs, as
//! `stop` does: the manager would start it again.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use heartwood_kernel::{Clock, SystemClock};
use nix::sys::signal::Signal;

use crate::errors::ProcessError;
use crate::process_table;
use crate::seams::{Env, Instance, ProcessOps, StatusRow, SupervisedStatus};
use crate::supervisor::manager::ManagerRecord;
use crate::supervisor::signals::{
    KILL_AFTER, held_pid, is_locked, leads_group, leads_session, send, send_group, signal_if_locked,
};
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
        // The supervisor's stderr is a pipe until it has started, so that `start` can say why a
        // supervisor that fails to start did; it then points its stderr at /dev/null.
        command.stdout(Stdio::null()).stderr(Stdio::piped());
    }
    // A state file left by an earlier supervisor may name a pid the system has since reused.
    let earlier = SupervisorState::read(&instance.state_path()).ok().flatten();
    let mut supervisor = command.spawn().map_err(|e| failed(e.to_string()))?;
    let lock = instance.supervisor_lock_path();
    // The lock is this start's once a state file written since the spawn names its supervisor: a
    // start beside this one may have taken it first.
    let ours = |id: u32| -> Result<bool, ProcessError> {
        let state = SupervisorState::read(&instance.state_path()).ok().flatten();
        let fresh = state.filter(|state| Some(state) != earlier.as_ref());
        Ok(is_locked(&lock)? && fresh.is_some_and(|state| state.supervisor_pid == id))
    };
    let deadline = Instant::now() + START_WAIT;
    loop {
        match supervisor.try_wait().map_err(|e| failed(e.to_string()))? {
            Some(exit) if exit.success() && !daemon => return Ok(()),
            Some(_) if daemon && is_locked(&lock)? => {
                return Err(ProcessError::AlreadyRunning { name });
            }
            Some(exit) => {
                let mut reason = String::new();
                if let Some(pipe) = supervisor.stderr.take() {
                    let _ = pipe.take(4096).read_to_string(&mut reason);
                }
                let reason = reason.trim();
                let sep = if reason.is_empty() { "" } else { ": " };
                return Err(failed(format!(
                    "the supervisor exited: {exit}{sep}{reason}"
                )));
            }
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
    refuse_managed(instance, "stop")?;
    let row = status(instance)?;
    let (lock, pid, signal) = match row.status {
        SupervisedStatus::OnlineUnsupervised => (instance.lock_path(), row.pid, Signal::SIGINT),
        status if status.is_running() => {
            let pid = supervisor_pid(instance)?;
            let pid = pid.ok_or_else(|| ProcessError::NotRunning { name: name.clone() })?;
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
/// exited, and record the stop in the state file. The supervisor is killed first, so the node is
/// never restarted. A daemon supervisor leads its session, whose process group holds its node, so
/// the group is killed: that ends a node the supervisor has just started again and that has not
/// taken its lock yet. Any other supervisor is killed alone, then the process group of the node
/// it named last, as it starts each node in a group of its own. Last, the node is killed through
/// its own lock.
pub fn kill(instance: &Instance) -> Result<(), ProcessError> {
    refuse_managed(instance, "stop")?;
    let name = instance.name();
    let (supervisor_lock, node_lock) = (instance.supervisor_lock_path(), instance.lock_path());
    let seconds = EXIT_WAIT.as_secs();
    let timeout = || ProcessError::Timeout {
        name: name.clone(),
        seconds,
    };
    let mut killed = false;
    // The pid of a supervisor in the foreground, killed alone.
    let mut foreground = None;
    if let Some(pid) = supervisor_pid(instance)? {
        let daemon = leads_session(pid);
        let sent = if daemon {
            send_group(pid, Signal::SIGKILL)?
        } else {
            send(pid, Signal::SIGKILL)?
        };
        if sent && !wait_unlocked(&supervisor_lock, EXIT_WAIT)? {
            return Err(timeout());
        }
        killed = sent;
        foreground = (sent && !daemon).then_some(pid);
    }
    // Read once the supervisor has exited, so that it names the last node the supervisor started.
    let state = SupervisorState::read(&instance.state_path())?;
    if let Some(pid) = foreground {
        kill_node_group(state.as_ref(), pid)?;
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

/// SIGKILL the process group of the node that `state` names, when `supervisor`, a supervisor in
/// the foreground killed a moment ago, wrote it: such a supervisor starts each node in a group of
/// its own, and the last one may not hold its lock yet. The group is signalled only while the
/// node's pid still leads one.
fn kill_node_group(state: Option<&SupervisorState>, supervisor: u32) -> Result<(), ProcessError> {
    let named =
        state.filter(|state| state.supervisor_pid == supervisor && state.status.is_running());
    let node = named.and_then(|state| state.pid);
    if let Some(pid) = node.filter(|&pid| pid != supervisor && leads_group(pid)) {
        send_group(pid, Signal::SIGKILL)?;
    }
    Ok(())
}

/// Stop `instance`, then start it again with the argv its supervisor ran and `env`.
pub fn restart(instance: &Instance, env: &Env) -> Result<(), ProcessError> {
    refuse_managed(instance, "restart")?;
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
    let managed = managed_by(instance)?;
    let (status, pid) = match &state {
        _ if managed.is_some() => (
            SupervisedStatus::OnlineSystemd,
            read_pid(&instance.pid_path()),
        ),
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

/// The record of the service manager that runs `instance`: only while its node holds its lock
/// without a supervisor, the record being the node's own ([`ManagerRecord`]), and a stale one
/// naming nothing.
pub(crate) fn managed_by(instance: &Instance) -> Result<Option<ManagerRecord>, ProcessError> {
    let node = !is_locked(&instance.supervisor_lock_path())? && is_locked(&instance.lock_path())?;
    match node {
        true => ManagerRecord::read(&instance.manager_path()),
        false => Ok(None),
    }
}

/// Refuse the command of a node that a service manager runs, naming the manager's `verb`
/// (`stop` or `restart`): the manager would start a stopped node again.
fn refuse_managed(instance: &Instance, verb: &str) -> Result<(), ProcessError> {
    refuse_managed_with(instance, |unit| {
        format!("`systemctl --user {verb} {unit}` (`systemctl {verb} {unit}` for a system unit)")
    })
}

/// Refuse the command of a node that a service manager runs, naming the command `command` gives
/// for its unit.
fn refuse_managed_with(
    instance: &Instance,
    command: impl FnOnce(&str) -> String,
) -> Result<(), ProcessError> {
    let Some(record) = managed_by(instance)? else {
        return Ok(());
    };
    let command = command(&record.unit);
    let (name, manager) = (instance.name(), record.manager.name());
    Err(ProcessError::Managed {
        name,
        manager,
        command,
    })
}

/// Print the last `lines` lines of `instance`'s log, or of its error capture, and follow it.
pub fn log(
    instance: &Instance,
    lines: usize,
    error: bool,
    follow: bool,
) -> Result<(), ProcessError> {
    // A node a service manager runs prints to the manager's journal: a capture there is an
    // earlier start's.
    refuse_managed_with(instance, |unit| {
        format!("`journalctl --user -u {unit}` (`journalctl -u {unit}` for a system unit)")
    })?;
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

/// The pid of the supervisor that holds `instance`'s supervisor lock, as the lock file records
/// it; the state file's, for a supervisor that recorded none (one started by an earlier build).
fn supervisor_pid(instance: &Instance) -> Result<Option<u32>, ProcessError> {
    let lock = instance.supervisor_lock_path();
    if let Some(pid) = held_pid(&lock)? {
        return Ok(Some(pid));
    }
    let state = SupervisorState::read(&instance.state_path())?;
    Ok(state
        .filter(|_| is_locked(&lock).unwrap_or(false))
        .map(|s| s.supervisor_pid))
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
