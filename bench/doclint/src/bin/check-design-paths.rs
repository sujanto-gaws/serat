//! CI entry point for the F-239 check: every path cited in the design
//! documents must resolve, unless it is marked planned in
//! `docs/05-testing/planned-design-artifacts.txt`.
//!
//! ```text
//! cargo run -p doclint --bin check-design-paths
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let report = match doclint::check_paths(&doclint::repo_root()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL  could not run the design-doc path check\n  {e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", report.to_report());
    if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
