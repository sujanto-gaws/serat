//! NFR-PRF-01 · interactive window within 400 ms, usable vault within 1 s.
//!
//! Verifies **NFR-PRF-01**, **NFR-PRF-08**, **US-35**, and the load-bearing
//! property of architecture §3.4. One requirement, two marks on one timeline,
//! so this job asserts both and reports both — "cold start failed" without
//! saying which half is a bug report nobody can act on.
//!
//! Measured from the startup trace `serat-app` writes and validated by
//! [`budget::trace`], which refuses a trace that undercounts pre-`main` time,
//! comes from a debug build, has marks out of order, or shows a paint mark
//! wired to window *creation* rather than to a composited frame.
//!
//! It also runs against the **50k** vault and asserts the window-painted mark
//! did not grow, because §3.4's claim is that nothing on the critical path is
//! proportional to vault size — the main thread never opens the index and the
//! file tree renders from a query, not from the walk. That is a claim worth a
//! test rather than a paragraph.
//!
//! ```text
//! cargo run --release -p budget --bin prf01-cold-start -- --profile reference
//! ```
//!
//! `--skip-scale` runs the two budgets only, for a quick local check.

use std::process::ExitCode;

use budget::{PRF01_VAULT_USABLE, PRF01_WINDOW_VISIBLE, measure_cold_start, run_paired_job};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let skip_scale = args.iter().any(|a| a == "--skip-scale");

    // Both budgets go through run_paired_job, so they keep the argument
    // parsing, the empty-samples check and the vault verification that this
    // binary used to reimplement and lose (F-27).
    let budgets = run_paired_job(
        &PRF01_WINDOW_VISIBLE,
        &PRF01_VAULT_USABLE,
        &args,
        measure_cold_start,
    );
    if budgets != ExitCode::SUCCESS || skip_scale {
        return budgets;
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
