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

use budget::{
    PRF01_VAULT_USABLE, PRF01_WINDOW_VISIBLE, assert_scale_independent, ensure_vault,
    measure_cold_start, run_paired_job, subject,
};

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
    if scale_check() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Architecture §3.4: nothing on the critical path is proportional to vault
/// size. Same measurement, 2.5x the notes.
fn scale_check() -> bool {
    let mut p95 = Vec::new();
    for preset in ["20k", "50k"] {
        let vault = match ensure_vault(preset) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("budget: cannot check scale independence: {e}");
                return false;
            }
        };
        match subject::cold_start(&vault.path, PRF01_WINDOW_VISIBLE.iterations) {
            Ok((visible, _)) => p95.push(visible.percentile(0.95)),
            Err(e) => {
                eprintln!("budget: cannot check scale independence: {e}");
                return false;
            }
        }
    }
    match assert_scale_independent(p95[0], p95[1]) {
        Ok(()) => {
            println!(
                "PASS  NFR-PRF-01 · scale independence
  window painted p95 {:.1} ms at 20k,                  {:.1} ms at 50k — the critical path does not grow with the vault (§3.4)",
                p95[0], p95[1]
            );
            true
        }
        Err(e) => {
            println!(
                "FAIL  NFR-PRF-01 · scale independence
  {e}"
            );
            false
        }
    }
}
