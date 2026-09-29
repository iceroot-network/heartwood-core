//! `heartwood snapshot rollback`: the relay's stored chain rolled back to the last block of the
//! round before the requested height's round, as the reference's `snapshot:rollback` does, with
//! the relay's lock checked and then held for the whole command. Rollback is the only snapshot
//! command: the snapshot formats are not built.
//!
//! The command resolves the relay's instance, refuses while the relay runs (the reference's text,
//! with the instance's name), takes the relay's lock, reads the relay's configuration and crypto
//! files, and opens the store only when the data directory holds one of the configured network.
//! It logs the reference's lines through the logger (`<token>-<network>-snapshot` in the log
//! directory) and exits with 0 when it rolled back, 1 on every refusal. The next start of the
//! relay replays the chain to the target and writes the next round's rows again.

use std::fmt::Display;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_blockchain::ChainStore;
use heartwood_blockchain::rollback;
use heartwood_cli::instance::{InstanceFlags, resolve};
use heartwood_cli::supervisor::{run_dir, signals::take_lock};
use heartwood_database::Settings;
use heartwood_kernel::config::{Role, crypto};
use heartwood_kernel::log::Level;
use serde_json::json;

use crate::app::relay::process_context;
use crate::config::{Base, Package};

/// A snapshot command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum SnapshotCommand {
    /// Roll the relay's stored chain back to the last block of a finished round.
    Rollback(RollbackFlags),
}

/// The flags of `snapshot rollback`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct RollbackFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The height to roll back to; its round's first block and the blocks above it go.
    #[arg(long, allow_negative_numbers = true)]
    pub height: Option<i64>,
    /// The number of blocks to roll back, when no height is given.
    #[arg(long, allow_negative_numbers = true)]
    pub number: Option<i64>,
}

/// Run `command`; the process's exit status.
pub fn run(command: SnapshotCommand) -> ExitCode {
    let SnapshotCommand::Rollback(flags) = command;
    match roll_back(&flags) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => crate::exit(Err(error)),
    }
}

/// The rollback `flags` ask for: whether it was made, else it was refused with the reference's
/// lines logged; an error when the command cannot run.
fn roll_back(flags: &RollbackFlags) -> Result<bool, String> {
    let (process, home, cwd) = process_context()?;
    let env = |name: &str| std::env::var(name).ok();
    let role = heartwood_cli::Role::Relay;
    let instance = resolve(role, &flags.instance, &env, &home, &cwd).map_err(text)?;
    let settings = Settings::in_data_dir(&instance.paths.data);
    if !settings.path.is_file() {
        return Err(format!("there is no store in {}", settings.path.display()));
    }
    run_dir(&instance).map_err(text)?;
    let running = format!("The \"{}\" process is already running", instance.name());
    let _lock = take_lock(&instance.lock_path())
        .map_err(text)?
        .ok_or(running)?;
    let names = (Some(&*instance.token), Some(&*instance.network));
    let base = Base::load(Role::Relay, names, None, (&process, &home, &cwd))?;
    let files = crypto::load(&base.paths.config.join("crypto")).map_err(text)?;
    let logger = base
        .app
        .plugins
        .iter()
        .find(|p| p.package == Package::Logger.name());
    let logger = logger.map_or_else(|| json!({}), |plugin| plugin.options.clone());
    let logger = crate::options::logger(&logger, &base.env).map_err(text)?;
    let stem = format!("{}-{}-snapshot", instance.token, instance.network);
    let _logs = heartwood_logger::init(&logger, &base.paths.log, &stem).map_err(text)?;
    let mut store = ChainStore::open(&settings).map_err(text)?;
    let meta = store.database().meta().map_err(text)?;
    if meta.is_none_or(|meta| meta.nethash != files.network.nethash) {
        return Err(format!(
            "{} is not this network's store",
            settings.path.display()
        ));
    }
    let tip = store.database().last_block().map_err(text)?;
    let current = tip
        .map(|block| block.row.height)
        .ok_or("the store has no block")?;
    let milestones = &files.milestones;
    let refused = match rollback::plan(flags.height, flags.number, current, milestones) {
        Ok(plan) => match rollback::rollback(&mut store, &plan, milestones) {
            Ok(rolled) => {
                log(rollback::lines(&plan, &rolled).map(|line| (Level::Info, line)));
                return Ok(true);
            }
            Err(error) => vec![
                (Level::Error, "Rollback failed".to_owned()),
                (Level::Error, text(error)),
            ],
        },
        Err(refusal) => refusal.lines(),
    };
    log(refused);
    Ok(false)
}

/// An error's text.
fn text(error: impl Display) -> String {
    error.to_string()
}

/// Log `lines`, each at its level (the rollback's are info or error lines).
fn log(lines: impl IntoIterator<Item = (Level, String)>) {
    for (level, line) in lines {
        match level {
            Level::Error => heartwood_kernel::error!("{line}"),
            _ => heartwood_kernel::info!("{line}"),
        }
    }
}
