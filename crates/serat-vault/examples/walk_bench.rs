//! The walk and the tree walk on a real vault, as numbers (F-12, FR-VLT-01; working
//! agreement 4: a change to an enumeration path adds or updates its bench).
//!
//! ```text
//! cargo run --release -p serat-vault --example walk_bench -- bench/vaults/vault-20k [runs]
//! ```
//!
//! Prints the median and worst of `runs` (default 10) for each enumeration, after one
//! untimed warm-up so the numbers describe syscalls rather than first touch of the
//! directory cache (ADR-0015 §4's "warm vault"). This is not the budget job: the
//! FR-VLT-01 interval is `tree_complete - T_open` in the application, with the rebuild
//! running beside the walk (bench/budget). This is the isolated cost of the two
//! enumerations, so that a regression in either is attributable to this crate.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serat_vault::Vault;

fn summarise(name: &str, mut samples: Vec<Duration>, count: usize) {
    samples.sort();
    let median = samples[samples.len() / 2];
    let worst = samples[samples.len() - 1];
    println!(
        "{name:<14} median {:>8.1} ms  worst {:>8.1} ms  ({count} files)",
        median.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("usage: walk_bench <vault-dir> [runs]");
        std::process::exit(2);
    };
    let runs: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(10);
    let vault = Vault::open(&root).expect("open vault");

    let warm = vault.walk().expect("warm-up walk");
    let count = warm.len();

    let mut walk = Vec::new();
    for _ in 0..runs {
        let started = Instant::now();
        let found = vault.walk().expect("walk");
        walk.push(started.elapsed());
        assert_eq!(found.len(), count);
    }
    summarise("Vault::walk", walk, count);

    // The tree walk: breadth-first, one listing per directory, names and kinds only.
    let mut tree = Vec::new();
    for _ in 0..runs {
        let started = Instant::now();
        let mut listed = 0usize;
        let summary = vault
            .walk_tree("", |_, entries| listed += entries.len())
            .expect("tree walk");
        tree.push(started.elapsed());
        assert_eq!(summary.files as usize, count);
        assert!(listed >= count);
    }
    summarise("Vault::walk_tree", tree, count);
}
