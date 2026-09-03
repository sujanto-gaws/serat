//! NFR-PRF-01 · interactive window within 400 ms, usable vault within 1 s.
//!
//! Verifies **NFR-PRF-01**, **NFR-PRF-08**, **US-35**. One requirement, two
//! marks on one timeline, so this job asserts two budgets and fails if either
//! is breached — reporting both, because "cold start failed" without saying
//! which half is a bug report nobody can act on.
//!
//! Blocked on task D2: the architect defines t0, "window visible" and "vault
//! usable", and this job measures whatever D2 defines rather than guessing.
//!
//! ```text
//! cargo run --release -p budget --bin prf01-cold-start -- --profile reference
//! ```

use std::process::ExitCode;

use budget::env::Environment;
use budget::{
    Outcome, PRF01_VAULT_USABLE, PRF01_WINDOW_VISIBLE, Profile, ensure_vault, subject, write_result,
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
                "FAIL  {} · {} and {}\n  {e}",
                PRF01_WINDOW_VISIBLE.requirement, PRF01_WINDOW_VISIBLE.id, PRF01_VAULT_USABLE.id
            );
            return ExitCode::FAILURE;
        }
    };

    let environment = Environment::detect();
    let mut ok = true;
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
        };
        print!("{}", outcome.to_report());
        match write_result(&outcome) {
            Ok(p) => println!("  result      {}", p.display()),
            Err(e) => eprintln!("budget: could not write the result file: {e}"),
        }
        ok &= outcome.passed();
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
