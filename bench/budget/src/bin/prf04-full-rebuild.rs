//! NFR-PRF-04 · full rebuild of 20,000 notes within 30 seconds.
//!
//! Verifies **NFR-PRF-04** and, as one of the four P0 jobs, **NFR-PRF-08** and
//! **US-35**. Measured against the 20k benchmark vault, whose digest is checked
//! against the canonical one before the clock starts.
//!
//! The measurement itself is `budget::measure_full_rebuild`, in the library
//! rather than here. P0 finding F-27 was four mutations that no test noticed,
//! and every one of them was in a job binary that nothing covered — including
//! deleting the note-count guard, which is the only thing standing between a
//! partially indexed vault and a passing budget. What runs is now what is
//! tested, and this file has nothing left to get wrong.
//!
//! ```text
//! cargo run --release -p budget --bin prf04-full-rebuild -- --profile reference
//! ```

use std::process::ExitCode;

use budget::{PRF04_FULL_REBUILD, measure_full_rebuild, run_job};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run_job(&PRF04_FULL_REBUILD, &args, measure_full_rebuild)
}
