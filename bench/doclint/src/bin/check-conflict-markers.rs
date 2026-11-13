//! CI entry point: no committed document may carry an unresolved merge
//! conflict marker (NFR-MNT-01).
//!
//! ```text
//! cargo run -p doclint --bin check-conflict-markers
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    // Resolved at run time and announced, like `check-table-structure`: never
    // the compiled-in fallback silently (tests/runtime_root.rs, task #45).
    let root = match doclint::announce_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL  {e}");
            return ExitCode::FAILURE;
        }
    };
    let (scanned, problems) = match doclint::conflict_markers::check_repo_docs(&root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL  could not run the conflict-marker check\n  {e}");
            return ExitCode::FAILURE;
        }
    };
    if problems.is_empty() {
        println!("PASS  no document carries a conflict marker ({scanned} file(s) scanned)");
        return ExitCode::SUCCESS;
    }
    println!("FAIL  {} unresolved conflict marker(s)", problems.len());
    for problem in &problems {
        println!("  {}", problem.describe());
    }
    ExitCode::FAILURE
}
