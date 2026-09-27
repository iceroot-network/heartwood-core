//! `mock-peer`: the standalone mock peer, serving a corpus chain until it is killed. Test support
//! only (the `difftest` feature).

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("mock-peer: {error}");
            return ExitCode::FAILURE;
        }
    };
    let args = std::env::args().skip(1).collect();
    match runtime.block_on(heartwood_p2p::testing::mock_peer::run_standalone(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mock-peer: {error}");
            ExitCode::FAILURE
        }
    }
}
