//! The crate's errors. Each names the file, instance or address it is about, so that a command
//! can print it as it is.

use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;

/// An instance could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstanceError {
    /// Neither `--token` nor `config.json` names the token.
    #[error("no token: pass --token or set it in {}", config.display())]
    MissingToken {
        /// The `config.json` read.
        config: PathBuf,
    },
    /// Neither `--network` nor `config.json` names the network.
    #[error("no network: pass --network or set it in {}", config.display())]
    MissingNetwork {
        /// The `config.json` read.
        config: PathBuf,
    },
    /// A token or network name that cannot be part of a file name.
    #[error("the {what} {value:?} may hold letters, digits, '-' and '_' only")]
    InvalidName {
        /// `token` or `network`.
        what: &'static str,
        /// The name given.
        value: String,
    },
    /// `config.json` exists but is not valid.
    #[error("{} is not valid: {reason}", path.display())]
    Config {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
}

/// A process command or the supervisor failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProcessError {
    /// `start` of a role that is running.
    #[error("{name} is already running")]
    AlreadyRunning {
        /// The instance's name.
        name: String,
    },
    /// `stop`, `restart` or `log` of a role that is not running.
    #[error("{name} is not running")]
    NotRunning {
        /// The instance's name.
        name: String,
    },
    /// A lock file could not be taken or read.
    #[error("the lock {} cannot be taken: {message}", path.display())]
    Lock {
        /// The lock file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// A process could not be started.
    #[error("{program} cannot be started: {message}")]
    Spawn {
        /// The program.
        program: String,
        /// The system's error.
        message: String,
    },
    /// A signal could not be sent.
    #[error("process {pid} cannot be signalled: {message}")]
    Signal {
        /// The process.
        pid: u32,
        /// The system's error.
        message: String,
    },
    /// A process did not stop in time.
    #[error("{name} did not stop within {seconds} s")]
    Timeout {
        /// The instance's name.
        name: String,
        /// The time waited.
        seconds: u64,
    },
    /// A state, pid or log file could not be read or written.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The system's error.
        message: String,
    },
    /// A state file is not valid.
    #[error("{} is not a valid state file: {reason}", path.display())]
    State {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// A command that is not built yet.
    #[error("{0} is not built yet")]
    NotBuilt(&'static str),
}

/// `network generate` failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NetworkError {
    /// The profile could not be read or is not valid.
    #[error("the network profile {name} is not valid: {reason}")]
    Profile {
        /// The profile's name or file.
        name: String,
        /// Why.
        reason: String,
    },
    /// The chain definition could not be built from the inputs.
    #[error("the chain definition cannot be built: {0}")]
    Genesis(String),
    /// The epoch is not a whole second in the reference's format.
    #[error("the epoch {epoch:?} is not a time such as 2026-01-01T00:00:00.000Z")]
    Epoch {
        /// The epoch given.
        epoch: String,
    },
    /// The output directory holds files and `--force` was not given.
    #[error("{} is not empty; pass --force to overwrite its files", path.display())]
    Exists {
        /// The directory.
        path: PathBuf,
    },
    /// A file could not be written.
    #[error("{} cannot be written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The system's error.
        message: String,
    },
}

/// A loopback HTTP read failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HttpError {
    /// The address is not a loopback address: the CLI reads local APIs only.
    #[error("{address} is not a loopback address")]
    NotLoopback {
        /// The address.
        address: SocketAddr,
    },
    /// The connection could not be made, or failed.
    #[error("GET http://{address}{path} failed: {message}")]
    Connection {
        /// The address.
        address: SocketAddr,
        /// The path.
        path: String,
        /// The system's error.
        message: String,
    },
    /// The request did not complete in time.
    #[error("GET http://{address}{path} timed out")]
    Timeout {
        /// The address.
        address: SocketAddr,
        /// The path.
        path: String,
    },
    /// The answer is not an HTTP/1.1 response, or is too long.
    #[error("GET http://{address}{path} gave no valid response: {reason}")]
    Response {
        /// The address.
        address: SocketAddr,
        /// The path.
        path: String,
        /// Why.
        reason: String,
    },
    /// The status was not 200.
    #[error("GET http://{address}{path} answered {status}")]
    Status {
        /// The address.
        address: SocketAddr,
        /// The path.
        path: String,
        /// The HTTP status.
        status: u16,
    },
    /// The body is not the JSON the read expects.
    #[error("GET http://{address}{path} answered a body that cannot be read: {reason}")]
    Body {
        /// The address.
        address: SocketAddr,
        /// The path.
        path: String,
        /// Why.
        reason: String,
    },
}

/// A devnet command failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DevnetError {
    /// The node counts are out of range.
    #[error("{0}")]
    Counts(String),
    /// A listener's address cannot be bound: another process holds it, or it is not local.
    #[error("{address} cannot be bound: {message}")]
    Address {
        /// The address.
        address: SocketAddr,
        /// The system's error.
        message: String,
    },
    /// A devnet guard refused the configuration: a host that is not loopback, or a network
    /// whose name is not `devnet`.
    #[error("the devnet guard refuses the configuration: {0}")]
    Guard(String),
    /// The devnet's directory holds no valid manifest, or its files do not match it.
    #[error("the devnet in {} is not valid: {reason}", root.display())]
    Manifest {
        /// The devnet's directory.
        root: PathBuf,
        /// Why.
        reason: String,
    },
    /// The directory is not empty and `--force` was not given.
    #[error("{} is not empty; pass --force to replace the devnet", root.display())]
    Exists {
        /// The devnet's directory.
        root: PathBuf,
    },
    /// A node is not in the devnet.
    #[error("the devnet has no node {0}")]
    NoNode(u32),
    /// A wait ran out.
    #[error("{what} did not happen within {seconds} s")]
    Timeout {
        /// What was waited for.
        what: String,
        /// The time waited.
        seconds: u64,
    },
    /// A process command failed.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// A relay's API could not be read.
    #[error(transparent)]
    Api(#[from] HttpError),
    /// The chain could not be generated.
    #[error(transparent)]
    Generate(#[from] NetworkError),
    /// A file could not be read or written.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The system's error.
        message: String,
    },
    /// A devnet command or flag that is not built yet.
    #[error("{0} is not built yet")]
    NotBuilt(&'static str),
    /// `devnet run` found no scenario runner: the harness is not installed.
    #[error(
        "{0} is neither next to this binary nor on PATH; the scenario runner belongs to the test \
         harness, which is not installed"
    )]
    RunnerMissing(&'static str),
}
