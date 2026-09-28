//! The `heartwood` binary: `heartwood relay run` (and its alias `relay:run`).

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match heartwood_core::cli::parse(&args) {
        Ok(command) => heartwood_core::run(command),
        Err(error) => {
            // clap writes its own help and error text, with its exit status.
            let status = u8::try_from(error.exit_code()).unwrap_or(2);
            let _ = error.print();
            ExitCode::from(status)
        }
    }
}
