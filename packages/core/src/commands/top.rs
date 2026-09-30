//! `heartwood top`: the token's running instances across its networks, with `status`'s columns.

use std::process::ExitCode;

use clap::Args;

/// The flags of `top`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Top {
    /// The token (`solar` by default).
    #[arg(long)]
    pub token: Option<String>,
}

/// Run `top`.
pub fn run(top: Top) -> ExitCode {
    let token = top.token.unwrap_or_else(|| "solar".to_owned());
    let root = heartwood_cli::instance_index::process_data_root(&token);
    let rows = heartwood_cli::top::top(&token, &root).map_err(|e| e.to_string());
    let table = rows.and_then(|rows| match rows.is_empty() {
        true => Err(heartwood_cli::top::NONE_RUNNING.to_owned()),
        false => Ok(heartwood_cli::process_table::render(&rows)),
    });
    crate::exit(table.map(|text| print!("{text}")))
}
