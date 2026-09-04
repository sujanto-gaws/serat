//! NFR-PRF-03 · a single note save reflected in the index within 50 ms at p95.
//!
//! Verifies **NFR-PRF-03**, **NFR-PRF-08**, **US-35**. The acceptance criterion
//! is "benchmark over 1,000 saves; p95 within budget", so the default iteration
//! count is 1,000 and running fewer prints a warning that the result is not a
//! gate number.
//!
//! **F-03 is ruled:** there are two paths into the index, not one. A save made
//! in Serat goes straight from the write to the parse pool to the writer, with
//! no watcher and no debounce, and that is what this 50 ms governs. An external
//! change goes through the watcher's 150 ms coalescing window and is
//! FR-VLT-04's 500 ms. The job measures both and asserts the save path; the
//! end-to-end figure is reported beside it, inside the report block rather than
//! above it (F-28).
//!
//! ```text
//! cargo run --release -p budget --bin prf03-save-to-index -- --profile reference
//! ```

use std::process::ExitCode;

use budget::{PRF03_SAVE_TO_INDEX, measure_save_to_index, run_job};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run_job(&PRF03_SAVE_TO_INDEX, &args, measure_save_to_index)
}
