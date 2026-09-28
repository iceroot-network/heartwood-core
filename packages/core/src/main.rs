//! The `heartwood` binary: the command tree of [`heartwood_core::commands`], with the reference's
//! colon spellings (`relay:run`) rewritten first.

use std::process::ExitCode;

fn main() -> ExitCode {
    match heartwood_core::commands::parse(std::env::args_os().collect()) {
        Ok(command) => heartwood_core::commands::run(command),
        Err(error) => {
            // clap writes its own help and error text, with its exit status.
            let status = u8::try_from(error.exit_code()).unwrap_or(2);
            let _ = error.print();
            ExitCode::from(status)
        }
    }
}
