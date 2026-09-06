//! CI entry point for the F-303 check: every Must the PRD declares must be
//! accounted for somewhere in architecture.md's five design-accounting
//! places, and every ID those places cite must still exist in the PRD.
//!
//! ```text
//! cargo run -p doclint --bin check-prd-join
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let report = match doclint::prd_join::check_prd_join(&doclint::repo_root()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL  could not run the PRD/architecture.md join check\n  {e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", report.to_report());
    if report.holds() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
