//! Serat.
//!
//! The first line of `main` starts the cold-start timeline (NFR-PRF-01), and it is the
//! first line for a reason: every millisecond after it is inside the 400 ms budget.
//! Everything before it — the loader, the WebView2 runtime — is inside the budget too,
//! and is only visible when the budget harness supplies `t0` from outside; see
//! [`serat_app::startup`].

// Without this, a Windows release build opens a console window behind the application.
// Debug builds keep it, because that is where `eprintln!` goes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// `bench-bridge` relaxes the test-hooks release guard in lib.rs so the IPC-bridge example can
// be timed in release (task #172). That relaxation must never ship: the application binary
// does not build with it, whatever the profile.
#[cfg(feature = "bench-bridge")]
compile_error!("the `serat` binary must not be built with the `bench-bridge` feature");

use std::process::ExitCode;
use std::sync::Arc;

use serat_app::cli::Boot;
use serat_app::startup::Startup;

fn main() -> ExitCode {
    let startup = Arc::new(Startup::begin());

    let boot = match Boot::from_env() {
        Ok(boot) => boot,
        Err(problem) => {
            // A typo in a benchmark flag must fail rather than silently measure the
            // default.
            eprintln!("serat: {problem}");
            return ExitCode::from(2);
        }
    };

    match serat_app::run(startup, boot) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("serat: {err}");
            ExitCode::FAILURE
        }
    }
}
