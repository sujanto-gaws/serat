//! CI entry point for F-496: a count in prose immediately above a list must
//! match the list (task #126).
//!
//! ```text
//! cargo run -p doclint --bin check-count-over-list
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let root = match doclint::announce_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL  {e}");
            return ExitCode::FAILURE;
        }
    };
    let (scanned, problems) = match doclint::count_over_list::check_repo_docs(&root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL  could not run the count-over-list check\n  {e}");
            return ExitCode::FAILURE;
        }
    };
    if problems.is_empty() {
        println!("PASS  every count written above a list matches it ({scanned} file(s) scanned)");
        return ExitCode::SUCCESS;
    }
    println!(
        "FAIL  {} count(s) do not match the list beneath them",
        problems.len()
    );
    for problem in &problems {
        println!("  {}", problem.describe());
    }
    ExitCode::FAILURE
}
