//! Print the measurement environment and whether it is gate eligible.
//!
//! Run this before and after a benchmark series so the conditions in a gate
//! report (`docs/05-testing/reports/`) are recorded rather than remembered.
//! Exits 0 if a filesystem-bound budget measured here could be recorded as the
//! gate figure, 1 if it could not — so a script can refuse to take the gate
//! number in the wrong environment instead of relying on someone noticing.
//!
//! ```text
//! cargo run --release -p budget --bin environment
//! ```

use std::process::ExitCode;

use budget::env::Environment;

fn main() -> ExitCode {
    let env = Environment::detect();
    println!("machine       {}", env.summary());
    println!("gate eligible {}", env.gate_eligible());
    match env.ineligibility_reason() {
        Some(reason) => {
            println!("reason        {reason}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    }
}
