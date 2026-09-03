//! NFR-PRF-04 · full rebuild of 20,000 notes within 30 seconds.
//!
//! Verifies **NFR-PRF-04** and, as one of the four P0 jobs, **NFR-PRF-08** and
//! **US-35**. Measured against the 20k benchmark vault, whose digest is checked
//! before the clock starts.
//!
//! **P0 finding F-10:** the aggregate is what the budget asserts, because
//! NFR-PRF-04 is a wall-clock requirement. Beside it the job records where the
//! time went — walk, read+hash, parse, write — because a single number cannot
//! answer architecture.md's open design question 1, *does the parse pool or the
//! index writer dominate rebuild time?*, and §1.2 says the parse pool exists
//! because of that unmeasured assumption. "The writer dominates" and "reading
//! 39.4 MiB off disk dominates" are indistinguishable in one figure and imply
//! completely different fixes.
//!
//! ```text
//! cargo run --release -p budget --bin prf04-full-rebuild -- --profile reference
//! ```

use std::process::ExitCode;

use budget::subject::{RebuildPhases, RebuildTiming};
use budget::{PRF04_FULL_REBUILD, Samples, subject};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    budget::run_job_with_phases(&PRF04_FULL_REBUILD, &args, |vault, iterations| {
        // A fresh index directory per iteration: a rebuild over a warm index is
        // not a rebuild, and reusing the directory would measure the second run
        // of something the requirement describes as the first.
        let mut samples = Samples::new();
        let mut runs: Vec<RebuildTiming> = Vec::with_capacity(iterations);
        for i in 0..iterations {
            let index_dir = vault.path.join(format!(".serat-bench-{i}"));
            let _ = std::fs::remove_dir_all(&index_dir);
            let timing = subject::full_rebuild(&vault.path, &index_dir)?;
            let _ = std::fs::remove_dir_all(&index_dir);

            // A rebuild that indexed half the vault quickly is the failure a
            // wall-clock number cannot see on its own.
            if timing.notes != vault.notes {
                return Err(format!(
                    "the rebuild reported {} notes, the vault manifest says {}.                      A fast number over a partial index is not a passing budget.",
                    timing.notes, vault.notes
                ));
            }
            samples.push(timing.total);
            runs.push(timing);
        }
        Ok((samples, Some(median_phases(&runs))))
    })
}

/// The per-phase median across runs.
///
/// Median rather than mean, and per phase rather than from the median run,
/// because one slow run should not decide where the time went — the budget's own
/// statistic for NFR-PRF-04 is a median for the same reason.
fn median_phases(runs: &[RebuildTiming]) -> RebuildPhases {
    fn median(mut xs: Vec<std::time::Duration>) -> std::time::Duration {
        if xs.is_empty() {
            return std::time::Duration::ZERO;
        }
        xs.sort_unstable();
        xs[(xs.len() - 1) / 2]
    }
    RebuildPhases {
        walk: median(runs.iter().map(|r| r.phases.walk).collect()),
        read_hash: median(runs.iter().map(|r| r.phases.read_hash).collect()),
        parse: median(runs.iter().map(|r| r.phases.parse).collect()),
        write: median(runs.iter().map(|r| r.phases.write).collect()),
    }
}
