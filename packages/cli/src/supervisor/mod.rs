//! `heartwood supervise`: the built-in supervisor of one role of one instance, with pm2's
//! semantics.
//!
//! - It holds its lock (`<role>.supervisor.lock`) while it runs and writes its state file
//!   ([`state`]) at every change: `launching` before each start of the node, `online` once it
//!   runs, `stopping`, then `stopped`, or `errored`.
//! - It runs the node with an explicit argv and its own environment (the caller's, with the
//!   instance's five `CORE_PATH_*`, from which it also takes the instance's directories), its
//!   output captured to the `-out.log` and `-error.log` files ([`logs`]); its own lines go to
//!   `-out.log` with the prefix `supervisor:`.
//! - A node that exits without a stop request is restarted at once. A run under 1 s is unstable
//!   while the supervisor is under 5 s old, and at the 5th unstable run the status is `errored`
//!   and the supervisor exits; later short runs are restarted for good, as pm2 does ([`restart`]).
//! - SIGTERM or SIGINT asks it to stop: it sends SIGINT to the node, SIGKILL after 30 s, and exits
//!   when the node has ([`signals`]). SIGHUP is ignored. With `--daemon` it first leaves the
//!   caller's session, so that a closing terminal never reaches it, and leads a session of its
//!   own, whose one process group holds its node: `kill` ends both at once. In the foreground the
//!   node leads a process group of its own instead, so that a terminal's SIGINT or hang-up never
//!   reaches it except through the supervisor.

pub mod logs;
pub mod manager;
pub mod restart;
pub mod signals;
pub mod state;

use std::error::Error;
use std::ffi::{CString, OsString};
use std::fs::{self, DirBuilder, File, Permissions};
use std::io::{BufRead, BufReader, ErrorKind, PipeReader, Read};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use heartwood_kernel::paths::resolve_paths;
use heartwood_kernel::{Clock, SystemClock};
use nix::fcntl::{OFlag, openat, renameat};
use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawnp};
use nix::sys::signal::SigSet;
use nix::sys::signal::Signal::{self, SIGHUP, SIGINT, SIGKILL, SIGPIPE, SIGQUIT, SIGTERM};
use nix::sys::stat::Mode;
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::unistd::{Pid, UnlinkatFlags, unlinkat};

use self::logs::{Capture, ROTATE_AT};
use self::restart::{AfterExit, Restarts};
use self::signals::KILL_AFTER;
use self::state::SupervisorState;
use crate::errors::ProcessError;
use crate::seams::{Instance, Role, SupervisedStatus};

/// The version a Heartwood node reports, as the state file and `status` show it.
pub const VERSION: &str = concat!("4.3.1-heartwood.", env!("CARGO_PKG_VERSION"));

/// How often the supervisor looks at its node and its signals.
const POLL: Duration = Duration::from_millis(50);
/// How long the supervisor waits for the last output of a node that exited.
const DRAIN: Duration = Duration::from_secs(1);
/// The longest piece of a line copied at once from the node's output.
const CHUNK: u64 = 64 * 1024;

/// The arguments of the hidden `heartwood supervise` command, which `start` writes.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct SuperviseArgs {
    /// The role.
    #[arg(long)]
    pub role: Role,
    /// The instance's token.
    #[arg(long)]
    pub token: String,
    /// The instance's network.
    #[arg(long)]
    pub network: String,
    /// Leave the caller's session first.
    #[arg(long)]
    pub daemon: bool,
    /// The node's command line, the program first.
    #[arg(last = true, required = true)]
    pub argv: Vec<OsString>,
}

impl SuperviseArgs {
    /// The command line after the program that supervises `instance`'s node, run with `argv`:
    /// `supervise --role <role> --token <token> --network <network> [--daemon] -- <argv>`.
    pub fn command_line(instance: &Instance, argv: &[OsString], daemon: bool) -> Vec<OsString> {
        let role = instance.role.name();
        let flags = ["--role", role, "--token", &instance.token];
        let mut args: Vec<OsString> = ["supervise"].iter().chain(&flags).map(Into::into).collect();
        args.extend(["--network".into(), instance.network.clone().into()]);
        args.extend(daemon.then(|| "--daemon".into()));
        args.push("--".into());
        args.extend(argv.iter().cloned());
        args
    }

    /// Leave the caller's session when a daemon, then supervise the instance, its directories
    /// taken from the environment; never returns.
    pub fn run(self) -> ! {
        if self.daemon {
            // Fails only for a process group leader, which `start` never spawns.
            let _ = nix::unistd::setsid();
        }
        let env = |name: &str| std::env::var(name).ok();
        let home = std::env::home_dir().unwrap_or_default();
        let cwd = std::env::current_dir().unwrap_or_default();
        let paths = resolve_paths(&self.token, &self.network, &env, &home, &cwd);
        let (token, network, role) = (self.token, self.network, self.role);
        let instance = Instance {
            token,
            network,
            role,
            paths,
        };
        serve(instance, self.argv, self.daemon)
    }
}

/// Supervise `instance`'s node, run with `argv` (the program first), until a stop; never returns.
/// The exit status is 0 after a stop and 1 when the supervisor gave up or could not start.
pub fn supervise(instance: Instance, argv: Vec<OsString>) -> ! {
    serve(instance, argv, false)
}

/// [`supervise`]; a `daemon` points its stderr at `/dev/null` once it has started, as the pipe
/// that carried its start error to `start` is closed then.
fn serve(instance: Instance, argv: Vec<OsString>, daemon: bool) -> ! {
    let code = match Supervisor::start(instance, argv) {
        Ok(mut supervisor) => {
            if daemon && let Ok(null) = File::options().write(true).open("/dev/null") {
                let _ = nix::unistd::dup2_stderr(&null);
            }
            supervisor.run()
        }
        Err(e) => {
            eprintln!("supervisor: {e}");
            1
        }
    };
    std::process::exit(code)
}

/// Create `instance`'s run directory with mode 0700 (its parents with the default mode), or
/// narrow an existing one to 0700; refused when another user owns it. The directory is checked
/// and dropped: what makes files in it should hold it ([`RunDir::open`]).
pub fn run_dir(instance: &Instance) -> Result<PathBuf, ProcessError> {
    RunDir::open(instance).map(|dir| dir.path)
}

/// A run directory held open. The files of a lock, a pid and a state are opened, made, renamed and
/// removed relative to its descriptor, so a directory swapped for a link, or for another user's,
/// after its checks is never where they land. Each method takes a file's path, as [`Instance`]
/// names it, and uses its last component within the directory held.
#[derive(Debug)]
pub struct RunDir {
    dir: File,
    path: PathBuf,
}

impl RunDir {
    /// Make `instance`'s run directory as [`run_dir`] does and hold it: opened without following
    /// a link, and narrowed to 0700 through the descriptor, which succeeds only for the user the
    /// directory belongs to.
    pub fn open(instance: &Instance) -> Result<RunDir, ProcessError> {
        let path = instance.run_dir();
        if let Some(parent) = path.parent() {
            at(parent, fs::create_dir_all(parent))?;
        }
        // Made private as it is made, so it is never there with a wider mode.
        match DirBuilder::new().mode(0o700).create(&path) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            made => at(&path, made)?,
        }
        let held = at(&path, RunDir::hold(&path))?;
        at(
            &path,
            held.dir.set_permissions(Permissions::from_mode(0o700)),
        )?;
        Ok(held)
    }

    /// The directory of the file at `path`, held without following a link.
    pub fn beside(path: &Path) -> Result<RunDir, ProcessError> {
        let dir = directory_of(path);
        at(dir, RunDir::hold(dir))
    }

    /// [`RunDir::beside`]; `None` when the directory is not there.
    pub(crate) fn beside_if_there(path: &Path) -> Result<Option<RunDir>, ProcessError> {
        let dir = directory_of(path);
        match RunDir::hold(dir) {
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            held => at(dir, held).map(Some),
        }
    }

    fn hold(path: &Path) -> std::io::Result<RunDir> {
        let flags = (OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits();
        let dir = File::options().read(true).custom_flags(flags).open(path)?;
        Ok(RunDir {
            dir,
            path: path.to_path_buf(),
        })
    }

    /// The file `path` names, opened in the directory, never through a link, and refused unless it
    /// is a regular file. The open does not wait: a FIFO in its place would block it until its
    /// other end opened.
    fn file(&self, path: &Path, flags: OFlag, mode: u32) -> std::io::Result<File> {
        let flags = flags | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK;
        let name = path.file_name().unwrap_or_default();
        let mode = Mode::from_bits_truncate(mode);
        let file = File::from(openat(&self.dir, name, flags, mode)?);
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        Ok(file)
    }

    /// A new file with `mode`, for writing; refused when there is one.
    pub fn create(&self, path: &Path, mode: u32) -> std::io::Result<File> {
        self.file(path, OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL, mode)
    }

    /// Remove the file.
    pub fn remove(&self, path: &Path) -> std::io::Result<()> {
        let name = path.file_name().unwrap_or_default();
        Ok(unlinkat(&self.dir, name, UnlinkatFlags::NoRemoveDir)?)
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        let names = (from.file_name(), to.file_name());
        let (from, to) = (names.0.unwrap_or_default(), names.1.unwrap_or_default());
        Ok(renameat(&self.dir, from, &self.dir, to)?)
    }

    fn read(&self, path: &Path) -> std::io::Result<String> {
        let mut text = String::new();
        (self.file(path, OFlag::O_RDONLY, 0)?).read_to_string(&mut text)?;
        Ok(text)
    }
}

/// The directory `path` is in: `.` for a name alone.
fn directory_of(path: &Path) -> &Path {
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    parent.unwrap_or(Path::new("."))
}

/// `result`, its error said to be about `path`.
pub(crate) fn at<T>(path: &Path, result: std::io::Result<T>) -> Result<T, ProcessError> {
    result.map_err(|e| ProcessError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// A running supervisor.
struct Supervisor {
    instance: Instance,
    state: SupervisorState,
    out: Arc<Mutex<Capture>>,
    err: Arc<Mutex<Capture>>,
    signals: Receiver<Signal>,
    restarts: Restarts,
    /// When the supervisor started: the restart rule's window runs from it.
    created: Instant,
    /// Whether the node leads a process group of its own: always, unless the supervisor leads its
    /// session (a daemon).
    own_group: bool,
    dir: RunDir,
    _lock: File,
}

impl Supervisor {
    fn start(instance: Instance, argv: Vec<OsString>) -> Result<Supervisor, ProcessError> {
        // Blocked before any thread starts, so every thread inherits the mask and only the
        // signal thread takes them; the node starts with none blocked ([`spawn`]).
        let mask: SigSet = [SIGTERM, SIGINT, SIGHUP].into_iter().collect();
        let pid = std::process::id();
        let blocked = mask.thread_block();
        blocked.map_err(|e| ProcessError::Signal {
            pid,
            message: e.to_string(),
        })?;
        let name = instance.name();
        let dir = RunDir::open(&instance)?;
        let lock = signals::take_lock_in(&dir, &instance.supervisor_lock_path())?;
        let lock = lock.ok_or_else(|| ProcessError::AlreadyRunning { name: name.clone() })?;
        signals::record_pid(&lock, &instance.supervisor_lock_path())?;
        let argv: Option<Vec<String>> = argv.into_iter().map(|a| a.into_string().ok()).collect();
        let argv = argv.ok_or_else(|| ProcessError::Spawn {
            program: name.clone(),
            message: "the command line is not UTF-8".into(),
        })?;
        let out = Capture::open(&instance.out_log_path(), ROTATE_AT)?;
        let err = Capture::open(&instance.error_log_path(), ROTATE_AT)?;
        let (sender, signals) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(signal) = mask.wait() {
                if sender.send(signal).is_err() {
                    break;
                }
            }
        });
        let state = SupervisorState {
            name,
            status: SupervisedStatus::Launching,
            supervisor_pid: pid,
            pid: None,
            argv,
            started_at_ms: None,
            restarts: 0,
            unstable: 0,
            last_exit: None,
            version: VERSION.to_owned(),
        };
        Ok(Supervisor {
            instance,
            state,
            out: Arc::new(Mutex::new(out)),
            err: Arc::new(Mutex::new(err)),
            signals,
            restarts: Restarts::default(),
            created: Instant::now(),
            own_group: !signals::leads_session(pid),
            dir,
            _lock: lock,
        })
    }

    /// Run and restart the node until a stop or the restart rule's end; gives the exit status.
    fn run(&mut self) -> i32 {
        loop {
            let (status, code) = match self.run_once() {
                Ok(AfterExit::Restart) => {
                    self.line("restarting the node");
                    continue;
                }
                Ok(AfterExit::Stop) => (SupervisedStatus::Stopped, 0),
                Ok(AfterExit::GiveUp) => {
                    self.line("the node exited within 1 s five times in its first 5 s");
                    (SupervisedStatus::Errored, 1)
                }
                Err(e) => {
                    self.line(&e.to_string());
                    (SupervisedStatus::Errored, 1)
                }
            };
            self.set(status, None);
            return code;
        }
    }

    /// Run the node once, handling stop requests, and decide what follows its exit.
    fn run_once(&mut self) -> Result<AfterExit, ProcessError> {
        self.set(SupervisedStatus::Launching, None);
        let program = self.state.argv.first().cloned().unwrap_or_default();
        let failed = move |message: String| ProcessError::Spawn {
            program: program.clone(),
            message,
        };
        self.line(&format!("starting {}", self.state.argv.join(" ")));
        let spawned = spawn(&self.state.argv, self.own_group);
        let (node, out, err) = spawned.map_err(|e| failed(e.to_string()))?;
        let started = Instant::now();
        // The moment the node exits, taken by a thread that waits for it without reaping it, so
        // that the run's uptime does not include the poll below.
        let (exits, exited) = mpsc::channel();
        thread::spawn(move || {
            let flags = WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT;
            let _ = waitid(Id::Pid(node), flags);
            let _ = exits.send(Instant::now());
        });
        let pumps = [pump(out, &self.out), pump(err, &self.err)];
        self.state.started_at_ms = Some(SystemClock.now_ms());
        let pid = node.as_raw().unsigned_abs();
        self.set(SupervisedStatus::Online, Some(pid));
        let (mut interrupted, mut killed) = (None::<Instant>, false);
        let exit = loop {
            if let Ok(signal) = self.signals.recv_timeout(POLL)
                && signal != SIGHUP
                && interrupted.is_none()
            {
                self.set(SupervisedStatus::Stopping, Some(pid));
                self.line(&format!("{signal}: SIGINT to the node {pid}"));
                // The child is not reaped yet, so its pid names no other process.
                signals::send(pid, SIGINT)?;
                interrupted = Some(Instant::now());
            }
            let status = waitpid(node, Some(WaitPidFlag::WNOHANG));
            let status = status.map_err(|e| failed(e.to_string()))?;
            match status {
                WaitStatus::Exited(_, code) => break format!("exit status: {code}"),
                WaitStatus::Signaled(_, signal, _) => {
                    break format!("signal: {} ({signal})", signal as i32);
                }
                _ => {}
            }
            if !killed && interrupted.is_some_and(|at| at.elapsed() >= KILL_AFTER) {
                self.line(&format!("the node {pid} did not stop within 30 s: SIGKILL"));
                signals::send(pid, SIGKILL)?;
                killed = true;
            }
        };
        let deadline = Instant::now() + DRAIN;
        while pumps.iter().any(|p| !p.is_finished()) && Instant::now() < deadline {
            thread::sleep(POLL);
        }
        let ended = exited
            .recv_timeout(DRAIN)
            .unwrap_or_else(|_| Instant::now());
        let uptime = ended.saturating_duration_since(started);
        self.line(&format!(
            "the node exited ({exit}) after {} ms",
            uptime.as_millis()
        ));
        self.state.last_exit = Some(exit.to_string());
        let age = self.created.elapsed();
        Ok(self.restarts.after_exit(uptime, age, interrupted.is_some()))
    }

    /// Record `status` and the node's `pid` in the state file, and say so in the capture.
    fn set(&mut self, status: SupervisedStatus, pid: Option<u32>) {
        self.state.status = status;
        self.state.pid = pid;
        self.state.restarts = self.restarts.total;
        self.state.unstable = self.restarts.unstable;
        let written = self.state.write_in(&self.dir, &self.instance.state_path());
        if let Err(e) = written {
            self.line(&e.to_string());
        }
        self.line(&format!("status {}", status.name()));
    }

    /// A line of the supervisor's own in the output capture.
    fn line(&self, text: &str) {
        let mut out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = out.write(format!("supervisor: {text}\n").as_bytes());
    }
}

/// Start the node with `argv`: in a process group of its own when `own_group`, its input
/// `/dev/null`, its output and errors into the pipes returned, no signal blocked, and SIGHUP,
/// SIGINT, SIGQUIT, SIGPIPE and SIGTERM at their defaults, whatever the supervisor (which blocks
/// them) and its caller (a shell may ignore SIGINT) had.
fn spawn(
    argv: &[String],
    own_group: bool,
) -> Result<(Pid, PipeReader, PipeReader), Box<dyn Error>> {
    let (out, out_writer) = std::io::pipe()?;
    let (err, err_writer) = std::io::pipe()?;
    let null = File::open("/dev/null")?;
    let mut actions = PosixSpawnFileActions::init()?;
    let fds = [
        null.as_raw_fd(),
        out_writer.as_raw_fd(),
        err_writer.as_raw_fd(),
    ];
    for (target, fd) in (0..).zip(fds) {
        actions.add_dup2(fd, target)?;
    }
    let mut attributes = PosixSpawnAttr::init()?;
    attributes.set_sigmask(&SigSet::empty())?;
    let defaults = [SIGHUP, SIGINT, SIGQUIT, SIGPIPE, SIGTERM];
    attributes.set_sigdefault(&defaults.into_iter().collect())?;
    let mut flags =
        PosixSpawnFlags::POSIX_SPAWN_SETSIGMASK | PosixSpawnFlags::POSIX_SPAWN_SETSIGDEF;
    if own_group {
        // In the foreground the node leads a process group of its own, as pm2's detached children
        // do, so that a terminal's SIGINT (the caller's foreground group) reaches the supervisor
        // alone, which passes it on once. A daemon has left the terminal's session, and keeps the
        // node in its own group, so that `kill` ends both at once, even a node just started again
        // that has not taken its lock yet.
        attributes.set_pgroup(Pid::from_raw(0))?;
        flags |= PosixSpawnFlags::POSIX_SPAWN_SETPGROUP;
    }
    attributes.set_flags(flags)?;
    let args: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_str()))
        .collect::<Result<_, _>>()?;
    let pair = |(key, value): (OsString, OsString)| {
        CString::new([key.as_bytes(), b"=", value.as_bytes()].concat())
    };
    let env: Vec<CString> = std::env::vars_os().map(pair).collect::<Result<_, _>>()?;
    let program = args.first().ok_or("no command line")?;
    let node = posix_spawnp(program, &actions, &attributes, &args, &env)?;
    Ok((node, out, err))
}

/// Copy `from`, a stream of the node, into `into`, a line (or 64 KiB) at a time.
fn pump(from: PipeReader, into: &Arc<Mutex<Capture>>) -> JoinHandle<()> {
    let into = Arc::clone(into);
    thread::spawn(move || {
        let (mut reader, mut line) = (BufReader::new(from), Vec::new());
        while let Ok(1..) = (&mut reader).take(CHUNK).read_until(b'\n', &mut line) {
            let mut capture = into.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = capture.write(&line);
            line.clear();
        }
    })
}
