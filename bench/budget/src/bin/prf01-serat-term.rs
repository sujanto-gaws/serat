//! NFR-PRF-01 (re-baselined, task #104) · the Serat term of cold start < 350 ms p95.
//!
//! Verifies **NFR-PRF-01**, **NFR-PRF-08**, **US-35**, ADR-0014 §Decision 3. This is the
//! binding CI gate for NFR-PRF-01 from the #104 re-baseline forward: `S_pre + S_setup +
//! S_shell + S_settle`, the portion of cold start Serat's own code is responsible for.
//! The platform term and both legacy marks (`window_visible`, `vault_usable`) are
//! reported inside this job's own output, never asserted — F-407 found the aggregate
//! moves ~19% batch-to-batch against ~8% for the Serat term, so it cannot decide a pass
//! at any budget.
//!
//! `prf01-cold-start` still asserts the two legacy marks separately (task #118, F-443:
//! that job has not yet been demoted to reported-only, and this job's task is additive
//! only). Running both today means NFR-PRF-01 is double-gated until #118 lands; that is
//! the known, named state of the gap, not an oversight in either job.
//!
//! ```text
//! cargo run --release -p budget --bin prf01-serat-term -- --profile reference
//! ```

use std::process::ExitCode;

use budget::{PRF01_SERAT_TERM, measure_serat_term, run_job};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run_job(&PRF01_SERAT_TERM, &args, measure_serat_term)
}
