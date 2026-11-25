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
//! **This file holds no logic, and that is task #124's whole point (F-501).**
//! F-81 recorded `run_scale_check` being droppable with nothing going red, and
//! the remedy moved the measurement and the judgement into the library -- but
//! the *composition* stayed here, in a `main` no test could call, and deleting
//! the scale-check call from it still left the suite green. So the composition
//! is `budget::cold_start_job` now, driven in tests through
//! `cold_start_job_with`. **A `main` that does anything is a `main` nobody can
//! test.**
//!
//! ```text
//! cargo run --release -p budget --bin prf01-cold-start -- --profile reference
//! ```
//!
//! `--skip-scale` reports the two figures only, for a quick local check. That
//! leaves the run with nothing asserted in it at all, which is what
//! `--skip-scale` means and why it is not what CI runs.

use std::process::ExitCode;

fn main() -> ExitCode {
    budget::cold_start_job(&std::env::args().skip(1).collect::<Vec<String>>())
}
