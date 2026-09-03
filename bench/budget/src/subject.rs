//! The seam — the only module in the harness that touches a Serat crate.
//!
//! Everything measured enters through one of the three functions below. They
//! return an error today because the entry points they need do not exist yet:
//! `serat-index` is scaffolded (task I1) and gets its rebuild in I4, its
//! incremental save path in I5; the cold-start marks are task S1 once the
//! architect's D2 defines them. **The jobs therefore land red on purpose.** A
//! budget job that skips when the thing it measures is absent is worse than no
//! job, because a green build then means nothing.
//!
//! Keeping the seam in one file is deliberate: when I4 lands, exactly one
//! function changes and no measurement code moves. The signatures below are the
//! request to `rust-core-dev` — anything that satisfies them satisfies the
//! harness.

use std::path::Path;
use std::time::Duration;

use crate::Samples;

/// What the harness needs from `serat-index` before NFR-PRF-04 is measurable.
pub const REBUILD_ENTRY_POINT: &str = "\
NFR-PRF-04 needs, from serat-index (task I4):

    // Walk `vault`, parse every note, populate a fresh index at `index_dir`,
    // and return only when the writer has drained and ANALYZE has run.
    pub fn rebuild(vault: &Path, index_dir: &Path) -> Result<RebuildReport, IndexError>;

    pub struct RebuildReport { pub notes: usize, pub blocks: usize, pub links: usize, ... }

Callable from a plain binary with no Tauri app running. The harness times the
call with a monotonic clock, does not time the vault generation, and asserts the
returned note count against the vault manifest so a rebuild that quietly indexed
half the vault cannot post a fast number.";

/// Two intervals, because P0 finding F-03 showed that "save to index" has two
/// defensible readings that differ by more than the budget itself.
#[derive(Clone, Debug, Default)]
pub struct SaveSamples {
    /// File written to row visible. What a user experiences, and what includes
    /// the 150 ms per-path coalescing window of architecture section 3.1 step 1.
    /// This reading cannot fit inside 50 ms by construction; measuring it anyway
    /// is how the gate report shows *why*, rather than asserting it is impossible.
    pub end_to_end: Samples,
    /// Watcher event dequeued to row visible. The pipeline's own cost, with the
    /// debounce excluded. The only reading of NFR-PRF-03 that is satisfiable under
    /// the current design, and therefore the one asserted -- pending the
    /// architect's ruling on F-03.
    pub pipeline: Samples,
}

/// What the harness needs before NFR-PRF-03 is measurable.
pub const SAVE_ENTRY_POINT: &str = "\
NFR-PRF-03 needs, from serat-index / serat-vault (task I5):

    // Open an already-built index over `vault`.
    pub fn open(vault: &Path, index_dir: &Path) -> Result<IndexHandle, IndexError>;

    // Submit one changed file and block until it is visible to a reader.
    // The budget is save -> queryable, so the harness must be able to observe
    // the point at which a query would see the change (ADR-0001's flush, or an
    // index.changed notification carrying the same guarantee).
    pub fn apply_and_wait(&self, path: &VaultPath, bytes: &[u8]) -> Result<(), IndexError>;

    // And, for F-03, the moment the watcher event left the queue, so the
    // 150 ms coalescing window is reported separately from the work:
    pub fn last_dequeued_at(&self) -> Instant;   // or a timestamped event

The harness edits 1,000 distinct notes drawn across the vault, one at a time,
and reports BOTH intervals: file-written-to-visible, and dequeued-to-visible.
It needs distinct notes rather than one note a thousand times, because
re-saving one note measures a warm row and not the budget.

F-03: end to end, the design misses 50 ms by construction, because the
coalescing window alone is 150 ms. Until the architect rules on which interval
the requirement means, the harness asserts the pipeline interval and prints the
end-to-end figure beside it, so the gate report states which number the claim
rests on instead of implying there was only ever one.";

/// What the harness needs before NFR-PRF-01 is measurable.
pub const COLD_START_ENTRY_POINT: &str = "\
NFR-PRF-01 needs, from serat-app (task S1) and a decision from D2:

    SERAT_STARTUP_TRACE=<file> serat.exe --vault <dir> --exit-after-ready

    ... writes one line of JSON and exits:
    {\"t0_to_window_visible_ms\": 312.4, \"t0_to_vault_usable_ms\": 803.1}

t0 is process spawn measured by the harness, not an Instant::now() inside main:
everything before main is cold-start cost the user feels. `window visible` must
be the first painted frame, not window creation. `vault usable` is D2's to
define, and the harness asserts whatever D2 says. Until D2 lands this job is
blocked rather than guessed at.";

/// Time one full rebuild of the vault into a throwaway index directory.
///
/// # Errors
/// Today: always, because `serat-index` has no rebuild entry point yet.
pub fn full_rebuild(vault: &Path, index_dir: &Path) -> Result<Duration, String> {
    let _ = (vault, index_dir);
    // Touching the crates keeps the dependency edge real, so the day the entry
    // point lands there is nothing to wire up but the call itself.
    Err(format!(
        "serat-index (schema v{}, sqlite {}) exposes no rebuild entry point yet — task I4.\n\n{}",
        serat_index::SCHEMA_VERSION,
        serat_index::sqlite_version(),
        REBUILD_ENTRY_POINT
    ))
}

/// Time `iterations` single-note saves through to being visible in the index,
/// on both of F-03's intervals.
///
/// # Errors
/// Today: always, because the incremental save path does not exist yet.
pub fn save_to_index(
    vault: &Path,
    index_dir: &Path,
    iterations: usize,
) -> Result<SaveSamples, String> {
    let _ = (vault, index_dir, iterations);
    // serat-vault's hash gate is the first step of the path being measured
    // (architecture §3.1 step 3); calling it here proves the link, and the
    // measurement will use it for real.
    let _gate = serat_vault::hash_bytes(b"---\ntype: note\n---\n");
    Err(format!(
        "the incremental save path does not exist yet — task I5.\n\n{SAVE_ENTRY_POINT}"
    ))
}

/// Run `iterations` cold starts, returning the window-visible and vault-usable
/// timelines.
///
/// # Errors
/// Today: always, because neither the app nor D2's definition of the marks
/// exists.
pub fn cold_start(vault: &Path, iterations: usize) -> Result<(Samples, Samples), String> {
    let _ = (vault, iterations);
    Err(format!(
        "there is no application to start yet — tasks D2 and S1.\n\n{COLD_START_ENTRY_POINT}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seam_fails_loudly_and_says_what_is_missing() {
        // The point of these assertions is that a red job is legible: whoever
        // reads the CI log learns which task unblocks it, not merely that
        // something is wrong.
        let vault = Path::new("nowhere");
        let index = Path::new("nowhere/.serat");

        let e = full_rebuild(vault, index).expect_err("must not silently pass");
        assert!(e.contains("task I4"), "{e}");
        assert!(e.contains("rebuild("), "{e}");

        let e = save_to_index(vault, index, 10).expect_err("must not silently pass");
        assert!(e.contains("task I5"), "{e}");

        let e = cold_start(vault, 20).expect_err("must not silently pass");
        assert!(e.contains("D2"), "{e}");
        assert!(e.contains("SERAT_STARTUP_TRACE"), "{e}");
    }
}
