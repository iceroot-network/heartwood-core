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
    let _ = top;
    super::not_built("top")
}
