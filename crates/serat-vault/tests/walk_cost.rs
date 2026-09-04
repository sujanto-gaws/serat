//! What the walk costs, as a number rather than an adjective (F-12, working
//! agreement 4).
//!
//! Not a budget job — that is `bench/budget`'s, against the real 20k vault. This is the
//! regression guard that lives with the code: it builds a synthetic tree, walks it both
//! ways, and fails if the canonicalising path is not decisively more expensive. If
//! someone reintroduces `canonicalize` per entry, this says so without waiting for a CI
//! run against a vault that has to be generated first.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serat_vault::Vault;

/// Files to build. Enough that the syscall difference is outside timer noise, small
/// enough that the test stays well under a second.
const FILES: usize = 2_000;

fn build(root: &Path) {
    for i in 0..FILES {
        let dir = root.join(format!("{:02}-folder", i % 40));
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join(format!("note-{i:05}.md")), b"# Note\n").expect("write");
    }
}

#[test]
fn the_walk_does_not_canonicalise_every_entry() {
    let dir = tempfile::tempdir().expect("temp");
    build(dir.path());
    let vault = Vault::open(dir.path()).expect("open");

    // Warm the directory cache so this measures syscalls rather than first touch.
    let entries = vault.walk().expect("warm");
    assert_eq!(entries.len(), FILES);

    // Best of five each, interleaved. A single pair of timings can invert under
    // contention — I saw lexical measure 146 ms on a loaded machine while the
    // canonicalising side measured 135 ms — and a guard that reports the opposite of
    // the truth when the machine is busy is worse than no guard. The minimum is the
    // right statistic here because both sides do identical work every round: noise
    // only ever adds.
    let mut lexical = Duration::MAX;
    let mut canonical = Duration::MAX;

    for _ in 0..5 {
        let started = Instant::now();
        let found = vault.walk().expect("walk");
        let elapsed = started.elapsed();
        assert_eq!(found.len(), FILES);
        lexical = lexical.min(elapsed);

        // The same enumeration, canonicalising each entry the way the walk used to.
        let started = Instant::now();
        let mut count = 0usize;
        for entry in &entries {
            let absolute = vault.resolve(&entry.path);
            if vault.relativise(&absolute).is_ok() {
                count += 1;
            }
        }
        let elapsed = started.elapsed();
        assert_eq!(count, FILES);
        canonical = canonical.min(elapsed);
    }

    println!(
        "walk of {FILES} files: lexical {lexical:?}, canonicalising {canonical:?} ({:.1}x)",
        canonical.as_secs_f64() / lexical.as_secs_f64().max(f64::EPSILON)
    );

    // Deliberately a weak inequality. The point is to catch canonicalisation coming
    // back, not to pin a ratio to a machine: RM-1 measured 1.42 s of a 1.85 s walk, and
    // a CI runner under contention will not reproduce that multiple. A test asserting
    // "at least 10x" would fail for reasons that say nothing about the code, which is
    // its own kind of guard that cannot be trusted.
    assert!(
        canonical > lexical,
        "canonicalising every entry ({canonical:?}) was not slower than the lexical \
         strip ({lexical:?}); either the walk has started canonicalising again, or this \
         measurement is too small to mean anything"
    );
}
