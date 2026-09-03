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

use budget::env::Environment;
use budget::{
    Outcome, PRF01_VAULT_USABLE, PRF01_WINDOW_VISIBLE, Profile, assert_scale_independent,
    ensure_vault, subject, write_result,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let profile = match args.iter().position(|a| a == "--profile") {
        Some(i) => match args.get(i + 1).map(|s| Profile::parse(s)) {
            Some(Ok(p)) => p,
            _ => {
                eprintln!("budget: --profile needs reference or ci");
                return ExitCode::FAILURE;
            }
        },
        None => Profile::Reference,
    };
    let skip_scale = args.iter().any(|a| a == "--skip-scale");

    let vault = match ensure_vault(PRF01_WINDOW_VISIBLE.vault) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("budget: {e}");
            return ExitCode::FAILURE;
        }
    };

    let iterations = PRF01_WINDOW_VISIBLE.iterations;
    let (visible, usable) = match subject::cold_start(&vault.path, iterations) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!(
                "FAIL  {} · {} and {}
  {e}",
                PRF01_WINDOW_VISIBLE.requirement, PRF01_WINDOW_VISIBLE.id, PRF01_VAULT_USABLE.id
            );
            return ExitCode::FAILURE;
        }
    };

    let environment = Environment::detect();
    let mut ok = true;
    let mut visible_p95 = 0.0;
    for (budget, samples) in [
        (PRF01_WINDOW_VISIBLE, visible),
        (PRF01_VAULT_USABLE, usable),
    ] {
        let outcome = Outcome {
            budget,
            profile,
            vault: vault.clone(),
            environment: environment.clone(),
            samples,
            // Cold start has no phase breakdown; §3.4's three phases are marks
            // on one timeline, and they are the two budgets themselves.
            phases: None,
        };
        if budget.id == PRF01_WINDOW_VISIBLE.id {
            visible_p95 = outcome.value_ms();
        }
        print!("{}", outcome.to_report());
        match write_result(&outcome) {
            Ok(p) => println!("  result      {}", p.display()),
            Err(e) => eprintln!("budget: could not write the result file: {e}"),
        }
        ok &= outcome.passed();
    }

    if !skip_scale {
        ok &= scale_check(visible_p95);
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Architecture §3.4: nothing on the critical path is proportional to vault
/// size. Same measurement, 2.5x the notes.
fn scale_check(visible_p95_20k: f64) -> bool {
    let large = match ensure_vault("50k") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("budget: cannot check scale independence: {e}");
            return false;
        }
    };
    let (visible, _) = match subject::cold_start(&large.path, PRF01_WINDOW_VISIBLE.iterations) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("budget: cannot check scale independence: {e}");
            return false;
        }
    };
    let visible_p95_50k = visible.percentile(0.95);
    match assert_scale_independent(visible_p95_20k, visible_p95_50k) {
        Ok(()) => {
            println!(
                "PASS  NFR-PRF-01 · scale independence
                   window painted p95 {visible_p95_20k:.1} ms at 20k, {visible_p95_50k:.1} ms at                  50k — the critical path does not grow with the vault (architecture §3.4)"
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
