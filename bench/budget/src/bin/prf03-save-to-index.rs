//! NFR-PRF-03 · a single note save reflected in the index within 50 ms at p95.
//!
//! Verifies **NFR-PRF-03**, **NFR-PRF-08**, **US-35**. The acceptance criterion
//! is "benchmark over 1,000 saves; p95 within budget", so the default iteration
//! count is 1,000 and running fewer prints a warning that the result is not a
//! gate number.
//!
//! **P0 finding F-03 is open, and this job is where it lands.** "save→index"
//! has two readings: end to end from the file being written, which includes the
//! 150 ms per-path coalescing window of architecture §3.1 step 1 and therefore
//! cannot fit inside 50 ms by construction; and the pipeline's own cost from the
//! event being dequeued. This job measures both and asserts the second, because
//! it is the only reading the current design can satisfy — and prints the first
//! beside it, so the gate report states which number the 50 ms claim rests on
//! rather than implying there was only ever one. If the architect rules the
//! other way, the assertion moves one line.
//!
//! ```text
//! cargo run --release -p budget --bin prf03-save-to-index -- --profile reference
//! ```

use std::process::ExitCode;

use budget::{PRF03_SAVE_TO_INDEX, Samples, subject};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    budget::run_job(&PRF03_SAVE_TO_INDEX, &args, |vault, iterations| {
        let index_dir = vault.path.join(".serat-bench");
        let both = subject::save_to_index(&vault.path, &index_dir, iterations)?;
        report_end_to_end(&both.end_to_end);
        Ok(both.pipeline)
    })
}

/// The interval the budget does *not* assert, printed so it cannot be lost.
fn report_end_to_end(samples: &Samples) {
    if samples.is_empty() {
        return;
    }
    println!(
        "  F-03        end-to-end (file written → visible, includes the 150 ms coalescing          window): p95 {:.1} ms over {} samples — reported, not asserted",
        samples.percentile(0.95),
        samples.len()
    );
}
