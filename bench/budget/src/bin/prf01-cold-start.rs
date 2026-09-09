//! NFR-PRF-01 · the two pre-split cold-start aggregates, reported; and §3.4's
//! scale-independence assertion, on the Serat term.
//!
//! Verifies **NFR-PRF-01**, **NFR-PRF-08**, **US-35**, and the load-bearing
//! property of architecture §3.4.
//!
//! **Task #118 changed what this job gates, and not what it measures.** PRD
//! v1.2's re-baseline gates only the Serat term for NFR-PRF-01
//! (`prf01-serat-term`); `window_visible` and `vault_usable` are reported
//! figures, because F-407 measured the aggregate moving ~19% batch-to-batch
//! against ~8% for the Serat term, and a threshold the size of its own noise
//! cannot decide a pass. CI asserted them anyway until this task — that gap
//! between the requirement and the job is F-443 — so both marks are still
//! measured here, still validated, still printed and still written to their
//! JSON, and a breach of either no longer fails the build.
//!
//! Measured from the startup trace `serat-app` writes and validated by
//! [`budget::trace`], which refuses a trace that undercounts pre-`main` time,
//! comes from a debug build, has marks out of order, or shows a paint mark
//! wired to window *creation* rather than to a composited frame.
//!
//! **The one assertion left in this job is the scale check**, and §3.4's task
//! #118 ruling moved its subject: it now compares the **Serat term** across
//! the 20k and 50k vaults, with both aggregates' ratios reported beside the
//! verdict and asserted on nothing. The job stays here rather than moving to
//! `prf01-serat-term`, because that job is defined by being a second
//! *reduction* of one batch and this is a second *measurement* — two vaults,
//! interleaved, with the 50,000-note fixture in it (F-446).
//!
//! ```text
//! cargo run --release -p budget --bin prf01-cold-start -- --profile reference
//! ```
//!
//! `--skip-scale` reports the two figures only, for a quick local check. That
//! leaves the run with nothing asserted in it at all, which is what
//! `--skip-scale` now means and why it is not what CI runs.

use std::process::ExitCode;

use budget::{PRF01_VAULT_USABLE, PRF01_WINDOW_VISIBLE, measure_cold_start, run_paired_job};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let skip_scale = args.iter().any(|a| a == "--skip-scale");

    // Both figures go through run_paired_job, so they keep the argument
    // parsing, the empty-samples check and the vault verification that this
    // binary used to reimplement and lose (F-27). Since #118 it fails only
    // when a measurement could not be *taken*: `Budget::gated` is false for
    // both of these, so a breach is reported and does not fail the build.
    let reported = run_paired_job(
        &PRF01_WINDOW_VISIBLE,
        &PRF01_VAULT_USABLE,
        &args,
        measure_cold_start,
    );
    if reported != ExitCode::SUCCESS || skip_scale {
        return reported;
    }
    // Architecture §3.4: nothing on the critical path is proportional to vault
    // size. The measurement and the judgement both live in the library, because
    // F-81 was this exact call being droppable with nothing going red.
    if budget::run_scale_check(PRF01_WINDOW_VISIBLE.iterations) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
