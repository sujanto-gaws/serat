//! CI entry point for the F-346/F-348 check: every table in the design
//! documents must render as a table -- pipe counts match the header, no
//! orphan rows, no row left dangling with no blank line to end it.
//!
//! ```text
//! cargo run -p doclint --bin check-table-structure
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
    let (scanned, problems) = match doclint::table_structure::check_repo_docs(&root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL  could not run the table-structure check\n  {e}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", doclint::table_structure::to_report(&problems));
    println!("({scanned} markdown file(s) scanned)");
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
