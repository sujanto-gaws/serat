//! The `--assert` plumbing the P1 bins share (NFR-PRF-08, task #9/#10): pure functions,
//! so the verdict, the refusal and the exit decision are unit-tested rather than read.

/// `--assert` gates a figure measured at 20,000 notes only. Refuse any other vault
/// rather than gate a number the budget does not name.
///
/// # Errors
/// If `assert` is set and `vaults` does not include `20k`.
pub fn require_20k(assert: bool, vaults: &[&str]) -> Result<(), String> {
    if assert && !vaults.contains(&"20k") {
        return Err("--assert gates a figure at 20,000 notes; include --vault 20k".to_owned());
    }
    Ok(())
}

/// Like [`require_20k`] for a bin that measures exactly one vault.
///
/// # Errors
/// If `assert` is set and `vault` is not `20k`.
pub fn require_only_20k(assert: bool, vault: &str) -> Result<(), String> {
    if assert && vault != "20k" {
        return Err(format!(
            "--assert gates a figure at 20,000 notes; got --vault {vault}, use --vault 20k"
        ));
    }
    Ok(())
}

/// The rows (label, observed ms) strictly over `limit_ms`, as printable breach lines.
pub fn breaches(limit_ms: f64, rows: &[(String, f64)]) -> Vec<String> {
    rows.iter()
        .filter(|(_, ms)| *ms > limit_ms)
        .map(|(label, ms)| format!("{label}: p95 {ms:.2} ms over {limit_ms:.0} ms"))
        .collect()
}

/// Whether the process succeeds: always without `--assert`, otherwise only with no breach.
pub fn passes(assert: bool, breaches: &[String]) -> bool {
    !assert || breaches.is_empty()
}

/// Whether `--assert` is only reporting on this runner (task #166): the bin was asked
/// to assert, and the environment cannot produce a gate number.
pub fn report_only(assert: bool, gate_eligible: bool) -> bool {
    assert && !gate_eligible
}

/// [`passes`] for a bin whose budget is asserted only where the environment is gate
/// eligible. A failure to measure never reaches here: the bin returns an error first.
pub fn passes_when_eligible(assert: bool, gate_eligible: bool, breaches: &[String]) -> bool {
    passes(assert && !report_only(assert, gate_eligible), breaches)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(l, m)| ((*l).to_owned(), *m)).collect()
    }

    #[test]
    fn a_row_at_the_limit_passes_and_one_over_it_is_a_breach() {
        assert!(breaches(50.0, &rows(&[("a", 50.0), ("b", 49.9)])).is_empty());
        let b = breaches(50.0, &rows(&[("a", 50.01), ("b", 10.0)]));
        assert_eq!(b.len(), 1);
        assert!(b[0].starts_with("a: p95 50.01"));
    }

    #[test]
    fn assert_fails_on_a_breach_and_passes_without_one() {
        let b = breaches(50.0, &rows(&[("a", 60.0)]));
        assert!(!passes(true, &b));
        assert!(passes(true, &[]));
    }

    #[test]
    fn without_assert_a_breach_never_changes_the_exit() {
        let b = breaches(50.0, &rows(&[("a", 600.0)]));
        assert!(!b.is_empty());
        assert!(passes(false, &b));
    }

    #[test]
    fn assert_refuses_anything_but_the_20k_vault() {
        assert!(require_20k(true, &["50k"]).is_err());
        assert!(require_20k(true, &["20k", "50k"]).is_ok());
        assert!(require_20k(false, &["50k"]).is_ok());
        assert!(require_only_20k(true, "50k").is_err());
        assert!(require_only_20k(true, "20k").is_ok());
        assert!(require_only_20k(false, "50k").is_ok());
    }

    #[test]
    fn a_breach_fails_where_the_runner_is_eligible_and_only_reports_where_it_is_not() {
        let b = breaches(500.0, &rows(&[("single note", 615.3)]));
        assert!(!passes_when_eligible(true, true, &b));
        assert!(passes_when_eligible(true, false, &b));
        assert!(report_only(true, false));
        assert!(!report_only(true, true));
        assert!(!report_only(false, false));
        // No breach passes either way; the eligible runner with a breach still fails.
        assert!(passes_when_eligible(true, true, &[]));
    }
}
