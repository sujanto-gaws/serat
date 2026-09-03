//! NFR-PRF-04 · full rebuild of 20,000 notes within 30 seconds.
//!
//! Verifies **NFR-PRF-04** and, as one of the four P0 jobs, **NFR-PRF-08** and
//! **US-35**. Measured against the 20k benchmark vault, whose digest is checked
//! before the clock starts.
//!
//! ```text
//! cargo run --release -p budget --bin prf04-full-rebuild -- --profile reference
//! ```

use std::process::ExitCode;

use budget::{PRF04_FULL_REBUILD, Samples, subject};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    budget::run_job(&PRF04_FULL_REBUILD, &args, |vault, iterations| {
        // A fresh index directory per iteration: a rebuild over a warm index is
        // not a rebuild, and reusing the directory would measure the second run
        // of something the requirement describes as the first.
        let mut samples = Samples::new();
        for i in 0..iterations {
            let index_dir = vault.path.join(format!(".serat-bench-{i}"));
            let _ = std::fs::remove_dir_all(&index_dir);
            let elapsed = subject::full_rebuild(&vault.path, &index_dir)?;
            let _ = std::fs::remove_dir_all(&index_dir);
            samples.push(elapsed);
        }
        Ok(samples)
    })
}
