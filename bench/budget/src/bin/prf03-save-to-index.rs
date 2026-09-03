//! NFR-PRF-03 · a single note save reflected in the index within 50 ms at p95.
//!
//! Verifies **NFR-PRF-03**, **NFR-PRF-08**, **US-35**. The acceptance criterion
//! is "benchmark over 1,000 saves; p95 within budget", so the default iteration
//! count is 1,000 and running fewer prints a warning that the result is not a
//! gate number.
//!
//! ```text
//! cargo run --release -p budget --bin prf03-save-to-index -- --profile reference
//! ```

use std::process::ExitCode;

use budget::{PRF03_SAVE_TO_INDEX, subject};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    budget::run_job(&PRF03_SAVE_TO_INDEX, &args, |vault, iterations| {
        let index_dir = vault.path.join(".serat-bench");
        subject::save_to_index(&vault.path, &index_dir, iterations)
    })
}
